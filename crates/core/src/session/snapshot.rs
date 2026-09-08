use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub fn sha256_of_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn sha256_of_file(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_of_bytes(&bytes))
}

pub fn sha256_of_str(s: &str) -> String {
    sha256_of_bytes(s.as_bytes())
}

pub fn snapshot_path(session_id: &str, seq: i64, path: &str) -> PathBuf {
    let file = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    PathBuf::from(format!(".vioraharness/snapshots/{session_id}/{seq}/{file}"))
}

#[derive(Debug, Default)]
pub struct UndoStack {
    inner: Mutex<HashMap<String, Vec<Snapshot>>>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub session_id: String,
    pub seq: i64,
    pub path: String,
    pub sha: String,
    pub content: Vec<u8>,
}

impl UndoStack {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn push(&self, session_id: &str, seq: i64, path: &str) {
        if let Ok(bytes) = std::fs::read(path) {
            let sha = sha256_of_bytes(&bytes);
            let snap = Snapshot {
                session_id: session_id.into(),
                seq,
                path: path.into(),
                sha: sha.clone(),
                content: bytes.clone(),
            };
            let mut g = self.inner.lock().await;
            g.entry(session_id.into()).or_default().push(snap);

            if let Some(v) = g.get_mut(session_id) {
                if v.len() > 20 {
                    v.remove(0);
                }
            }

            let sp = snapshot_path(session_id, seq, path);
            if let Some(parent) = sp.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&sp, &g.get(session_id).unwrap().last().unwrap().content);

            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            if let Ok(store) = crate::session::SessionStore::new(&db) {
                let _ = store.insert_snapshot(session_id, seq, path, &sha, &bytes);
            }
        }
    }

    pub async fn pop(&self, session_id: &str) -> Option<Snapshot> {
        let mut g = self.inner.lock().await;
        let v = g.get_mut(session_id)?;
        let snap = v.pop()?;

        let _ = std::fs::write(&snap.path, &snap.content);
        Some(snap)
    }

    pub async fn list(&self, session_id: &str) -> Vec<Snapshot> {
        let g = self.inner.lock().await;
        g.get(session_id).cloned().unwrap_or_default()
    }
}

pub fn restore_latest(
    store: &super::store::SessionStore,
    session_id: &str,
) -> Result<Option<String>> {
    let Some((path, content)) = store.pop_snapshot(session_id)? else {
        return Ok(None);
    };
    let target = resolve_session_path(store, session_id, &path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &content)?;
    Ok(Some(target.to_string_lossy().to_string()))
}

fn resolve_session_path(
    store: &super::store::SessionStore,
    session_id: &str,
    path: &str,
) -> Result<PathBuf> {
    let base = store
        .get_session(session_id)?
        .and_then(|s| s.cwd)
        .filter(|c| !c.trim().is_empty())
        .map(PathBuf::from);
    let target = PathBuf::from(path);
    Ok(match (target.is_absolute(), base) {
        (true, _) => target,
        (false, Some(b)) => b.join(target),
        (false, None) => target,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct RewindReport {
    pub target_seq: i64,
    pub dropped_messages: usize,
    pub dropped_tool_calls: usize,
    /// Paths restored to their at-checkpoint content.
    pub restored_files: Vec<String>,
    /// Paths with no snapshot at/before the checkpoint: restored to the
    /// earliest known content (file may have been created after the
    /// checkpoint — never deleted, user decides).
    pub earliest_state_files: Vec<String>,
}

/// Rewind a session to `target_seq`: every snapshotted file returns to its
/// newest snapshot at or before the checkpoint (or earliest known content),
/// snapshots and conversation rows after the checkpoint are dropped.
/// Files are never deleted. Conversation events are kept as an audit trail.
pub fn rewind_to_seq(
    store: &super::store::SessionStore,
    session_id: &str,
    target_seq: i64,
) -> Result<RewindReport> {
    let snaps = store.get_snapshot_contents(session_id)?;
    let mut by_path: HashMap<String, Vec<(i64, Vec<u8>)>> = HashMap::new();
    for (seq, path, content) in snaps {
        by_path.entry(path).or_default().push((seq, content));
    }
    let mut restored_files = Vec::new();
    let mut earliest_state_files = Vec::new();
    let mut paths: Vec<&String> = by_path.keys().collect();
    paths.sort();
    for path in paths {
        let versions = &by_path[path];
        let at_or_before = versions.iter().rfind(|(s, _)| *s <= target_seq);
        let (content, earliest) = match at_or_before {
            Some((_, c)) => (c, false),
            None => (&versions[0].1, true),
        };
        let target = resolve_session_path(store, session_id, path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        if earliest {
            earliest_state_files.push(path.clone());
        } else {
            restored_files.push(path.clone());
        }
    }
    store.delete_snapshots_after(session_id, target_seq)?;
    let (dropped_messages, dropped_tool_calls) =
        store.delete_messages_after(session_id, target_seq)?;
    store.touch_session(session_id, None)?;
    Ok(RewindReport {
        target_seq,
        dropped_messages,
        dropped_tool_calls,
        restored_files,
        earliest_state_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_of_str(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sha256_of_bytes(b"abc").len(), 64);
        assert_eq!(sha256_of_str("abc"), sha256_of_bytes(b"abc"));
        assert_ne!(sha256_of_str("a"), sha256_of_str("b"));
    }

    #[test]
    fn sha256_of_missing_file_is_none() {
        assert!(sha256_of_file("/nonexistent-vh-xyz/file.txt").is_none());
        let p = std::env::temp_dir().join("vh_sha_test.txt");
        std::fs::write(&p, b"data").unwrap();
        assert_eq!(
            sha256_of_file(p.to_str().unwrap()).unwrap(),
            sha256_of_str("data")
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn snapshot_path_shape() {
        let p = snapshot_path("sess-1", 7, "/proj/sub/deck.cir");
        assert_eq!(
            p,
            PathBuf::from(".vioraharness/snapshots/sess-1/7/deck.cir")
        );
    }

    #[tokio::test]
    async fn undo_stack_push_pop_list_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_db = std::env::var("VIORAHARNESS_DB").ok();
        let prev_cwd = std::env::current_dir().unwrap();
        let work = std::env::temp_dir().join(format!("vh_undo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        std::env::set_var("VIORAHARNESS_DB", work.join("t.db").to_str().unwrap());
        std::env::set_current_dir(&work).unwrap();

        let target = work.join("f.txt");
        std::fs::write(&target, b"v1").unwrap();
        let stack = UndoStack::new();
        stack.push("sess", 1, target.to_str().unwrap()).await;
        std::fs::write(&target, b"v2").unwrap();
        stack.push("sess", 2, target.to_str().unwrap()).await;

        let list = stack.list("sess").await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].seq, 1);
        assert_eq!(list[1].seq, 2);
        assert_eq!(list[0].sha, sha256_of_str("v1"));

        let snap = stack.pop("sess").await.unwrap();
        assert_eq!(snap.content, b"v2");
        assert_eq!(std::fs::read(&target).unwrap(), b"v2");
        assert!(stack.pop("nosuch").await.is_none());

        std::env::set_current_dir(&prev_cwd).unwrap();
        match prev_db {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_dir_all(&work);
    }

    #[tokio::test]
    async fn undo_stack_caps_at_20() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_db = std::env::var("VIORAHARNESS_DB").ok();
        let prev_cwd = std::env::current_dir().unwrap();
        let work = std::env::temp_dir().join(format!("vh_undo2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        std::env::set_var("VIORAHARNESS_DB", work.join("t.db").to_str().unwrap());
        std::env::set_current_dir(&work).unwrap();

        let target = work.join("g.txt");
        let stack = UndoStack::new();
        for i in 0..25 {
            std::fs::write(&target, format!("v{i}")).unwrap();
            stack.push("sess", i, target.to_str().unwrap()).await;
        }
        assert_eq!(stack.list("sess").await.len(), 20);

        std::env::set_current_dir(&prev_cwd).unwrap();
        match prev_db {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn rewind_restores_files_and_truncates_conversation() {
        use crate::session::SessionStore;
        let work = std::env::temp_dir().join(format!("vh_rewind_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let f = work.join("f.txt");
        let g = work.join("g.txt");
        let s = SessionStore::new_in_memory().unwrap();
        s.create_session("sess", "m", None).unwrap();
        for i in 1..=4 {
            s.append_message("sess", "user", &format!("m{i}")).unwrap();
        }
        s.record_tool_call("c4", "sess", 4, "write", &serde_json::json!({}))
            .unwrap();
        let fs = f.to_str().unwrap().to_string();
        let gs = g.to_str().unwrap().to_string();
        s.insert_snapshot("sess", 1, &fs, "s", b"v0").unwrap();
        s.insert_snapshot("sess", 3, &fs, "s", b"v1").unwrap();
        s.insert_snapshot("sess", 2, &gs, "s", b"g").unwrap();
        std::fs::write(&f, b"v1").unwrap();

        let rep = rewind_to_seq(&s, "sess", 2).unwrap();
        assert_eq!(rep.target_seq, 2);
        assert_eq!((rep.dropped_messages, rep.dropped_tool_calls), (2, 1));
        assert_eq!(rep.restored_files, vec![fs.clone(), gs.clone()]);
        assert!(rep.earliest_state_files.is_empty());
        assert_eq!(std::fs::read(&f).unwrap(), b"v0");
        assert_eq!(std::fs::read(&g).unwrap(), b"g");
        assert_eq!(s.get_messages("sess").unwrap().len(), 2);
        assert!(s.get_tool_calls_grouped_simple("sess").unwrap().is_empty());
        assert!(s
            .get_snapshots("sess")
            .unwrap()
            .iter()
            .all(|(q, _, _)| *q <= 2));
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn rewind_never_deletes_files_without_checkpoint() {
        use crate::session::SessionStore;
        let work = std::env::temp_dir().join(format!("vh_rewind2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let n = work.join("new.txt");
        let s = SessionStore::new_in_memory().unwrap();
        s.create_session("sess", "m", None).unwrap();
        s.append_message("sess", "user", "hi").unwrap();
        let ns = n.to_str().unwrap().to_string();
        s.insert_snapshot("sess", 5, &ns, "s", b"created-state")
            .unwrap();
        std::fs::write(&n, b"later").unwrap();

        let rep = rewind_to_seq(&s, "sess", 2).unwrap();
        assert!(rep.restored_files.is_empty());
        assert_eq!(rep.earliest_state_files, vec![ns]);
        assert!(n.exists(), "rewind never deletes files");
        assert_eq!(std::fs::read(&n).unwrap(), b"created-state");
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn rewind_to_latest_drops_nothing() {
        use crate::session::SessionStore;
        let work = std::env::temp_dir().join(format!("vh_rewind3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let f = work.join("f.txt");
        let s = SessionStore::new_in_memory().unwrap();
        s.create_session("sess", "m", None).unwrap();
        s.append_message("sess", "user", "hi").unwrap();
        let fs = f.to_str().unwrap().to_string();
        s.insert_snapshot("sess", 1, &fs, "s", b"v").unwrap();

        let rep = rewind_to_seq(&s, "sess", 99).unwrap();
        assert_eq!((rep.dropped_messages, rep.dropped_tool_calls), (0, 0));
        assert_eq!(rep.restored_files, vec![fs]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn rewind_resolves_relative_paths_via_session_cwd() {
        use crate::session::SessionStore;
        let work = std::env::temp_dir().join(format!("vh_rewind4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(work.join("sub")).unwrap();
        let s = SessionStore::new_in_memory().unwrap();
        s.create_session_full(
            "sess",
            "m",
            None,
            Some(work.to_str().unwrap()),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        s.append_message("sess", "user", "one").unwrap();
        s.append_message("sess", "user", "two").unwrap();
        s.insert_snapshot("sess", 1, "sub/rel.cir", "s", b"orig")
            .unwrap();
        std::fs::write(work.join("sub/rel.cir"), b"modified").unwrap();

        let rep = rewind_to_seq(&s, "sess", 1).unwrap();
        assert_eq!(rep.restored_files, vec!["sub/rel.cir".to_string()]);
        assert_eq!(std::fs::read(work.join("sub/rel.cir")).unwrap(), b"orig");
        let _ = std::fs::remove_dir_all(&work);
    }
}
