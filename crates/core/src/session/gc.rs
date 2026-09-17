// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::store::SessionStore;

/// What `gc_snapshots` removed (or would remove with `dry_run`).
#[derive(Debug, Clone, PartialEq)]
pub struct GcReport {
    pub snapshot_rows_deleted: usize,
    pub snapshot_sessions_pruned: Vec<String>,
    pub snapshot_dirs_removed: usize,
    pub bytes_freed: u64,
    pub dry_run: bool,
}

/// Garbage-collect snapshot state for sessions idle longer than `max_idle`.
///
/// Two sides, kept consistent. DB side: snapshot rows of sessions whose
/// `updated_at` is older than the cutoff, plus rows of sessions missing
/// from the DB entirely (orphans). Conversation messages are never touched,
/// only undo/rewind fuel. Disk side: `<snapshots_root>/<session>/<seq>/`
/// dirs belonging to pruned sessions, or older than the cutoff themselves
/// (covers DB-less runs). Empty session dirs are removed afterwards.
pub fn gc_snapshots(
    store: &SessionStore,
    snapshots_root: &Path,
    max_idle: Duration,
    dry_run: bool,
) -> Result<GcReport> {
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff_secs = now_secs.saturating_sub(max_idle.as_secs() as i64);

    // Stale = idle past the cutoff. updated_at <= 0 means pre-migration rows.
    let mut stale: HashSet<String> = HashSet::new();
    for s in store.list_sessions_filtered(None, None, true, 100_000, 0)? {
        if s.updated_at <= cutoff_secs {
            stale.insert(s.id);
        }
    }
    // Orphan snapshot rows (session gone from the DB).
    for sid in store.snapshot_session_ids().unwrap_or_default() {
        if store.get_session(&sid)?.is_none() {
            stale.insert(sid);
        }
    }

    let mut rows_deleted = 0;
    let mut pruned: Vec<String> = stale.iter().cloned().collect();
    pruned.sort();
    for sid in &pruned {
        let n = if dry_run {
            count_snapshot_rows(store, sid)?
        } else {
            delete_snapshot_rows(store, sid)?
        };
        rows_deleted += n;
    }

    let (mut dirs_removed, mut bytes_freed) = (0usize, 0u64);
    if snapshots_root.is_dir() {
        let cutoff_time = SystemTime::now()
            .checked_sub(max_idle)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Ok(sessions) = std::fs::read_dir(snapshots_root) {
            for sess in sessions.flatten() {
                let sess_path = sess.path();
                if !sess_path.is_dir() {
                    continue;
                }
                let sid = sess.file_name().to_string_lossy().to_string();
                let session_stale = stale.contains(&sid);
                if let Ok(seqs) = std::fs::read_dir(&sess_path) {
                    for seq in seqs.flatten() {
                        let p = seq.path();
                        if !p.is_dir() {
                            continue;
                        }
                        let old = p
                            .metadata()
                            .and_then(|m| m.modified())
                            .map(|t| t <= cutoff_time)
                            .unwrap_or(false);
                        if session_stale || old {
                            let bytes = dir_size(&p);
                            if !dry_run {
                                let _ = std::fs::remove_dir_all(&p);
                            }
                            dirs_removed += 1;
                            bytes_freed += bytes;
                        }
                    }
                }
                if !dry_run {
                    let empty = std::fs::read_dir(&sess_path)
                        .map(|mut e| e.next().is_none())
                        .unwrap_or(false);
                    if empty {
                        let _ = std::fs::remove_dir_all(&sess_path);
                    }
                }
            }
        }
    }

    Ok(GcReport {
        snapshot_rows_deleted: rows_deleted,
        snapshot_sessions_pruned: pruned,
        snapshot_dirs_removed: dirs_removed,
        bytes_freed,
        dry_run,
    })
}

fn count_snapshot_rows(store: &SessionStore, session_id: &str) -> Result<usize> {
    Ok(store.get_snapshots(session_id)?.len())
}

fn delete_snapshot_rows(store: &SessionStore, session_id: &str) -> Result<usize> {
    let n = store.get_snapshots(session_id)?.len();
    store.delete_all_snapshots(session_id)?;
    Ok(n)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&p) else {
            continue;
        };
        for e in entries.flatten() {
            let fp = e.path();
            if fp.is_dir() {
                stack.push(fp);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_old(path: &Path) {
        // Backdate without extra deps (linux CI).
        let _ = std::process::Command::new("touch")
            .args(["-d", "10 days ago"])
            .arg(path)
            .output();
    }

    fn file_store(
        tag: &str,
    ) -> (
        SessionStore,
        std::path::PathBuf,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("vh_gc_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        let store = SessionStore::new(db.to_str().unwrap()).expect("file-backed store for gc test");
        (store, dir, guard)
    }

    fn backdate_session(db: &Path, id: &str, secs_ago: i64) {
        let con = rusqlite::Connection::open(db).unwrap();
        let now: i64 = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        con.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now - secs_ago, id],
        )
        .unwrap();
    }

    #[test]
    fn prunes_stale_session_state_only() {
        let (store, dir, _guard) = file_store("prune");
        let root = dir.join("snaps");
        // stale session: old + snapshots on disk
        store.create_session("old-sess", "m", None).unwrap();
        store
            .insert_snapshot("old-sess", 1, "f.txt", "s", b"data-bytes")
            .unwrap();
        backdate_session(&dir.join("t.db"), "old-sess", 30 * 24 * 3600);
        let sdir = root.join("old-sess").join("1");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("f.txt"), b"data-bytes").unwrap();
        touch_old(&sdir);
        // fresh session: untouched
        store.create_session("new-sess", "m", None).unwrap();
        store
            .insert_snapshot("new-sess", 1, "g.txt", "s", b"z")
            .unwrap();
        let ndir = root.join("new-sess").join("1");
        std::fs::create_dir_all(&ndir).unwrap();
        std::fs::write(ndir.join("g.txt"), b"z").unwrap();

        let rep = gc_snapshots(&store, &root, Duration::from_secs(7 * 24 * 3600), false).unwrap();
        assert_eq!(rep.snapshot_rows_deleted, 1);
        assert_eq!(rep.snapshot_sessions_pruned, vec!["old-sess".to_string()]);
        assert_eq!(rep.snapshot_dirs_removed, 1);
        assert_eq!(rep.bytes_freed, "data-bytes".len() as u64);
        assert!(store.get_snapshots("old-sess").unwrap().is_empty());
        assert_eq!(store.get_snapshots("new-sess").unwrap().len(), 1);
        assert!(ndir.exists(), "fresh dir kept");
        assert!(!root.join("old-sess").exists(), "emptied session dir gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let (store, dir, _guard) = file_store("dry");
        store.create_session("old-sess", "m", None).unwrap();
        store
            .insert_snapshot("old-sess", 1, "f.txt", "s", b"data")
            .unwrap();
        backdate_session(&dir.join("t.db"), "old-sess", 30 * 24 * 3600);
        let root = dir.join("snaps");
        let sdir = root.join("old-sess").join("1");
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("f.txt"), b"data").unwrap();
        touch_old(&sdir);

        let rep = gc_snapshots(&store, &root, Duration::from_secs(7 * 24 * 3600), true).unwrap();
        assert!(rep.dry_run);
        assert_eq!(rep.snapshot_rows_deleted, 1, "would delete");
        assert_eq!(rep.snapshot_dirs_removed, 1, "would remove");
        assert_eq!(store.get_snapshots("old-sess").unwrap().len(), 1, "kept");
        assert!(sdir.exists(), "dir kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_dirs_age_out_without_db() {
        let (store, dir, _guard) = file_store("orphan");
        let root = dir.join("snaps");
        let odir = root.join("ghost-sess").join("3");
        std::fs::create_dir_all(&odir).unwrap();
        std::fs::write(odir.join("f.txt"), b"xy").unwrap();
        touch_old(&odir);
        let fresh = root.join("ghost-fresh").join("1");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(fresh.join("f.txt"), b"xy").unwrap();

        let rep = gc_snapshots(&store, &root, Duration::from_secs(7 * 24 * 3600), false).unwrap();
        assert_eq!(rep.snapshot_dirs_removed, 1);
        assert!(!odir.exists(), "old orphan gone");
        assert!(
            fresh.exists(),
            "fresh orphan kept (could be a live session)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
