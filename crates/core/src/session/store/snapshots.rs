// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl SessionStore {
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
