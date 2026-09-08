use super::*;

impl SessionStore {
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
}
