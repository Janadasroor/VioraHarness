// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use std::sync::{Arc, Mutex};

/// Inbound prompt handle for a live agent turn (`$` instant prompts).
///
/// The TUI clones this out of its `AgentLoop` before spawning the turn task
/// and pushes user text into it while the turn runs. The loop drains it at
/// turn boundaries (top of each iteration + the no-tool-call branch) so an
/// urgent prompt is picked up after the current thinking or tool call —
/// without waiting for the whole response to finish.
///
/// Server-spawned loops never share their handle, so this is inert there.
#[derive(Debug, Clone, Default)]
pub struct Injector {
    inner: Arc<Mutex<Vec<String>>>,
}

impl Injector {
    pub fn push(&self, text: String) {
        if let Ok(mut q) = self.inner.lock() {
            q.push(text);
        }
    }

    /// Take all pending prompts, oldest first. Never blocks; a poisoned
    /// mutex yields whatever is there and clears it.
    pub fn drain(&self) -> Vec<String> {
        match self.inner.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_drain_len_roundtrip_in_order() {
        let inj = Injector::default();
        assert!(inj.is_empty());
        inj.push("first".into());
        inj.push("second".into());
        assert_eq!(inj.len(), 2);
        assert_eq!(inj.drain(), vec!["first".to_string(), "second".to_string()]);
        assert!(inj.is_empty());
        assert!(inj.drain().is_empty());
    }

    #[test]
    fn clones_share_one_backlog() {
        let inj = Injector::default();
        let other = inj.clone();
        other.push("x".into());
        assert_eq!(inj.len(), 1);
        assert_eq!(inj.drain(), vec!["x".to_string()]);
        assert!(other.is_empty());
    }
}
