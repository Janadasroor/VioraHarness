use super::*;

impl SessionStore {
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
        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN mode TEXT", []);

        let mut sql = String::from(
            "SELECT id, created_at, COALESCE(updated_at, created_at), model, status, title, cwd, project_hash, parent_id, archived_at, model_last, theme, mode FROM sessions WHERE 1=1",
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
                mode: r.get::<_, Option<String>>(12).unwrap_or(None),
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
}
