-- 002_session_v2 — Codex/opencode-grade session lifecycle
-- Event-sourced V2 deltas, WAL crash safety, per-message reasoning, fork/archive
-- Idempotent: ADD COLUMN IF NOT EXISTS emulated via pragma check not needed — SQLite ignores duplicate? We use ALTER TABLE ADD COLUMN which errors if exists, so guard with careful migration runner that ignores duplicate column errors via store.rs logic. This file is executed via execute_batch which will error on second run if not guarded; store.rs handles by trying and ignoring.

PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA foreign_keys=ON;
PRAGMA busy_timeout=5000;

-- sessions: add lifecycle columns (NOT NULL with non-constant default fails on existing rows, so add nullable then backfill)
ALTER TABLE sessions ADD COLUMN updated_at INTEGER;
ALTER TABLE sessions ADD COLUMN cwd TEXT;
ALTER TABLE sessions ADD COLUMN project_hash TEXT;
ALTER TABLE sessions ADD COLUMN parent_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
ALTER TABLE sessions ADD COLUMN archived_at INTEGER;
ALTER TABLE sessions ADD COLUMN fork_seq INTEGER;
ALTER TABLE sessions ADD COLUMN model_last TEXT;
ALTER TABLE sessions ADD COLUMN tokens_in INTEGER DEFAULT 0;
ALTER TABLE sessions ADD COLUMN tokens_out INTEGER DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_hash, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id);

-- messages: per-message metadata (reasoning, model, multimodal)
ALTER TABLE messages ADD COLUMN model TEXT;
ALTER TABLE messages ADD COLUMN tokens INTEGER DEFAULT 0;
ALTER TABLE messages ADD COLUMN reasoning TEXT;
ALTER TABLE messages ADD COLUMN content_json TEXT;
ALTER TABLE messages ADD COLUMN tool_call_id TEXT;
ALTER TABLE messages ADD COLUMN is_compaction INTEGER DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_messages_tool ON messages(tool_call_id) WHERE tool_call_id IS NOT NULL;

-- tool_calls: timestamps, error (avoid non-constant default for ADD COLUMN on populated tables)
ALTER TABLE tool_calls ADD COLUMN created_at INTEGER;
ALTER TABLE tool_calls ADD COLUMN settled_at INTEGER;
ALTER TABLE tool_calls ADD COLUMN error TEXT;

CREATE INDEX IF NOT EXISTS idx_tool_calls_session_seq ON tool_calls(session_id, message_seq);
CREATE INDEX IF NOT EXISTS idx_tool_calls_status ON tool_calls(status) WHERE status='pending';

-- events: per-session seq for strong ordering
ALTER TABLE events ADD COLUMN seq INTEGER;
ALTER TABLE events ADD COLUMN actor TEXT DEFAULT 'system';

-- Backfill seq per session where NULL (monotonic by id)
-- This is done in Rust migration runner for safety; keep here as optional no-op for fresh DBs where seq NULL will be filled on insert.

CREATE UNIQUE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_session_created ON events(session_id, created_at);

-- compactions audit
CREATE TABLE IF NOT EXISTS compactions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  from_seq INTEGER NOT NULL,
  to_seq INTEGER NOT NULL,
  summary TEXT NOT NULL,
  tokens_before INTEGER NOT NULL,
  tokens_after INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_compactions_session ON compactions(session_id, created_at);
