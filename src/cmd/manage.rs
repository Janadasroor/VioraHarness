pub(crate) fn cmd_gc(days: u64, dry_run: bool) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let snapshots_root = cwd.join(".vioraharness/snapshots");
    let max_idle = std::time::Duration::from_secs(days.saturating_mul(24 * 3600));
    let rep =
        vioraharness_core::session::gc::gc_snapshots(&store, &snapshots_root, max_idle, dry_run)?;
    if dry_run {
        println!(
            "gc --dry-run (idle > {days}d, root {})",
            snapshots_root.display()
        );
    } else {
        println!("gc (idle > {days}d, root {})", snapshots_root.display());
    }
    println!(
        "  sessions pruned: {}{}",
        rep.snapshot_sessions_pruned.len(),
        if rep.snapshot_sessions_pruned.is_empty() {
            String::new()
        } else {
            format!(" ({})", rep.snapshot_sessions_pruned.join(", "))
        }
    );
    println!("  snapshot rows: {}", rep.snapshot_rows_deleted);
    println!(
        "  snapshot dirs: {} ({:.1} KiB freed)",
        rep.snapshot_dirs_removed,
        rep.bytes_freed as f64 / 1024.0
    );
    if dry_run {
        println!("  (nothing removed — rerun without --dry-run to apply)");
    }
    Ok(())
}

pub(crate) fn cmd_sessions(all: bool, search: Option<String>, limit: usize) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let sessions = store.list_sessions_filtered(None, search.as_deref(), all, limit, 0)?;
    if sessions.is_empty() {
        println!("No chats yet. Run `vioraharness run \"hello\"` or TUI /new to create one.");
    } else {
        println!(
            "Chats (sorted by updated_at) — {} total (showing {}):",
            sessions.len(),
            sessions.len().min(limit)
        );
        for s in sessions {
            let upd = {
                let age = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    - s.updated_at)
                    .max(0);
                if age < 60 {
                    format!("{}s ago", age)
                } else if age < 3600 {
                    format!("{}m ago", age / 60)
                } else if age < 86400 {
                    format!("{}h ago", age / 3600)
                } else {
                    format!("{}d ago", age / 86400)
                }
            };
            let parent = s
                .parent_id
                .as_deref()
                .map(|p| format!(" fork:{}", &p[..8.min(p.len())]))
                .unwrap_or_default();
            let arch = if s.archived_at.is_some() {
                " [archived]"
            } else {
                ""
            };
            println!(
                "{} | {} | {} | {}{} | upd:{} | {}",
                &s.id[..8.min(s.id.len())],
                s.model,
                s.status,
                s.title.clone().unwrap_or_else(|| "(untitled)".into()),
                parent,
                upd,
                arch,
            );
            println!(
                "  id={} updated_at={} created_at={}",
                s.id, s.updated_at, s.created_at
            );
        }
    }
    Ok(())
}

pub(crate) fn cmd_resume(session: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    if session.trim().is_empty() {
        let sessions = store.list_sessions_filtered(None, None, false, 10, 0)?;
        if sessions.is_empty() {
            println!("No chats yet. Run `vioraharness tui` or `vioraharness run \"hello\"`");
        } else {
            println!("Recent chats (use `vioraharness resume <id>`):");
            for s in sessions {
                println!(
                    "  {}  {}  {}  {}",
                    &s.id[..8.min(s.id.len())],
                    s.model,
                    s.title.clone().unwrap_or_else(|| "(untitled)".into()),
                    s.id
                );
            }
            println!(
                "\nTo resume: vioraharness resume <id>  or  vioraharness tui  then /resume <id>"
            );
        }
        return Ok(());
    }

    let id = if let Ok(Some(s)) = store.get_session(&session) {
        s.id
    } else {
        let list = store.list_sessions_filtered(None, Some(&session), true, 10, 0)?;
        list.into_iter()
            .find(|s| s.id.starts_with(&session))
            .map(|s| s.id)
            .unwrap_or(session.clone())
    };
    let sess = store.get_session(&id)?.ok_or_else(|| {
        anyhow::anyhow!("session {id} not found — try `vioraharness sessions` to list")
    })?;
    println!(
        "Resumed chat {} — model {} title {:?} updated_at {}",
        sess.id, sess.model, sess.title, sess.updated_at
    );
    println!(
        "To continue in TUI: vioraharness tui  then /resume {}",
        sess.id
    );
    println!(
        "To continue headless: vioraharness run \"your prompt\" --session {}",
        sess.id
    );
    let msgs = store.get_messages_detailed(&id)?;
    for m in msgs {
        println!(
            "[{}] {}: {}",
            m.seq,
            m.role,
            m.content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>()
        );
        if let Some(r) = m.reasoning {
            if !r.is_empty() {
                println!("  reasoning: {} chars", r.len());
            }
        }
    }
    Ok(())
}

pub(crate) fn cmd_fork(session: String, at: Option<i64>) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let new_id = format!(
        "{:010x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            & 0xffffffffff
    );
    store.fork_session(&session, &new_id, at)?;
    println!("Forked {} at seq {:?} → {}", session, at, new_id);
    Ok(())
}

pub(crate) fn cmd_rename(session: String, title: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    store.rename_session(&session, &title)?;
    println!("Renamed {} → '{}'", session, title);
    Ok(())
}

pub(crate) fn cmd_archive(session: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    store.archive_session(&session)?;
    println!("Archived {}", session);
    Ok(())
}

pub(crate) fn cmd_delete(session: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    store.delete_session(&session)?;
    println!("Deleted {} (CASCADE)", session);
    Ok(())
}

pub(crate) fn cmd_export(session: String, output: Option<String>) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let jsonl = store.export_jsonl(&session)?;
    if let Some(path) = output {
        std::fs::write(&path, &jsonl)?;
        println!(
            "Exported {} → {} ({} bytes, {} lines)",
            session,
            path,
            jsonl.len(),
            jsonl.lines().count()
        );
    } else {
        print!("{}", jsonl);
    }
    Ok(())
}

pub(crate) fn cmd_history(session: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let sid = if session.trim().is_empty() {
        let last_path = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            })
            .join("vioraharness/last_session");
        if let Ok(last) = std::fs::read_to_string(&last_path) {
            last.trim().to_string()
        } else {
            let recents = store.list_sessions_filtered(None, None, false, 1, 0)?;
            recents
                .into_iter()
                .next()
                .map(|s| s.id)
                .unwrap_or_else(|| "".into())
        }
    } else {
        session.clone()
    };
    if sid.is_empty() {
        anyhow::bail!("no session id and no last session — try `vioraharness sessions`");
    }
    let id = if let Ok(Some(s)) = store.get_session(&sid) {
        s.id
    } else {
        let list = store.list_sessions_filtered(None, Some(&sid), true, 10, 0)?;
        list.into_iter()
            .find(|s| s.id.starts_with(&sid))
            .map(|s| s.id)
            .unwrap_or(sid.clone())
    };
    let msgs = store.get_messages_detailed(&id)?;
    println!("History for {} ({} msgs):", id, msgs.len());
    for m in msgs {
        println!(
            "seq={} role={} model={:?} reasoning={} chars\n{}",
            m.seq,
            m.role,
            m.model,
            m.reasoning.as_deref().map(|r| r.len()).unwrap_or(0),
            m.content
        );
        println!("---");
    }
    println!("\nTo resume: vioraharness resume {}", id);
    Ok(())
}

pub(crate) fn cmd_undo(session: String) -> anyhow::Result<()> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let sid = if session.trim().is_empty() {
        let last_path = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            })
            .join("vioraharness/last_session");
        if let Ok(last) = std::fs::read_to_string(&last_path) {
            last.trim().to_string()
        } else {
            store
                .list_sessions_filtered(None, None, false, 1, 0)?
                .into_iter()
                .next()
                .map(|s| s.id)
                .unwrap_or_default()
        }
    } else if let Ok(Some(s)) = store.get_session(session.trim()) {
        s.id
    } else {
        store
            .list_sessions_filtered(None, None, true, 50, 0)?
            .into_iter()
            .find(|s| s.id.starts_with(session.trim()))
            .map(|s| s.id)
            .unwrap_or_default()
    };
    if sid.is_empty() {
        anyhow::bail!("no such session — try `vioraharness sessions`");
    }
    match vioraharness_core::session::snapshot::restore_latest(&store, &sid)? {
        Some(path) => {
            println!("undo: restored {path} — repeat to go further back");
        }
        None => println!("undo: nothing to undo in chat {}", &sid[..8.min(sid.len())]),
    }
    Ok(())
}

pub(crate) fn cmd_last_session() -> anyhow::Result<()> {
    let last_path = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        })
        .join("vioraharness/last_session");
    if let Ok(sid) = std::fs::read_to_string(&last_path) {
        let sid = sid.trim().to_string();
        if sid.is_empty() {
            println!("No last session yet — run TUI first");
        } else {
            println!("{}", sid);
            println!("\nTo resume: vioraharness resume {}", sid);
            println!("Short: vioraharness resume {}", &sid[..8.min(sid.len())]);
        }
    } else {
        println!("No last session file — try `vioraharness sessions`");
    }
    Ok(())
}
