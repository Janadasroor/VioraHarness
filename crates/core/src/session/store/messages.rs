use super::*;

impl SessionStore {
    pub fn append_message(&self, session_id: &str, role: &str, content: &str) -> Result<i64> {
        self.append_message_full(session_id, role, content, None, None, None, None)
    }

    /// Highest message seq in the session (0 when empty). Snapshot writers
    /// must use this — never an in-memory vec length — so snapshot
    /// `message_seq` values line up with what rewind compares against.
    pub fn latest_seq(&self, session_id: &str) -> Result<i64> {
        let conn = self.pool.get()?;
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?)
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

    pub fn settle_tool_call(&self, session_id: &str, id: &str, result: &Value) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        tx.execute(
            "UPDATE tool_calls SET result = ?1, status = 'settled', settled_at = ?3 WHERE session_id = ?4 AND id = ?2",
            params![result.to_string(), id, now, session_id],
        )?;
        let session_known: bool = tx
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![session_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if session_known {
            let sid = session_id;
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

    pub fn fail_tool_call(&self, session_id: &str, id: &str, error: &str) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = Self::now();
        tx.execute(
            "UPDATE tool_calls SET error = ?1, status = 'error', settled_at = ?3 WHERE session_id = ?4 AND id = ?2",
            params![error, id, now, session_id],
        )?;
        let session_known: bool = tx
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![session_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if session_known {
            let sid = session_id;
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
}
