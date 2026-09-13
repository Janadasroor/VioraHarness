use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Max snapshot rows kept per session (DB + `.vioraharness/snapshots/`).
/// Undo/rewind history stays deep enough for real sessions; growth is bounded.
pub const MAX_SNAPSHOTS_PER_SESSION: i64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub model: String,
    pub status: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub project_hash: Option<String>,
    pub parent_id: Option<String>,
    pub archived_at: Option<i64>,
    pub model_last: Option<String>,
    pub theme: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub content_json: Option<String>,
    pub reasoning: Option<String>,
    pub model: Option<String>,
    pub tool_call_id: Option<String>,
    pub created_at: i64,
    pub timestamp: Option<String>,
    pub is_compaction: bool,
}

pub type ToolCallGrouped =
    std::collections::HashMap<i64, Vec<(String, String, String, Option<String>, Option<String>)>>;

mod messages;
mod queries;
mod sessions;
mod snapshots;

pub struct SessionStore {
    pub(crate) pool: Pool<SqliteConnectionManager>,
}

impl SessionStore {
    pub fn new(db_path: &str) -> Result<Self> {
        let expanded = shellexpand_path(db_path);
        if let Some(parent) = std::path::Path::new(&expanded).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let manager = SqliteConnectionManager::file(&expanded);
        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .context("r2d2 pool build")?;

        {
            let conn = pool.get()?;
            let _ = conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
            );
        }
        let s = Self { pool };
        s.migrate()?;
        Ok(s)
    }

    pub fn new_in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::new(manager)?;
        {
            let conn = pool.get()?;
            let _ = conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;");
        }
        let s = Self { pool };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.pool.get()?;

        const SQL001_EMBEDDED: &str = include_str!("../../../../../migrations/001_init.sql");
        const SQL002_EMBEDDED: &str = include_str!("../../../../../migrations/002_session_v2.sql");
        let sql001 = std::fs::read_to_string("migrations/001_init.sql")
            .or_else(|_| std::fs::read_to_string("../migrations/001_init.sql"))
            .or_else(|_| std::fs::read_to_string("../../migrations/001_init.sql"))
            .or_else(|_| std::fs::read_to_string("../../../migrations/001_init.sql"))
            .or_else(|_| std::fs::read_to_string("../../../../migrations/001_init.sql"))
            .unwrap_or_else(|_| SQL001_EMBEDDED.to_string());

        let _ = conn.execute_batch(&sql001);

        let sql002 = std::fs::read_to_string("migrations/002_session_v2.sql")
            .or_else(|_| std::fs::read_to_string("../migrations/002_session_v2.sql"))
            .or_else(|_| std::fs::read_to_string("../../migrations/002_session_v2.sql"))
            .or_else(|_| std::fs::read_to_string("../../../migrations/002_session_v2.sql"))
            .or_else(|_| std::fs::read_to_string("../../../../migrations/002_session_v2.sql"))
            .unwrap_or_else(|_| SQL002_EMBEDDED.to_string());
        if !sql002.is_empty() {
            let sql_no_comments = sql002
                .lines()
                .filter(|l| !l.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            for raw_stmt in sql_no_comments.split(';') {
                let t = raw_stmt.trim().to_string();
                if t.is_empty() {
                    continue;
                }

                let res = conn.execute_batch(&format!("{t};"));
                if let Err(e) = res {
                    let msg = e.to_string();
                    if msg.contains("duplicate column name") || msg.contains("already exists") {
                        continue;
                    }

                    eprintln!("migrate 002 warning: {e} for `{t}`");
                }
            }

            let _ = conn.execute(
                "UPDATE events SET seq = (SELECT COUNT(*) FROM events e2 WHERE e2.session_id = events.session_id AND e2.id <= events.id) WHERE seq IS NULL",
                [],
            );

            let _ = conn.execute(
                "UPDATE sessions SET updated_at = created_at WHERE updated_at IS NULL OR updated_at = 0",
                [],
            );

            // 004: task-wake prompts were persisted as role=user,
            // impersonating the user in history. Re-tag rows only the
            // harness itself produced (task_wake_prompt is the sole
            // writer of this prefix). Idempotent.
            let _ = conn.execute(
                "UPDATE messages SET role = 'system' WHERE role = 'user' AND content LIKE '[background task finished] %'",
                [],
            );

            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;");
        }

        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN theme TEXT", []);
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN timestamp TEXT", []);

        // 003: tool_calls PK (session_id, id) so forks keep tool history.
        // Legacy DBs have id-only PK and silently drop forked tool calls.
        // Guarded: runs once, only when the old schema is detected.
        let tc_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'tool_calls'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        if !tc_sql.is_empty() && !tc_sql.contains("PRIMARY KEY (session_id, id)") {
            let _ = conn.execute_batch(
                "CREATE TABLE tool_calls_new (
                    id TEXT NOT NULL,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    message_seq INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    args TEXT NOT NULL,
                    result TEXT,
                    status TEXT NOT NULL DEFAULT 'pending',
                    created_at INTEGER,
                    settled_at INTEGER,
                    PRIMARY KEY (session_id, id)
                );",
            );
            let old_rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))
                .unwrap_or(-1);
            let copied = conn
                .execute(
                    "INSERT OR IGNORE INTO tool_calls_new (id, session_id, message_seq, name, args, result, status, created_at, settled_at) SELECT id, session_id, message_seq, name, args, result, status, created_at, settled_at FROM tool_calls",
                    [],
                )
                .unwrap_or(0) as i64;
            // Only swap when every row survived (legacy ids are globally
            // unique, so OR IGNORE cannot drop rows here).
            if old_rows >= 0 && copied == old_rows {
                let _ = conn.execute_batch(
                    "DROP TABLE tool_calls;
                     ALTER TABLE tool_calls_new RENAME TO tool_calls;",
                );
            } else {
                let _ = conn.execute_batch("DROP TABLE IF EXISTS tool_calls_new;");
                eprintln!("migrate 003 skipped: copy mismatch ({copied}/{old_rows}), keeping legacy tool_calls");
            }
        }

        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS todos (
                session_id TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                priority TEXT,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (session_id, content)
            );
            CREATE INDEX IF NOT EXISTS idx_todos_session ON todos(session_id);",
        );
        Ok(())
    }

    fn now() -> i64 {
        chrono_now()
    }

    fn cwd_and_hash() -> (String, String) {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into());
        let hash = format!("{:x}", md5ish(&cwd));
        (cwd, hash)
    }
}

fn shellexpand_path(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub(crate) fn md5ish(s: &str) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Project hash for a cwd — the same derivation `create_session` stores,
/// so pickers can scope their lists to the current project.
pub fn project_hash_for_cwd(cwd: &str) -> String {
    format!("{:x}", md5ish(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_latest_resolves_session_cwd() {
        use crate::session::snapshot::restore_latest;
        let s = mem_store();
        let proj = std::env::temp_dir().join(format!("vh_undo_cwd_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proj);
        std::fs::create_dir_all(&proj).unwrap();
        s.create_session_full(
            "sess",
            "m",
            None,
            Some(&proj.to_string_lossy()),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        s.insert_snapshot("sess", 1, "rel.txt", "sha", b"data")
            .unwrap();

        let got = restore_latest(&s, "sess").unwrap().expect("restored");
        assert_eq!(
            std::path::PathBuf::from(&got),
            proj.join("rel.txt"),
            "{got}"
        );
        assert_eq!(std::fs::read(&got).unwrap(), b"data");
        assert!(restore_latest(&s, "sess").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn pop_snapshot_restores_newest_first() {
        let s = mem_store();
        s.create_session_full("sess", "m", None, None, None, None, None, None)
            .unwrap();
        assert!(s.pop_snapshot("sess").unwrap().is_none());
        s.insert_snapshot("sess", 1, "/tmp/a.txt", "sha1", b"one")
            .unwrap();
        s.insert_snapshot("sess", 2, "/tmp/a.txt", "sha2", b"two")
            .unwrap();
        assert_eq!(
            s.pop_snapshot("sess").unwrap(),
            Some(("/tmp/a.txt".into(), b"two".to_vec()))
        );
        assert_eq!(
            s.pop_snapshot("sess").unwrap(),
            Some(("/tmp/a.txt".into(), b"one".to_vec()))
        );
        assert!(s.pop_snapshot("sess").unwrap().is_none());
        assert!(s.pop_snapshot("nope").unwrap().is_none());
    }

    fn mem_store() -> SessionStore {
        SessionStore::new_in_memory().expect("in-memory store")
    }

    #[test]
    fn session_mode_roundtrips_and_forks() {
        let s = mem_store();
        s.create_session("m1", "m", Some("t")).unwrap();
        assert_eq!(s.get_session("m1").unwrap().unwrap().mode, None);
        s.set_session_mode("m1", "web").unwrap();
        assert_eq!(
            s.get_session("m1").unwrap().unwrap().mode.as_deref(),
            Some("web")
        );
        // Mode switches never reorder lists (updated_at untouched).
        let before = s.get_session("m1").unwrap().unwrap().updated_at;
        s.set_session_mode("m1", "eda").unwrap();
        assert_eq!(s.get_session("m1").unwrap().unwrap().updated_at, before);
        s.fork_session("m1", "m2", None).unwrap();
        assert_eq!(
            s.get_session("m2").unwrap().unwrap().mode.as_deref(),
            Some("eda"),
            "fork inherits parent mode"
        );
        let listed = s.list_sessions().unwrap();
        assert!(listed
            .iter()
            .any(|x| x.id == "m1" && x.mode.as_deref() == Some("eda")));
    }

    #[test]
    fn project_hash_scopes_picker_lists() {
        let s = mem_store();
        let ha = project_hash_for_cwd("/proj/a");
        let hb = project_hash_for_cwd("/proj/b");
        assert_ne!(ha, hb);
        assert_eq!(ha, project_hash_for_cwd("/proj/a"), "stable");
        s.create_session_full(
            "sess-a",
            "m",
            Some("t"),
            Some("/proj/a"),
            Some(&ha),
            None,
            None,
            None,
        )
        .unwrap();
        s.create_session_full(
            "sess-b",
            "m",
            Some("t"),
            Some("/proj/b"),
            Some(&hb),
            None,
            None,
            None,
        )
        .unwrap();
        let only_a = s
            .list_sessions_filtered(Some(&ha), None, true, 50, 0)
            .unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].id, "sess-a");
        let all = s.list_sessions_filtered(None, None, true, 50, 0).unwrap();
        assert_eq!(all.len(), 2);
    }

    fn mk(store: &SessionStore, id: &str, cwd: &str, with_msg: bool, updated: i64) {
        store
            .create_session_full(id, "m", Some("t"), Some(cwd), None, None, None, None)
            .unwrap();
        if with_msg {
            store.append_message(id, "user", "hi").unwrap();
        }

        store
            .pool
            .get()
            .unwrap()
            .execute(
                "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![updated, id],
            )
            .unwrap();
    }

    #[test]
    fn latest_for_cwd_prefers_exact_then_nested() {
        let s = mem_store();
        mk(&s, "a", "/x/proj", true, 100);
        mk(&s, "b", "/x/proj", true, 200);
        mk(&s, "c", "/x/proj/sub", true, 150);
        mk(&s, "d", "/other", true, 300);
        mk(&s, "e", "/x/proj", false, 400);
        let got = s.latest_for_cwd("/x/proj").expect("ok").expect("found");
        assert_eq!(got.id, "b");
        let got = s.latest_for_cwd("/x/proj/sub").expect("ok").expect("found");
        assert_eq!(got.id, "c");
        assert!(s.latest_for_cwd("/nowhere").expect("ok").is_none());
    }

    #[test]
    fn messages_roundtrip_with_seq_and_details() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        assert!(s.get_messages("sess").unwrap().is_empty());
        let q1 = s.append_message("sess", "user", "hello").unwrap();
        let q2 = s
            .append_message_full("sess", "assistant", "hi", None, Some("r"), Some("m"), None)
            .unwrap();
        assert_eq!((q1, q2), (1, 2));
        let msgs = s.get_messages("sess").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], (1, "user".into(), "hello".into()));
        let det = s.get_messages_detailed("sess").unwrap();
        assert_eq!(det[1].reasoning.as_deref(), Some("r"));
        assert_eq!(det[1].model.as_deref(), Some("m"));
    }

    #[test]
    fn migrate_004_retags_wake_prompts_as_system() {
        let db = std::env::temp_dir().join(format!("vh_mig004_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let path = db.to_string_lossy().to_string();
        {
            let s = SessionStore::new(&path).expect("open");
            s.create_session("sess", "m", None).unwrap();
            // Legacy rows, as run_inner stored them before the fix.
            s.append_message(
                "sess",
                "user",
                "[background task finished] abc123 (`sleep 1`) done (exit 0).\nLog tail:\n...",
            )
            .unwrap();
            s.append_message("sess", "user", "real prompt mentioning background work")
                .unwrap();
            s.append_message("sess", "assistant", "on it").unwrap();
        }
        // Reopen runs migrations.
        let s = SessionStore::new(&path).expect("reopen");
        let msgs = s.get_messages("sess").unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].1, "system", "wake prompt re-tagged");
        assert_eq!(msgs[1].1, "user", "real user prompt untouched");
        assert_eq!(msgs[2].1, "assistant");
        let _ = std::fs::remove_file(&db);
        for ext in ["wal", "shm", "journal"] {
            let _ = std::fs::remove_file(db.with_extension(format!("db-{ext}")));
        }
    }

    #[test]
    fn tool_calls_record_settle_fail_grouped() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        let seq = s.append_message("sess", "assistant", "calling").unwrap();
        s.record_tool_call("c1", "sess", seq, "read", &serde_json::json!({"path": "f"}))
            .unwrap();
        s.record_tool_call_with_sig(
            "c2",
            "sess",
            seq,
            "bash",
            &serde_json::json!({"command": "ls"}),
            Some("sig"),
        )
        .unwrap();
        s.settle_tool_call("sess", "c1", &serde_json::json!({"ok": true}))
            .unwrap();
        s.fail_tool_call("sess", "c2", "boom").unwrap();
        let grouped = s.get_tool_calls_grouped_simple("sess").unwrap();
        let calls = grouped.get(&seq).expect("grouped by message seq");
        assert_eq!(calls.len(), 2);
        let c1 = calls.iter().find(|c| c.0 == "c1").unwrap();
        assert_eq!(c1.1, "read");
        assert!(c1.2.contains("path"));
        let c2 = calls.iter().find(|c| c.0 == "c2").unwrap();
        assert_eq!(c2.3.as_deref(), Some("sig"));
    }

    #[test]
    fn latest_seq_tracks_max_message_seq() {
        // Snapshot writers must use this (never vec lengths or constants)
        // so rewind targets line up with stored snapshots.
        let s = mem_store();
        assert_eq!(s.latest_seq("sess").unwrap(), 0);
        s.create_session("sess", "m", None).unwrap();
        assert_eq!(s.latest_seq("sess").unwrap(), 0);
        s.append_message("sess", "user", "one").unwrap();
        s.append_message("sess", "assistant", "two").unwrap();
        assert_eq!(s.latest_seq("sess").unwrap(), 2);
        assert_eq!(s.latest_seq("missing").unwrap(), 0);
    }

    #[test]
    fn todos_replace_and_read() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        assert!(s.get_todos("sess").unwrap().is_empty());
        let n = s
            .set_todos(
                "sess",
                &[
                    ("a".into(), "in_progress".into(), Some("high".into())),
                    ("b".into(), "pending".into(), None),
                ],
            )
            .unwrap();
        assert_eq!(n, 2);
        let todos = s.get_todos("sess").unwrap();
        assert_eq!(
            todos[0],
            ("a".into(), "in_progress".into(), Some("high".into()))
        );
        let n = s
            .set_todos("sess", &[("c".into(), "completed".into(), None)])
            .unwrap();
        assert_eq!(n, 1, "replace, not merge");
        assert_eq!(s.get_todos("sess").unwrap().len(), 1);
    }

    #[test]
    fn fork_copies_messages_tools_todos() {
        let s = mem_store();
        s.create_session_full("p", "m", Some("orig"), None, None, None, None, None)
            .unwrap();
        let seq = s.append_message("p", "user", "hi").unwrap();
        s.record_tool_call("c1", "p", seq, "read", &serde_json::json!({}))
            .unwrap();
        s.set_todos("p", &[("t".into(), "pending".into(), None)])
            .unwrap();
        s.fork_session("p", "f", None).unwrap();
        assert_eq!(s.get_messages("f").unwrap().len(), 1);
        // Composite PK (session_id, id): forked tool calls survive alongside
        // the parent's rows.
        let forked_calls = s.get_tool_calls_grouped_simple("f").unwrap();
        assert_eq!(
            forked_calls.get(&seq).map(|v| v.len()),
            Some(1),
            "fork keeps tool history"
        );
        // Settling inside the fork must not touch the parent's row.
        s.settle_tool_call("f", "c1", &serde_json::json!({"ok": true}))
            .unwrap();
        let parent_calls = s.get_tool_calls_grouped("p").unwrap();
        let prow = parent_calls.get(&seq).unwrap()[0].clone();
        assert!(prow.4.is_none(), "parent result untouched by fork settle");
        let fork_calls = s.get_tool_calls_grouped("f").unwrap();
        assert!(fork_calls.get(&seq).unwrap()[0].4.is_some(), "fork settled");
        assert_eq!(s.get_todos("f").unwrap().len(), 1);
        let forked = s.get_session("f").unwrap().unwrap();
        assert_eq!(forked.parent_id.as_deref(), Some("p"));
        assert!(forked.title.unwrap().contains("fork"));
        assert!(s.fork_session("missing", "f2", None).is_err());
    }

    #[test]
    fn fork_at_seq_truncates_tail() {
        let s = mem_store();
        s.create_session("p", "m", None).unwrap();
        s.append_message("p", "user", "one").unwrap();
        s.append_message("p", "user", "two").unwrap();
        s.fork_session("p", "f", Some(1)).unwrap();
        let msgs = s.get_messages("f").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].2, "one");
    }

    #[test]
    fn migrate_003_rebuilds_legacy_tool_calls_pk() {
        // Legacy DBs have id-only PK: fork drops tool history. Opening the
        // DB must rebuild to (session_id, id) without losing rows.
        let dir = std::env::temp_dir().join(format!("vh_mig3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("legacy.db");
        {
            let con = rusqlite::Connection::open(&db).unwrap();
            con.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, model TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'active', title TEXT, updated_at INTEGER, cwd TEXT);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, seq INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, created_at INTEGER NOT NULL);
                 CREATE TABLE tool_calls (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_seq INTEGER NOT NULL, name TEXT NOT NULL, args TEXT NOT NULL, result TEXT, status TEXT NOT NULL DEFAULT 'pending', created_at INTEGER, settled_at INTEGER);
                 CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, type TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER);",
            )
            .unwrap();
            con.execute(
                "INSERT INTO sessions (id, created_at, updated_at, model, status) VALUES ('p', 1, 2, 'm', 'active')",
                [],
            )
            .unwrap();
            con.execute(
                "INSERT INTO messages (session_id, seq, role, content, created_at) VALUES ('p', 1, 'user', 'hi', 1)",
                [],
            )
            .unwrap();
            con.execute(
                "INSERT INTO tool_calls (id, session_id, message_seq, name, args, status) VALUES ('c1', 'p', 1, 'read', '{}', 'pending')",
                [],
            )
            .unwrap();
        }
        let s = SessionStore::new(db.to_str().unwrap()).unwrap();
        let schema: String = s
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'tool_calls'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(schema.contains("PRIMARY KEY (session_id, id)"), "{schema}");
        // Legacy row survived and fork now keeps it.
        s.fork_session("p", "f", None).unwrap();
        let calls = s.get_tool_calls_grouped_simple("f").unwrap();
        assert_eq!(calls.get(&1).map(|v| v.len()), Some(1));
        // Reopening is a no-op (idempotent migration).
        drop(s);
        let s2 = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(
            s2.get_tool_calls_grouped_simple("f")
                .unwrap()
                .get(&1)
                .map(|v| v.len()),
            Some(1)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_replace_drops_head_keeps_tail() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        for i in 0..5 {
            s.append_message("sess", "user", &format!("m{i}")).unwrap();
        }
        let (dropped, kept) = s.compact_replace("sess", 4, "summary here").unwrap();
        assert_eq!(dropped, 3);
        assert_eq!(kept, 2);
        let msgs = s.get_messages_detailed("sess").unwrap();
        assert!(msgs.iter().any(|m| m.content == "summary here"));
        assert!(msgs.iter().any(|m| m.content == "m3"));
        assert!(!msgs.iter().any(|m| m.content == "m0"));
    }

    #[test]
    fn archive_counts_and_rename_touch() {
        let s = mem_store();
        s.create_session("a", "m", None).unwrap();
        s.create_session("b", "m", None).unwrap();
        assert_eq!(s.count_sessions(false).unwrap(), 2);
        s.archive_session("a").unwrap();
        assert_eq!(s.count_sessions(false).unwrap(), 1);
        assert_eq!(s.count_sessions(true).unwrap(), 2);
        s.unarchive_session("a").unwrap();
        assert_eq!(s.count_sessions(false).unwrap(), 2);
        s.rename_session("a", "new title").unwrap();
        assert_eq!(
            s.get_session("a").unwrap().unwrap().title.as_deref(),
            Some("new title")
        );
        assert!(
            !s.ensure_session("a", "m", None).unwrap(),
            "existing touches"
        );
        assert!(s.ensure_session("c", "m", None).unwrap(), "missing creates");
        s.delete_session("c").unwrap();
        assert!(s.get_session("c").unwrap().is_none());
    }

    #[test]
    fn snapshots_list_and_export_jsonl() {
        let s = mem_store();
        s.create_session_full("sess", "m", Some("t"), None, None, None, None, None)
            .unwrap();
        s.append_message("sess", "user", "hello").unwrap();
        s.insert_snapshot("sess", 1, "f.txt", "s1", b"one").unwrap();
        s.insert_snapshot("sess", 2, "f.txt", "s2", b"two").unwrap();
        let snaps = s.get_snapshots("sess").unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].0, 2, "newest first");
        let jl = s.export_jsonl("sess").unwrap();
        let lines: Vec<&str> = jl.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], serde_json::json!("session"));
        assert_eq!(first["id"], serde_json::json!("sess"));
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["type"], serde_json::json!("message"));
        assert!(s.export_jsonl("missing").is_err());
    }

    #[test]
    fn export_jsonl_includes_todos() {
        // P1 #13: todos are auditable outside the session via export.
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        s.set_todos(
            "sess",
            &[
                ("a".into(), "completed".into(), None),
                ("b".into(), "in_progress".into(), Some("high".into())),
            ],
        )
        .unwrap();
        let jl = s.export_jsonl("sess").unwrap();
        let todos: Vec<serde_json::Value> = jl
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .filter(|v: &serde_json::Value| v["type"] == "todo")
            .collect();
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0]["content"], serde_json::json!("a"));
        assert_eq!(todos[1]["priority"], serde_json::json!("high"));
    }

    #[test]
    fn insert_snapshot_caps_per_session() {
        // P1 #11: snapshot history is bounded (DB rows; files best-effort).
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        for i in 0..(MAX_SNAPSHOTS_PER_SESSION + 10) {
            s.insert_snapshot("sess", i, "f.txt", "s", b"x").unwrap();
        }
        let snaps = s.get_snapshots("sess").unwrap();
        assert_eq!(snaps.len() as i64, MAX_SNAPSHOTS_PER_SESSION);
        assert_eq!(snaps[0].0, MAX_SNAPSHOTS_PER_SESSION + 9, "newest kept");
        // Other sessions are unaffected.
        s.create_session("other", "m", None).unwrap();
        s.insert_snapshot("other", 1, "g.txt", "s", b"y").unwrap();
        assert_eq!(s.get_snapshots("other").unwrap().len(), 1);
    }

    #[test]
    fn delete_messages_after_truncates_conversation() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        s.append_message("sess", "user", "one").unwrap();
        let seq = s.append_message("sess", "assistant", "two").unwrap();
        s.record_tool_call("c1", "sess", seq, "read", &serde_json::json!({}))
            .unwrap();
        s.append_message("sess", "user", "three").unwrap();
        let (msgs, calls) = s.delete_messages_after("sess", 1).unwrap();
        assert_eq!((msgs, calls), (2, 1));
        let rest = s.get_messages("sess").unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].2, "one");
        assert!(s.get_tool_calls_grouped_simple("sess").unwrap().is_empty());
        let (msgs, calls) = s.delete_messages_after("sess", 99).unwrap();
        assert_eq!((msgs, calls), (0, 0));
    }

    #[test]
    fn snapshot_contents_order_and_prune() {
        let s = mem_store();
        s.create_session("sess", "m", None).unwrap();
        s.insert_snapshot("sess", 2, "b.txt", "s", b"two").unwrap();
        s.insert_snapshot("sess", 1, "a.txt", "s", b"one").unwrap();
        let all = s.get_snapshot_contents("sess").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, 1, "ascending by message_seq");
        assert_eq!(s.delete_snapshots_after("sess", 1).unwrap(), 1);
        assert_eq!(s.get_snapshots("sess").unwrap().len(), 1);
    }
}
