use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub enum HunkLine {
    Context(String),
    Del(String),
    Add(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileOp {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, hunks: Vec<Hunk> },
    Move { from: String, to: String },
}

pub fn parse_patch(text: &str) -> Result<Vec<FileOp>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut ops: Vec<FileOp> = Vec::new();

    enum Block {
        Add {
            path: String,
            body: Vec<String>,
        },
        Update {
            path: String,
            hunks: Vec<Hunk>,
            move_to: Option<String>,
        },
    }
    let mut block: Option<Block> = None;
    let mut cur_hunk: Vec<HunkLine> = Vec::new();
    let mut in_hunk = false;
    let mut begun = false;
    let mut ended = false;

    let flush_hunk = |block: &mut Option<Block>, cur_hunk: &mut Vec<HunkLine>| {
        if !cur_hunk.is_empty() {
            if let Some(Block::Update { hunks, .. }) = block.as_mut() {
                hunks.push(Hunk {
                    lines: std::mem::take(cur_hunk),
                });
            }
        }
    };

    let flush_block = |block: &mut Option<Block>,
                       cur_hunk: &mut Vec<HunkLine>,
                       ops: &mut Vec<FileOp>|
     -> Result<(), String> {
        flush_hunk(block, cur_hunk);
        match block.take() {
            None => Ok(()),
            Some(Block::Add { path, body }) => {
                if path.trim().is_empty() {
                    return Err("*** Add File: missing path".into());
                }
                let mut content = body.join("\n");
                if !body.is_empty() {
                    content.push('\n');
                }
                ops.push(FileOp::Add { path, content });
                Ok(())
            }
            Some(Block::Update {
                path,
                hunks,
                move_to,
            }) => {
                if path.trim().is_empty() {
                    return Err("*** Update File: missing path".into());
                }
                if hunks.is_empty() && move_to.is_none() {
                    return Err(format!(
                        "*** Update File: {path} has no @@ hunks (and no *** Move to:)"
                    ));
                }
                ops.push(FileOp::Update { path, hunks });
                if let Some(to) = move_to {
                    if to.trim().is_empty() {
                        return Err("*** Move to: missing path".into());
                    }

                    ops.push(FileOp::Move {
                        from: String::new(),
                        to,
                    });
                }
                Ok(())
            }
        }
    };

    for (idx, raw) in lines.iter().enumerate() {
        let lineno = idx + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("*** Begin Patch") {
            begun = true;
            continue;
        }
        if line.starts_with("*** End Patch") {
            ended = true;
            flush_block(&mut block, &mut cur_hunk, &mut ops)?;
            continue;
        }
        if !begun {
            if line.trim().is_empty() {
                continue;
            }
            return Err(format!(
                "line {lineno}: expected *** Begin Patch (got {line:?})"
            ));
        }
        if ended {
            if line.trim().is_empty() {
                continue;
            }
            return Err(format!("line {lineno}: content after *** End Patch"));
        }
        if let Some(path) = line.strip_prefix("*** Add File:") {
            flush_block(&mut block, &mut cur_hunk, &mut ops)?;
            block = Some(Block::Add {
                path: path.trim().to_string(),
                body: Vec::new(),
            });
            in_hunk = false;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File:") {
            flush_block(&mut block, &mut cur_hunk, &mut ops)?;
            let path = path.trim().to_string();
            if path.is_empty() {
                return Err(format!("line {lineno}: *** Delete File: missing path"));
            }
            ops.push(FileOp::Delete { path });
            in_hunk = false;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File:") {
            flush_block(&mut block, &mut cur_hunk, &mut ops)?;
            block = Some(Block::Update {
                path: path.trim().to_string(),
                hunks: Vec::new(),
                move_to: None,
            });
            in_hunk = false;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Move to:") {
            match block.as_mut() {
                Some(Block::Update { move_to, .. }) => {
                    *move_to = Some(path.trim().to_string());
                }
                _ => {
                    return Err(format!(
                        "line {lineno}: *** Move to: without an open *** Update File: block"
                    ))
                }
            }
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            match block.as_mut() {
                Some(Block::Update { .. }) => {
                    flush_hunk(&mut block, &mut cur_hunk);
                    in_hunk = true;
                }
                _ => {
                    return Err(format!(
                        "line {lineno}: @@ hunk outside an *** Update File: block"
                    ))
                }
            }
            continue;
        }

        match block.as_mut() {
            Some(Block::Add { body, .. }) => {
                body.push(line.to_string());
            }
            Some(Block::Update { .. }) => {
                if !in_hunk {
                    return Err(format!(
                        "line {lineno}: expected @@ hunk header inside *** Update File: block (got {line:?})"
                    ));
                }
                if let Some(rest) = line.strip_prefix('+') {
                    if rest.starts_with('+') {
                        return Err(format!(
                            "line {lineno}: unsupported +++ header (single-file hunks only)"
                        ));
                    }
                    cur_hunk.push(HunkLine::Add(rest.to_string()));
                } else if let Some(rest) = line.strip_prefix('-') {
                    if rest.starts_with('-') {
                        return Err(format!(
                            "line {lineno}: unsupported --- header (single-file hunks only)"
                        ));
                    }
                    cur_hunk.push(HunkLine::Del(rest.to_string()));
                } else if let Some(rest) = line.strip_prefix(' ') {
                    cur_hunk.push(HunkLine::Context(rest.to_string()));
                } else if line.is_empty() {
                    cur_hunk.push(HunkLine::Context(String::new()));
                } else {
                    return Err(format!(
                        "line {lineno}: hunk lines must start with ' ', '-', '+' or @@ (got {line:?})"
                    ));
                }
            }
            None => {
                if line.trim().is_empty() {
                    continue;
                }
                return Err(format!(
                    "line {lineno}: content outside *** Add/Delete/Update File: block (got {line:?})"
                ));
            }
        }
    }
    if !begun {
        return Err("missing *** Begin Patch".into());
    }
    flush_block(&mut block, &mut cur_hunk, &mut ops)?;
    if !ended {
        return Err("missing *** End Patch".into());
    }

    let mut resolved: Vec<FileOp> = Vec::with_capacity(ops.len());
    let mut pending_update_path: Option<String> = None;
    for op in ops {
        match op {
            FileOp::Update { path, hunks } => {
                pending_update_path = Some(path.clone());
                resolved.push(FileOp::Update { path, hunks });
            }
            FileOp::Move { from, to } if from.is_empty() => match pending_update_path.take() {
                Some(from_path) => resolved.push(FileOp::Move {
                    from: from_path,
                    to,
                }),
                None => return Err("*** Move to: without a preceding *** Update File:".into()),
            },
            other => {
                pending_update_path = None;
                resolved.push(other);
            }
        }
    }
    if resolved.is_empty() {
        return Err("patch contains no file operations".into());
    }
    Ok(resolved)
}

pub fn apply_hunks(old: &[String], hunks: &[Hunk]) -> Result<Vec<String>, String> {
    let mut cur: Vec<String> = old.to_vec();
    for (hi, hunk) in hunks.iter().enumerate() {
        let old_block: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(t) | HunkLine::Del(t) => Some(t.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();
        if old_block.is_empty() {
            return Err(format!(
                "hunk {}: pure-addition hunks need at least one context line (add ' ' context around the + lines)",
                hi + 1
            ));
        }
        let matches: Vec<usize> = find_block(&cur, &old_block);
        if matches.is_empty() {
            let preview = old_block
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            // Orient the caller: where does the hunk's first line occur, if at all?
            let anchor_hint = old_block.first().map(|first| {
                let at: Vec<usize> = cur
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l == first)
                    .map(|(i, _)| i + 1)
                    .take(4)
                    .collect();
                if at.is_empty() {
                    " (first expected line occurs nowhere in the file — it may differ by whitespace/encoding; `read` the exact lines)".to_string()
                } else {
                    format!(" (first expected line occurs at file line(s) {})", at.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", "))
                }
            }).unwrap_or_default();
            return Err(format!(
                "hunk {} does not match (exact, no fuzz). First expected lines:\n{preview}\nHint: read the file and include 3+ lines of ' ' context.{anchor_hint}",
                hi + 1
            ));
        }
        if matches.len() > 1 {
            return Err(format!(
                "hunk {} matches {} locations; add more ' ' context lines to disambiguate",
                hi + 1,
                matches.len()
            ));
        }
        let at = matches[0];
        let mut replacement: Vec<String> = Vec::new();
        for l in &hunk.lines {
            match l {
                HunkLine::Context(t) | HunkLine::Add(t) => replacement.push(t.clone()),
                HunkLine::Del(_) => {}
            }
        }
        cur.splice(at..at + old_block.len(), replacement);
    }
    Ok(cur)
}

fn find_block(lines: &[String], block: &[&str]) -> Vec<usize> {
    if block.is_empty() || block.len() > lines.len() {
        return Vec::new();
    }
    (0..=lines.len() - block.len())
        .filter(|&i| {
            lines[i..i + block.len()]
                .iter()
                .zip(block.iter())
                .all(|(a, b)| a == b)
        })
        .collect()
}

pub async fn apply_patch_text(patch_text: &str, session_id: &str) -> serde_json::Value {
    let ops = match parse_patch(patch_text) {
        Ok(o) => o,
        Err(e) => return json!({"ok": false, "error": format!("patch parse failed: {e}")}),
    };

    let mut resolved: Vec<(FileOp, Vec<std::path::PathBuf>)> = Vec::new();
    for op in &ops {
        let mut bad: Option<String> = None;
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for p in touched_paths(std::slice::from_ref(op)) {
            if p.trim().is_empty() {
                bad = Some("empty path".into());
                break;
            }
            let rp = super::viora::resolve_path(&p);
            if !super::viora::approved_call() && !super::viora::is_within_root(&rp) {
                bad = Some(format!(
                    "access denied: {p} outside project root (approve in the Ask dialog or run with -y)"
                ));
                break;
            }
            paths.push(rp);
        }
        if let Some(e) = bad {
            return json!({"ok": false, "error": e});
        }
        resolved.push((op.clone(), paths));
    }
    let snap = crate::session::snapshot::UndoStack::new();
    let mut files = Vec::new();
    for (op, paths) in resolved {
        match op {
            FileOp::Add { path, content } => {
                let rp = &paths[0];
                if rp.exists() {
                    return json!({"ok": false, "error": format!("*** Add File: {path} already exists (use edit or write to modify)")});
                }
                if let Some(parent) = rp.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return json!({"ok": false, "error": format!("mkdir for {path}: {e}")});
                    }
                }
                if let Err(e) = tokio::fs::write(rp, &content).await {
                    return json!({"ok": false, "error": format!("write {path}: {e}")});
                }
                files.push(json!({"path": path, "op": "add", "bytes": content.len()}));
            }
            FileOp::Delete { path } => {
                let rp = &paths[0];
                let old = match tokio::fs::read_to_string(rp).await {
                    Ok(c) => c,
                    Err(e) => {
                        return json!({"ok": false, "error": format!("*** Delete File: read {path}: {e}")})
                    }
                };
                snap.push(session_id, crate::session::snapshot::current_seq_for(session_id), &rp.to_string_lossy()).await;
                if let Err(e) = tokio::fs::remove_file(rp).await {
                    return json!({"ok": false, "error": format!("delete {path}: {e}")});
                }
                files.push(json!({"path": path, "op": "delete", "old_lines": old.lines().count()}));
            }
            FileOp::Update { path, hunks } => {
                let rp = &paths[0];
                let old = match tokio::fs::read_to_string(rp).await {
                    Ok(c) => c,
                    Err(e) => {
                        return json!({"ok": false, "error": format!("*** Update File: read {path}: {e}")})
                    }
                };
                let old_lines: Vec<String> = old.lines().map(|l| l.to_string()).collect();
                let new_lines = match apply_hunks(&old_lines, &hunks) {
                    Ok(n) => n,
                    Err(e) => return json!({"ok": false, "error": format!("{path}: {e}")}),
                };
                let mut new_content = new_lines.join("\n");

                if old.ends_with('\n') && !new_content.is_empty() {
                    new_content.push('\n');
                }
                snap.push(session_id, crate::session::snapshot::current_seq_for(session_id), &rp.to_string_lossy()).await;
                if let Err(e) = tokio::fs::write(rp, &new_content).await {
                    return json!({"ok": false, "error": format!("write {path}: {e}")});
                }
                let diff = similar::TextDiff::from_lines(&old, &new_content);
                let diff_str = diff
                    .unified_diff()
                    .header(&format!("a/{path}"), &format!("b/{path}"))
                    .to_string();
                let mut entry = json!({
                    "path": path, "op": "update",
                    "hunks": hunks.len(),
                    "old_lines": old_lines.len(),
                    "new_lines": new_lines.len(),
                    "diff_preview": diff_str.chars().take(2000).collect::<String>(),
                });
                if path.ends_with(".cir") || path.ends_with(".sp") || path.ends_with(".flxsch") {
                    if let Some(diag) = super::after_write_hook(&path).await {
                        entry["diagnostics"] = diag;
                    }
                }
                files.push(entry);
            }
            FileOp::Move { from, to } => {
                let src = &paths[0];
                let dst = &paths[1];
                if !src.exists() {
                    return json!({"ok": false, "error": format!("*** Move: source missing: {from}")});
                }
                if dst.exists() {
                    return json!({"ok": false, "error": format!("*** Move: destination exists: {to}")});
                }
                snap.push(session_id, crate::session::snapshot::current_seq_for(session_id), &src.to_string_lossy()).await;
                if let Some(parent) = dst.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return json!({"ok": false, "error": format!("mkdir for {to}: {e}")});
                    }
                }
                if let Err(e) = tokio::fs::rename(src, dst).await {
                    return json!({"ok": false, "error": format!("move {from} -> {to}: {e}")});
                }
                files.push(json!({"path": to, "op": "move", "from": from}));
            }
        }
    }
    serde_json::json!({"ok": true, "files": files})
}

pub fn touched_paths(ops: &[FileOp]) -> Vec<String> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            FileOp::Add { path, .. } => out.push(path.clone()),
            FileOp::Delete { path } => out.push(path.clone()),
            FileOp::Update { path, .. } => out.push(path.clone()),
            FileOp::Move { from, to } => {
                out.push(from.clone());
                out.push(to.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "*** Begin Patch\n*** Update File: a.txt\n@@\n line1\n-line2\n+line2!\n line3\n*** End Patch";

    #[test]
    fn parse_update_single_hunk() {
        let ops = parse_patch(SIMPLE).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            FileOp::Update { path, hunks } => {
                assert_eq!(path, "a.txt");
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].lines.len(), 4);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn apply_update_ok() {
        let ops = parse_patch(SIMPLE).unwrap();
        let old: Vec<String> = ["line1", "line2", "line3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let new = apply_hunks(
            &old,
            match &ops[0] {
                FileOp::Update { hunks, .. } => hunks,
                _ => unreachable!(),
            },
        )
        .unwrap();
        assert_eq!(new, vec!["line1", "line2!", "line3"]);
    }

    #[test]
    fn apply_rejects_fuzzy_match() {
        let ops = parse_patch(SIMPLE).unwrap();
        let old: Vec<String> = ["line1", "line2 changed", "line3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = apply_hunks(
            &old,
            match &ops[0] {
                FileOp::Update { hunks, .. } => hunks,
                _ => unreachable!(),
            },
        )
        .unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn apply_rejects_ambiguous_match() {
        let text = "*** Begin Patch\n*** Update File: a.txt\n@@\n-x\n+y\n*** End Patch";
        let ops = parse_patch(text).unwrap();
        let old: Vec<String> = ["x", "x"].iter().map(|s| s.to_string()).collect();
        let err = apply_hunks(
            &old,
            match &ops[0] {
                FileOp::Update { hunks, .. } => hunks,
                _ => unreachable!(),
            },
        )
        .unwrap_err();
        assert!(err.contains("2 locations"), "{err}");
    }

    #[test]
    fn parse_add_delete_move() {
        let text = "*** Begin Patch\n*** Add File: new.txt\nhello\nworld\n*** Delete File: old.txt\n*** Update File: a.txt\n@@\n keep\n*** Move to: b.txt\n*** End Patch";
        let ops = parse_patch(text).unwrap();
        assert_eq!(ops.len(), 4);
        assert!(matches!(&ops[0], FileOp::Add { path, .. } if path == "new.txt"));
        assert!(matches!(&ops[1], FileOp::Delete { path } if path == "old.txt"));
        assert!(matches!(&ops[2], FileOp::Update { path, .. } if path == "a.txt"));
        assert!(matches!(&ops[3], FileOp::Move { from, to } if from == "a.txt" && to == "b.txt"));
    }

    #[test]
    fn parse_errors_are_actionable() {
        assert!(parse_patch("*** Update File: a.txt\n@@\n x\n*** End Patch")
            .unwrap_err()
            .contains("Begin Patch"));
        assert!(parse_patch(
            "*** Begin Patch\n*** Update File: a.txt\nno-hunk-here\n*** End Patch"
        )
        .unwrap_err()
        .contains("@@ hunk header"));
        assert!(
            parse_patch("*** Begin Patch\n*** Update File: a.txt\n@@\n?bad\n*** End Patch")
                .unwrap_err()
                .contains("must start with")
        );
        assert!(parse_patch("*** Begin Patch\n@@\n x\n*** End Patch")
            .unwrap_err()
            .contains("outside an *** Update File: block"));
        assert!(parse_patch("*** Begin Patch\n*** End Patch")
            .unwrap_err()
            .contains("no file operations"));
        assert!(parse_patch("*** Begin Patch\n*** Add File: x\n1\n")
            .unwrap_err()
            .contains("End Patch"));
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vh-patch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn executor_update_add_delete() {
        // Asserts jail denial: serialize against tests that approve all.
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = test_dir("exec");
        let a = dir.join("a.txt");
        std::fs::write(&a, "one\ntwo\nthree\n").unwrap();
        let a_s = a.to_string_lossy().to_string();
        let new_s = dir.join("sub/new.txt").to_string_lossy().to_string();

        let patch = format!(
            "*** Begin Patch\n*** Update File: {a_s}\n@@\n one\n-two\n+TWO\n three\n*** End Patch"
        );
        let r = apply_patch_text(&patch, "sess-x").await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\nTWO\nthree\n");

        let patch = format!("*** Begin Patch\n*** Add File: {new_s}\nhello\n*** End Patch");
        let r = apply_patch_text(&patch, "sess-x").await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(
            std::fs::read_to_string(dir.join("sub/new.txt")).unwrap(),
            "hello\n"
        );

        let patch = format!("*** Begin Patch\n*** Delete File: {new_s}\n*** End Patch");
        let r = apply_patch_text(&patch, "sess-x").await;
        assert_eq!(r["ok"], true, "{r}");
        assert!(!dir.join("sub/new.txt").exists());

        let patch = "*** Begin Patch\n*** Add File: /etc/vh-nope.txt\nx\n*** End Patch";
        let r = apply_patch_text(patch, "sess-x").await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("outside project root"),
            "{r}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn executor_bad_hunk_fails_clean() {
        let dir = test_dir("badhunk");
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let a_s = dir.join("a.txt").to_string_lossy().to_string();
        let patch =
            format!("*** Begin Patch\n*** Update File: {a_s}\n@@\n one\n-TW0\n+two\n*** End Patch");
        let r = apply_patch_text(&patch, "sess-x").await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("does not match"),
            "{r}"
        );

        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_hunk_with_drift() {
        let text = "*** Begin Patch\n*** Update File: a.txt\n@@ first\n-a1\n+b1\n@@ second\n-a9\n+b9\n*** End Patch";
        let ops = parse_patch(text).unwrap();
        let old: Vec<String> = (1..=10).map(|i| format!("a{i}")).collect();
        let new = apply_hunks(
            &old,
            match &ops[0] {
                FileOp::Update { hunks, .. } => hunks,
                _ => unreachable!(),
            },
        )
        .unwrap();
        assert_eq!(new[0], "b1");
        assert_eq!(new[8], "b9");
        assert_eq!(new.len(), 10);
    }

    #[test]
    fn mismatch_hint_points_at_anchor_line() {
        // P0 #5: when the hunk fails, orient the caller with the closest match.
        // Exact matching is encoding-agnostic (katakana lines match byte-wise).
        let ops = parse_patch(
            "*** Begin Patch\n*** Update File: f.js\n@@\n if(mcan) resizeMatrix();\n-bbb\n+BBB\n*** End Patch",
        )
        .unwrap();
        let hunks = match &ops[0] {
            FileOp::Update { hunks, .. } => hunks.clone(),
            _ => unreachable!(),
        };
        let new = apply_hunks(
            &["if(mcan) resizeMatrix();".to_string(), "bbb".to_string()],
            &hunks,
        )
        .unwrap();
        assert_eq!(new, vec!["if(mcan) resizeMatrix();", "BBB"]);

        // Trailing comment differs → mismatch, and the bare line is absent.
        let err = apply_hunks(
            &[
                "if(mcan) resizeMatrix(); // ｱｲｳ".to_string(),
                "bbb".to_string(),
            ],
            &hunks,
        )
        .unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        assert!(err.contains("occurs nowhere"), "{err}");

        // Anchor present but block broken → hint names the file line.
        let err2 = apply_hunks(
            &["if(mcan) resizeMatrix();".to_string(), "zzz".to_string()],
            &hunks,
        )
        .unwrap_err();
        assert!(err2.contains("line(s) 1"), "{err2}");
    }
}
