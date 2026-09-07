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
    let base = store
        .get_session(session_id)?
        .and_then(|s| s.cwd)
        .filter(|c| !c.trim().is_empty())
        .map(PathBuf::from);
    let target = PathBuf::from(&path);
    let target = match (target.is_absolute(), base) {
        (true, _) => target,
        (false, Some(b)) => b.join(target),
        (false, None) => target,
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &content)?;
    Ok(Some(target.to_string_lossy().to_string()))
}
