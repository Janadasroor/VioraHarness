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

pub struct SessionStore {
    pool: Pool<SqliteConnectionManager>,
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

        const SQL001_EMBEDDED: &str = include_str!("../../../../migrations/001_init.sql");
        const SQL002_EMBEDDED: &str = include_str!("../../../../migrations/002_session_v2.sql");
        let sql001 = std::fs::read_to_string("migrations/001_init.sql")
            .or_else(|_| std::fs::read_to_string("../migrations/001_init.sql"))
            .or_else(|_| std::fs::read_to_string("../../migrations/001_init.sql"))
            .or_else(|_| std::fs::read_to_string("../../../migrations/001_init.sql"))
            .unwrap_or_else(|_| SQL001_EMBEDDED.to_string());

        let _ = conn.execute_batch(&sql001);

        let sql002 = std::fs::read_to_string("migrations/002_session_v2.sql")
            .or_else(|_| std::fs::read_to_string("../migrations/002_session_v2.sql"))
            .or_else(|_| std::fs::read_to_string("../../migrations/002_session_v2.sql"))
            .or_else(|_| std::fs::read_to_string("../../../migrations/002_session_v2.sql"))
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

            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;");
        }

        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN theme TEXT", []);
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN timestamp TEXT", []);

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

    pub fn create_session(&self, id: &str, model: &str, title: Option<&str>) -> Result<()> {
        let (cwd, project_hash) = Self::cwd_and_hash();
        self.create_session_full(
            id,
            model,
            title,
            Some(&cwd),
            Some(&project_hash),
            None,
            None,
            Self::current_theme(),
        )
    }

    fn current_theme() -> Option<String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let tui_state =
            std::path::PathBuf::from(&home).join(".local/share/vioraharness/tui_state.json");
        if let Ok(s) = std::fs::read_to_string(&tui_state) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(th) = v.get("last_theme").and_then(|x| x.as_str()) {
                    return Some(th.to_string());
                }
            }
        }
        for cand in crate::loop_mod::config_candidates() {
            if let Ok(s) = std::fs::read_to_string(&cand) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(th) = v
                        .get("tui")
                        .and_then(|x| x.get("theme"))
                        .and_then(|x| x.as_str())
                    {
                        return Some(th.to_string());
                    }
                }
            }
        }
        Some("tokyonight".into())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session_full(
        &self,
        id: &str,
        model: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        project_hash: Option<&str>,
        parent_id: Option<&str>,
        fork_seq: Option<i64>,
        theme: Option<String>,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        let theme = theme.or_else(Self::current_theme);

        tx.execute(
            "INSERT OR IGNORE INTO sessions (id, created_at, updated_at, model, status, title, cwd, project_hash, parent_id, fork_seq, model_last, theme) VALUES (?1, ?2, ?2, ?3, 'active', ?4, ?5, ?6, ?7, ?8, ?3, ?9)",
            params![id, now, model, title, cwd, project_hash, parent_id, fork_seq, theme],
        )?;

        tx.execute(
            "UPDATE sessions SET updated_at = ?2, model_last = ?3, theme = COALESCE(?4, theme) WHERE id = ?1",
            params![id, now, model, theme],
        )?;

        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let payload = serde_json::to_string(
            &serde_json::json!({"model": model, "title": title, "cwd": cwd, "parent_id": parent_id}),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at, actor) VALUES (?1, ?2, 'session.create', ?3, ?4, 'system')",
            params![id, seq, payload, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn ensure_session(&self, id: &str, model: &str, title: Option<&str>) -> Result<bool> {
        if self.get_session(id)?.is_some() {
            self.touch_session(id, Some(model))?;
            Ok(false)
        } else {
            self.create_session(id, model, title)?;
            Ok(true)
        }
    }

    pub fn get_session(&self, id: &str) -> Result<Option<StoredSession>> {
        let conn = self.pool.get()?;

        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN theme TEXT", []);
        let mut stmt = conn.prepare(
            "SELECT id, created_at, COALESCE(updated_at, created_at), model, status, title, cwd, project_hash, parent_id, archived_at, model_last, theme FROM sessions WHERE id = ?1",
        )?;
        let res = stmt
            .query_row(params![id], |r| {
                Ok(StoredSession {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    updated_at: r.get(2)?,
                    model: r.get(3)?,
                    status: r.get(4)?,
                    title: r.get(5)?,
                    cwd: r.get(6)?,
                    project_hash: r.get(7)?,
                    parent_id: r.get(8)?,
                    archived_at: r.get(9)?,
                    model_last: r.get(10)?,
                    theme: r.get::<_, Option<String>>(11)?,
                })
            })
            .optional()?;
        Ok(res)
    }

    pub fn touch_session(&self, id: &str, model_last: Option<&str>) -> Result<()> {
        let conn = self.pool.get()?;
        let now = Self::now();
        if let Some(m) = model_last {
            conn.execute(
                "UPDATE sessions SET updated_at = ?2, model_last = ?3 WHERE id = ?1",
                params![id, now, m],
            )?;
        } else {
            conn.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        }
        Ok(())
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        tx.execute(
            "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, now],
        )?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'session.rename', ?3, ?4)",
            params![id, seq, serde_json::to_string(&serde_json::json!({"title": title}))?, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn archive_session(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        tx.execute(
            "UPDATE sessions SET archived_at = ?2, status = 'archived', updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'session.archive', '{}', ?3)",
            params![id, seq, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn unarchive_session(&self, id: &str) -> Result<()> {
        let conn = self.pool.get()?;
        let now = Self::now();
        conn.execute(
            "UPDATE sessions SET archived_at = NULL, status = 'active', updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn compact_replace(
        &self,
        session_id: &str,
        cutoff_seq: i64,
        summary: &str,
    ) -> Result<(usize, usize)> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let dropped: i64 = tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND seq < ?2",
            params![session_id, cutoff_seq],
            |r| r.get(0),
        )?;
        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1 AND seq < ?2",
            params![session_id, cutoff_seq],
        )?;
        tx.execute(
            "DELETE FROM tool_calls WHERE session_id = ?1 AND message_seq < ?2",
            params![session_id, cutoff_seq],
        )?;
        let now = Self::now();
        let summary_seq = cutoff_seq - 1;
        tx.execute(
            "INSERT INTO messages (session_id, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, is_compaction) VALUES (?1, ?2, 'user', ?3, NULL, NULL, NULL, NULL, ?4, 1)",
            params![session_id, summary_seq, summary, now],
        )?;
        let kept: i64 = tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND seq >= ?2",
            params![session_id, cutoff_seq],
            |r| r.get(0),
        )?;
        let ev_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        let payload = serde_json::to_string(
            &serde_json::json!({"dropped": dropped, "kept": kept, "summary_seq": summary_seq}),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'session.compact', ?3, ?4)",
            params![session_id, ev_seq, payload, now],
        )?;
        tx.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
            params![session_id, now],
        )?;
        tx.commit()?;
        Ok((dropped as usize, kept as usize))
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;

        let _ = conn.execute("DELETE FROM todos WHERE session_id = ?1", params![id]);
        Ok(())
    }

    pub fn fork_session(&self, parent_id: &str, new_id: &str, at_seq: Option<i64>) -> Result<()> {
        let parent = self
            .get_session(parent_id)?
            .ok_or_else(|| anyhow::anyhow!("parent session {parent_id} not found"))?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        let title = parent.title.map(|t| format!("{t} (fork)")).or(Some(format!(
            "fork of {}",
            &parent_id[..8.min(parent_id.len())]
        )));
        tx.execute(
            "INSERT OR IGNORE INTO sessions (id, created_at, updated_at, model, status, title, cwd, project_hash, parent_id, fork_seq, model_last, theme) VALUES (?1, ?2, ?2, ?3, 'active', ?4, ?5, ?6, ?7, ?8, ?3, ?9)",
            params![new_id, now, parent.model, title, parent.cwd, parent.project_hash, parent_id, at_seq, parent.theme],
        )?;

        if let Some(seq) = at_seq {
            tx.execute(
                "INSERT INTO messages (session_id, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, is_compaction) SELECT ?1, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, is_compaction FROM messages WHERE session_id = ?2 AND seq <= ?3 ORDER BY seq",
                params![new_id, parent_id, seq],
            )?;

            tx.execute(
                "INSERT OR IGNORE INTO tool_calls (id, session_id, message_seq, name, args, result, status, created_at, settled_at) SELECT id, ?1, message_seq, name, args, result, status, created_at, settled_at FROM tool_calls WHERE session_id = ?2 AND message_seq <= ?3",
                params![new_id, parent_id, seq],
            )?;
        } else {
            tx.execute(
                "INSERT INTO messages (session_id, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, is_compaction) SELECT ?1, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, is_compaction FROM messages WHERE session_id = ?2 ORDER BY seq",
                params![new_id, parent_id],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO tool_calls (id, session_id, message_seq, name, args, result, status, created_at, settled_at) SELECT id, ?1, message_seq, name, args, result, status, created_at, settled_at FROM tool_calls WHERE session_id = ?2",
                params![new_id, parent_id],
            )?;
        }

        let _ = tx.execute(
            "INSERT OR IGNORE INTO todos (session_id, content, status, priority, updated_at) SELECT ?1, content, status, priority, updated_at FROM todos WHERE session_id = ?2",
            params![new_id, parent_id],
        );

        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at, actor) SELECT ?1, seq, type, payload, created_at, actor FROM events WHERE session_id = ?2 ORDER BY seq",
            params![new_id, parent_id],
        )?;

        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![new_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'session.fork', ?3, ?4)",
            params![
                new_id,
                next_seq,
                serde_json::to_string(&serde_json::json!({"parent_id": parent_id, "at_seq": at_seq}))?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_message(&self, session_id: &str, role: &str, content: &str) -> Result<i64> {
        self.append_message_full(session_id, role, content, None, None, None, None)
    }

    fn timestamp_now() -> String {
        std::process::Command::new("date")
            .arg("+%H:%M")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| {
                let now = Self::now();
                format!("{:02}:{:02}", (now % 86400 / 3600) % 24, (now % 3600) / 60)
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_message_full(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        content_json: Option<&str>,
        reasoning: Option<&str>,
        model: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> Result<i64> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        let now = Self::now();
        let ts = Self::timestamp_now();
        let _ = tx.execute("ALTER TABLE messages ADD COLUMN timestamp TEXT", []);
        tx.execute(
            "INSERT INTO messages (session_id, seq, role, content, content_json, reasoning, model, tool_call_id, created_at, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![session_id, seq, role, content, content_json, reasoning, model, tool_call_id, now, ts],
        )?;

        let ev_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        let payload = serde_json::to_string(
            &serde_json::json!({"seq": seq, "role": role, "content": content, "reasoning": reasoning}),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'message.append', ?3, ?4)",
            params![session_id, ev_seq, payload, now],
        )?;

        let tokens = (content.len() / 4) as i64;
        tx.execute(
            "UPDATE sessions SET updated_at = ?2, tokens_out = tokens_out + ?3, model_last = COALESCE(?4, model_last) WHERE id = ?1",
            params![session_id, now, tokens, model],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    pub fn record_tool_call(
        &self,
        id: &str,
        session_id: &str,
        message_seq: i64,
        name: &str,
        args: &Value,
    ) -> Result<()> {
        self.record_tool_call_with_sig(id, session_id, message_seq, name, args, None)
    }

    pub fn record_tool_call_with_sig(
        &self,
        id: &str,
        session_id: &str,
        message_seq: i64,
        name: &str,
        args: &Value,
        thought_signature: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();

        let _ = tx.execute(
            "ALTER TABLE tool_calls ADD COLUMN thought_signature TEXT",
            [],
        );
        tx.execute(
            "INSERT INTO tool_calls (id, session_id, message_seq, name, args, status, created_at, thought_signature) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
            params![id, session_id, message_seq, name, args.to_string(), now, thought_signature],
        )?;
        let ev_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'tool_call.pending', ?3, ?4)",
            params![session_id, ev_seq, serde_json::to_string(&serde_json::json!({"id": id, "name": name}))?, now],
        )?;
        tx.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
            params![session_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn settle_tool_call(&self, id: &str, result: &Value) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        let session_id: Option<String> = tx
            .query_row(
                "SELECT session_id FROM tool_calls WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "UPDATE tool_calls SET result = ?1, status = 'settled', settled_at = ?3 WHERE id = ?2",
            params![result.to_string(), id, now],
        )?;
        if let Some(sid) = session_id {
            let ev_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
                params![sid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'tool_call.settled', ?3, ?4)",
                params![sid, ev_seq, serde_json::to_string(&serde_json::json!({"id": id}))?, now],
            )?;
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![sid, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn fail_tool_call(&self, id: &str, error: &str) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        let session_id: Option<String> = tx
            .query_row(
                "SELECT session_id FROM tool_calls WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "UPDATE tool_calls SET error = ?1, status = 'error', settled_at = ?3 WHERE id = ?2",
            params![error, id, now],
        )?;
        if let Some(sid) = session_id {
            let ev_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
                params![sid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO events (session_id, seq, type, payload, created_at) VALUES (?1, ?2, 'tool_call.error', ?3, ?4)",
                params![sid, ev_seq, serde_json::to_string(&serde_json::json!({"id": id, "error": error}))?, now],
            )?;
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![sid, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<StoredSession>> {
        self.list_sessions_filtered(None, None, false, 50, 0)
    }

    pub fn latest_for_cwd(&self, cwd: &str) -> Result<Option<StoredSession>> {
        fn under(a: &str, b: &str) -> bool {
            a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
        }
        let cands = self.list_sessions_filtered(None, None, false, 25, 0)?;
        let mut best: Option<(u8, StoredSession)> = None;
        for s in cands {
            let scwd = s.cwd.as_deref().unwrap_or("");
            let tier = if scwd == cwd {
                0
            } else if under(scwd, cwd) {
                1
            } else if s.project_hash.as_deref() == Some(&format!("{:x}", md5ish(cwd))) {
                2
            } else {
                continue;
            };

            let has_msgs = self
                .get_messages(&s.id)
                .map(|m| !m.is_empty())
                .unwrap_or(false);
            if !has_msgs {
                continue;
            }
            let replace = match &best {
                None => true,
                Some((bt, bs)) => tier < *bt || (tier == *bt && s.updated_at > bs.updated_at),
            };
            if replace {
                best = Some((tier, s));
            }
        }
        Ok(best.map(|(_, s)| s))
    }

    pub fn list_sessions_filtered(
        &self,
        project_filter: Option<&str>,
        search: Option<&str>,
        include_archived: bool,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<StoredSession>> {
        let conn = self.pool.get()?;
        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN theme TEXT", []);

        let mut sql = String::from(
            "SELECT id, created_at, COALESCE(updated_at, created_at), model, status, title, cwd, project_hash, parent_id, archived_at, model_last, theme FROM sessions WHERE 1=1",
        );
        if !include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        if project_filter.is_some() {
            sql.push_str(" AND project_hash = ?");
        }
        if search.is_some() {
            sql.push_str(" AND (title LIKE ? OR id LIKE ?)");
        }
        sql.push_str(" ORDER BY COALESCE(updated_at, created_at) DESC LIMIT ? OFFSET ?");
        let mut stmt = conn.prepare(&sql)?;
        let search_like = search.map(|s| format!("%{s}%"));
        let mut rows: Vec<StoredSession> = Vec::new();

        let project = project_filter;
        let sl = search_like.as_deref();
        let mapper = |r: &rusqlite::Row| {
            Ok(StoredSession {
                id: r.get(0)?,
                created_at: r.get(1)?,
                updated_at: r.get(2)?,
                model: r.get(3)?,
                status: r.get(4)?,
                title: r.get(5)?,
                cwd: r.get(6)?,
                project_hash: r.get(7)?,
                parent_id: r.get(8)?,
                archived_at: r.get(9)?,
                model_last: r.get(10)?,
                theme: r.get::<_, Option<String>>(11)?,
            })
        };
        let iter: Box<dyn Iterator<Item = rusqlite::Result<StoredSession>>> = match (project, sl) {
            (Some(p), Some(s)) => {
                Box::new(stmt.query_map(params![p, s, s, limit as i64, offset as i64], mapper)?)
            }
            (Some(p), None) => {
                Box::new(stmt.query_map(params![p, limit as i64, offset as i64], mapper)?)
            }
            (None, Some(s)) => {
                Box::new(stmt.query_map(params![s, s, limit as i64, offset as i64], mapper)?)
            }
            (None, None) => Box::new(stmt.query_map(params![limit as i64, offset as i64], mapper)?),
        };
        for r in iter {
            rows.push(r?);
        }
        Ok(rows)
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<(i64, String, String)>> {
        let msgs = self.get_messages_detailed(session_id)?;
        Ok(msgs
            .into_iter()
            .map(|m| (m.seq, m.role, m.content))
            .collect())
    }

    pub fn get_messages_detailed(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.pool.get()?;
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN timestamp TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN is_compaction INTEGER DEFAULT 0",
            [],
        );
        let mut stmt = conn.prepare(
            "SELECT seq, role, content, content_json, reasoning, model, tool_call_id, created_at, timestamp, is_compaction FROM messages WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(StoredMessage {
                seq: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                content_json: r.get(3)?,
                reasoning: r.get(4)?,
                model: r.get(5)?,
                tool_call_id: r.get(6)?,
                created_at: r.get(7)?,
                timestamp: r.get::<_, Option<String>>(8)?,
                is_compaction: r.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    #[allow(clippy::type_complexity)]
    pub fn get_tool_calls_grouped(&self, session_id: &str) -> Result<ToolCallGrouped> {
        let conn = self.pool.get()?;

        let _ = conn.execute(
            "ALTER TABLE tool_calls ADD COLUMN thought_signature TEXT",
            [],
        );
        let _ = conn.execute("ALTER TABLE tool_calls ADD COLUMN result TEXT", []);
        let mut stmt = conn.prepare("SELECT message_seq, id, name, args, thought_signature, result FROM tool_calls WHERE session_id = ?1 ORDER BY message_seq ASC")?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut map: ToolCallGrouped = std::collections::HashMap::new();
        for r in rows {
            let (seq, id, name, args, sig, result) = r?;
            map.entry(seq)
                .or_default()
                .push((id, name, args, sig, result));
        }
        Ok(map)
    }

    #[allow(clippy::type_complexity)]
    pub fn get_tool_calls_grouped_simple(
        &self,
        session_id: &str,
    ) -> Result<std::collections::HashMap<i64, Vec<(String, String, String, Option<String>)>>> {
        let full = self.get_tool_calls_grouped(session_id)?;
        let mut simple = std::collections::HashMap::new();
        for (seq, vec) in full {
            let s: Vec<(String, String, String, Option<String>)> = vec
                .into_iter()
                .map(|(id, name, args, sig, _res)| (id, name, args, sig))
                .collect();
            simple.insert(seq, s);
        }
        Ok(simple)
    }

    pub fn get_events(
        &self,
        session_id: &str,
        since_seq: Option<i64>,
    ) -> Result<Vec<(i64, String, String)>> {
        let conn = self.pool.get()?;
        let (sql, params_seq): (String, Option<i64>) = match since_seq {
            Some(s) => (
                "SELECT seq, type, payload FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC".into(),
                Some(s),
            ),
            None => (
                "SELECT seq, type, payload FROM events WHERE session_id = ?1 ORDER BY seq ASC".into(),
                None,
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<(i64, String, String)> = match params_seq {
            Some(s) => {
                let iter = stmt.query_map(params![session_id, s], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
                iter.collect::<rusqlite::Result<Vec<_>>>()?
            }
            None => {
                let iter = stmt.query_map(params![session_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
                iter.collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    pub fn count_sessions(&self, include_archived: bool) -> Result<i64> {
        let conn = self.pool.get()?;
        let sql = if include_archived {
            "SELECT COUNT(*) FROM sessions"
        } else {
            "SELECT COUNT(*) FROM sessions WHERE archived_at IS NULL"
        };
        Ok(conn.query_row(sql, [], |r| r.get(0))?)
    }

    pub fn insert_snapshot(
        &self,
        session_id: &str,
        seq: i64,
        path: &str,
        sha: &str,
        content: &[u8],
    ) -> Result<()> {
        let conn = self.pool.get()?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO snapshots (session_id, message_seq, path, sha, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, seq, path, sha, content, now],
        )?;
        // Bound per-session growth (#11): keep newest MAX_SNAPSHOTS_PER_SESSION
        // rows; prune oldest first, including their on-disk copies.
        let stale: Vec<(i64, i64)> = conn
            .prepare(
                "SELECT rowid, message_seq FROM snapshots WHERE session_id = ?1 ORDER BY rowid DESC LIMIT -1 OFFSET ?2",
            )
            .map(|mut stmt| {
                stmt.query_map(params![session_id, MAX_SNAPSHOTS_PER_SESSION], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
            })
            .unwrap_or_default();
        for (rowid, old_seq) in stale {
            let _ = conn.execute("DELETE FROM snapshots WHERE rowid = ?1", params![rowid]);
            let dir =
                std::path::PathBuf::from(format!(".vioraharness/snapshots/{session_id}/{old_seq}"));
            let _ = std::fs::remove_dir_all(dir);
        }
        Ok(())
    }

    /// Replace the session's todo list wholesale (last-write-wins).
    pub fn set_todos(
        &self,
        session_id: &str,
        items: &[(String, String, Option<String>)],
    ) -> Result<usize> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let _ = tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS todos (
                session_id TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                priority TEXT,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (session_id, content)
            );",
        );
        tx.execute(
            "DELETE FROM todos WHERE session_id = ?1",
            params![session_id],
        )?;
        let now = Self::now();
        let mut n = 0;
        for (content, status, priority) in items {
            tx.execute(
                "INSERT INTO todos (session_id, content, status, priority, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session_id, content, status, priority, now],
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn get_todos(&self, session_id: &str) -> Result<Vec<(String, String, Option<String>)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT content, status, priority FROM todos WHERE session_id = ?1 ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn pop_snapshot(&self, session_id: &str) -> Result<Option<(String, Vec<u8>)>> {
        let conn = self.pool.get()?;
        let row: Option<(i64, String, Vec<u8>)> = conn
            .query_row(
                "SELECT rowid, path, content FROM snapshots WHERE session_id = ?1 ORDER BY rowid DESC LIMIT 1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((rowid, path, content)) = row {
            conn.execute("DELETE FROM snapshots WHERE rowid = ?1", params![rowid])?;
            return Ok(Some((path, content)));
        }
        Ok(None)
    }

    pub fn get_snapshots(&self, session_id: &str) -> Result<Vec<(i64, String, String)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT message_seq, path, sha FROM snapshots WHERE session_id = ?1 ORDER BY message_seq DESC")?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_snapshot_contents(&self, session_id: &str) -> Result<Vec<(i64, String, Vec<u8>)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT message_seq, path, content FROM snapshots WHERE session_id = ?1 ORDER BY message_seq ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Delete conversation rows after `target_seq` (rewind). Returns
    /// (messages dropped, tool calls dropped). Events are kept, matching
    /// `compact_replace` precedent (audit trail, never read for display).
    pub fn delete_messages_after(
        &self,
        session_id: &str,
        target_seq: i64,
    ) -> Result<(usize, usize)> {
        let conn = self.pool.get()?;
        let msgs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND seq > ?2",
            params![session_id, target_seq],
            |r| r.get(0),
        )?;
        conn.execute(
            "DELETE FROM messages WHERE session_id = ?1 AND seq > ?2",
            params![session_id, target_seq],
        )?;
        let calls: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tool_calls WHERE session_id = ?1 AND message_seq > ?2",
            params![session_id, target_seq],
            |r| r.get(0),
        )?;
        conn.execute(
            "DELETE FROM tool_calls WHERE session_id = ?1 AND message_seq > ?2",
            params![session_id, target_seq],
        )?;
        Ok((msgs as usize, calls as usize))
    }

    pub fn delete_snapshots_after(&self, session_id: &str, target_seq: i64) -> Result<usize> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "DELETE FROM snapshots WHERE session_id = ?1 AND message_seq > ?2",
            params![session_id, target_seq],
        )?;
        Ok(n)
    }

    pub fn snapshot_session_ids(&self) -> Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM snapshots")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_all_snapshots(&self, session_id: &str) -> Result<usize> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "DELETE FROM snapshots WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(n)
    }

    pub fn export_jsonl(&self, session_id: &str) -> Result<String> {
        let sess = self
            .get_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
        let msgs = self.get_messages_detailed(session_id)?;
        let mut out = String::new();
        out.push_str(&serde_json::to_string(&serde_json::json!({
            "type": "session",
            "id": sess.id,
            "created_at": sess.created_at,
            "updated_at": sess.updated_at,
            "model": sess.model,
            "title": sess.title,
            "cwd": sess.cwd,
            "parent_id": sess.parent_id,
        }))?);
        out.push('\n');
        for m in msgs {
            out.push_str(&serde_json::to_string(&serde_json::json!({
                "type": "message",
                "seq": m.seq,
                "role": m.role,
                "content": m.content,
                "reasoning": m.reasoning,
                "model": m.model,
                "tool_call_id": m.tool_call_id,
                "created_at": m.created_at,
            }))?);
            out.push('\n');
        }
        // Todos are part of the auditable session state (#13).
        for (content, status, priority) in self.get_todos(session_id).unwrap_or_default() {
            out.push_str(&serde_json::to_string(&serde_json::json!({
                "type": "todo",
                "content": content,
                "status": status,
                "priority": priority,
            }))?);
            out.push('\n');
        }
        Ok(out)
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

fn md5ish(s: &str) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
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
        s.settle_tool_call("c1", &serde_json::json!({"ok": true}))
            .unwrap();
        s.fail_tool_call("c2", "boom").unwrap();
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
        // NOTE: tool_calls.id is a global PRIMARY KEY, so the fork's
        // INSERT OR IGNORE silently drops copied calls whose ids exist
        // in the parent. Flagged for phase 2 (fork loses tool history).
        assert!(
            s.get_tool_calls_grouped_simple("f").unwrap().is_empty(),
            "documents current fork tool-call drop"
        );
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
