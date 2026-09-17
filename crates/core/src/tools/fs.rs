// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::viora::{is_within_root, resolve_path};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncReadExt;

fn is_screenshot_alias(text: &str) -> bool {
    matches!(
        text.trim().to_lowercase().as_str(),
        "screenshot"
            | "screenshot:latest"
            | "last screenshot"
            | "latest screenshot"
            | "newest screenshot"
            | "recent screenshot"
    )
}

fn latest_screenshot_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for dir in dirs {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if !name.starts_with("screenshot_") {
                continue;
            }
            let ext_ok = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_lowercase().as_str(), "png" | "jpg" | "jpeg"))
                .unwrap_or(false);
            if !ext_ok {
                continue;
            }
            let mtime = entry.metadata().and_then(|m| m.modified()).ok()?;
            let replace = best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true);
            if replace {
                best = Some((mtime, p));
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.map(|(_, p)| p)
}

fn resolve_screenshot_alias() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base = PathBuf::from(home);
    latest_screenshot_in(&[base.join("Pictures"), base.join("Desktop")])
}

fn image_mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn is_unsupported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default()
            .as_str(),
        "bmp" | "tif" | "tiff"
    )
}

async fn read_image_as_vision(path: &Path, path_str: &str, mime: &'static str) -> Value {
    const VISION_B64_CAP: usize = 700_000;

    const RAW_CAP: usize = 600_000;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let bytes = match fs::read(path).await {
        Ok(b) => b,
        Err(e) => return json!({"ok": false, "error": format!("read {path_str}: {e}")}),
    };
    if bytes.is_empty() {
        return json!({"ok": false, "error": format!("read {path_str}: empty image file")});
    }

    if mime == "image/webp" {
        if bytes.len() > RAW_CAP {
            return json!({"ok": false, "error": format!("{name}: too large ({}KB) and webp can't downscale — convert to PNG first", bytes.len() / 1024)});
        }
        return json!({
            "ok": true, "path": path_str, "mime": mime,
            "bytes": bytes.len(), "base64": BASE64.encode(&bytes),
            "note": "image attached as vision (base64 stripped from text) — describe what you see, don't dump pixels"
        });
    }
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i,
        Err(e) => {
            return json!({"ok": false, "error": format!("{name}: cannot decode image ({e})")})
        }
    };

    for (max_dim, quality) in [(1024u32, 85u8), (768, 75), (512, 70)] {
        let thumb = if img.width().max(img.height()) > max_dim {
            img.thumbnail(max_dim, max_dim)
        } else {
            img.clone()
        };
        let rgb = thumb.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        if enc
            .encode(&rgb.into_raw(), w, h, image::ExtendedColorType::Rgb8)
            .is_err()
        {
            continue;
        }
        let b64 = BASE64.encode(&buf);
        if b64.len() <= VISION_B64_CAP {
            return json!({
                "ok": true, "path": path_str, "mime": "image/jpeg",
                "width": w, "height": h,
                "bytes": bytes.len(), "base64": b64,
                "note": "image attached as vision (base64 stripped from text) — describe what you see, don't dump pixels"
            });
        }
    }
    json!({"ok": false, "error": format!("{name}: too large even downscaled to 512px")})
}

pub async fn read_file(args: Value) -> Value {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if path_str.is_empty() {
        return json!({"ok": false, "error": "missing path"});
    }

    if is_screenshot_alias(path_str) {
        match resolve_screenshot_alias() {
            Some(p) => {
                let mime = image_mime_for(&p).unwrap_or("image/png");
                let resolved = p.to_string_lossy().to_string();
                return read_image_as_vision(&p, &resolved, mime).await;
            }
            None => {
                return json!({"ok": false, "error": "no screenshots found (looked for Screenshot_*.png in ~/Pictures and ~/Desktop)"})
            }
        }
    }
    let path = resolve_path(path_str);
    if !super::viora::approved_call() && !is_within_root(&path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external access)", path.display())});
    }

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    if let Some(mime) = image_mime_for(&path) {
        return read_image_as_vision(&path, path_str, mime).await;
    }
    if is_unsupported_image(&path) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
        return json!({"ok": false, "error": format!("{name}: BMP/TIFF not supported for vision — convert to PNG first")});
    }
    match fs::read_to_string(&path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let start = offset.min(total);
            let end = if let Some(lim) = limit {
                if lim == 0 {
                    total
                } else {
                    (start + lim).min(total)
                }
            } else {
                total
            };
            let selected = &lines[start..end];
            let out = selected.join("\n");

            if selected.len() > 2000 {
                let truncated = selected[..2000].join("\n");
                json!({"ok": true, "path": path_str, "content": truncated, "truncated": true, "total_lines": total, "offset": offset, "limit": limit})
            } else {
                let mut res =
                    json!({"ok": true, "path": path_str, "content": out, "total_lines": total});
                if offset != 0 {
                    res["offset"] = json!(offset);
                }
                if let Some(lim) = limit {
                    if lim != 0 {
                        res["limit"] = json!(lim);
                    }
                }

                res
            }
        }
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

pub async fn write_file(args: Value) -> Value {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if path_str.is_empty() {
        return json!({"ok": false, "error": "missing path"});
    }
    let path = resolve_path(path_str);
    if !super::viora::approved_call() && !is_within_root(&path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external writes)", path.display())});
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent).await {
            return json!({"ok": false, "error": e.to_string()});
        }
    }

    let old_content = fs::read_to_string(&path).await.unwrap_or_default();
    let old_lines = old_content.lines().count();

    match fs::write(&path, content).await {
        Ok(_) => {
            let new_lines = content.lines().count();

            let diff = similar::TextDiff::from_lines(old_content.as_str(), content);
            let diff_str = diff
                .unified_diff()
                .header(&format!("a/{}", path_str), &format!("b/{}", path_str))
                .to_string();
            let diff_preview = if diff_str.len() > 4000 {
                format!(
                    "{}… ({} chars total, truncated)",
                    &diff_str[..4000],
                    diff_str.len()
                )
            } else {
                diff_str.clone()
            };
            let mut res = json!({
                "ok": true,
                "path": path_str,
                "old_lines": old_lines,
                "new_lines": new_lines,
                "diff": diff_str,
                "diff_preview": diff_preview,
                "bytes": content.len()
            });

            if old_content == content {
                res["note"] = json!("no change (identical)");
            }
            res
        }
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

pub async fn edit_file(args: Value) -> Value {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let old_string = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_string = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if path_str.is_empty() {
        return json!({"ok": false, "error": "missing path"});
    }
    if old_string.is_empty() {
        return json!({"ok": false, "error": "missing old_string (exact text to replace)"});
    }
    let path = resolve_path(path_str);
    if !super::viora::approved_call() && !is_within_root(&path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external writes)", path.display())});
    }
    let old_content = match fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("read {path_str}: {e}")}),
    };
    let matches = old_content.matches(old_string).count();
    if matches == 0 {
        let head: String = old_content.lines().take(5).collect::<Vec<_>>().join("\n");
        return json!({"ok": false, "error": format!("old_string not found in {path_str} (exact match, including whitespace). File starts:\n{head}\nHint: read the file first and copy the block verbatim.")});
    }
    if matches > 1 && !replace_all {
        return json!({"ok": false, "error": format!("old_string matches {matches} locations in {path_str}; add more surrounding context to make it unique, or set replace_all:true")});
    }
    let new_content = if replace_all {
        old_content.replace(old_string, new_string)
    } else {
        old_content.replacen(old_string, new_string, 1)
    };
    if new_content == old_content {
        return json!({"ok": true, "path": path_str, "note": "no change (identical)"});
    }

    if let Some(sid) = args.get("session_id").and_then(|v| v.as_str()) {
        let snap = crate::session::snapshot::UndoStack::new();
        let seq = crate::session::snapshot::current_seq_for(sid);
        snap.push(sid, seq, &path.to_string_lossy()).await;
    }
    if let Err(e) = fs::write(&path, &new_content).await {
        return json!({"ok": false, "error": e.to_string()});
    }
    let diff = similar::TextDiff::from_lines(old_content.as_str(), new_content.as_str());
    let diff_str = diff
        .unified_diff()
        .header(&format!("a/{path_str}"), &format!("b/{path_str}"))
        .to_string();
    let diff_preview = if diff_str.len() > 4000 {
        format!(
            "{}… ({} chars total, truncated)",
            &diff_str[..4000],
            diff_str.len()
        )
    } else {
        diff_str.clone()
    };
    json!({
        "ok": true,
        "path": path_str,
        "replacements": if replace_all { matches } else { 1 },
        "old_lines": old_content.lines().count(),
        "new_lines": new_content.lines().count(),
        "diff": diff_str,
        "diff_preview": diff_preview,
        "bytes": new_content.len()
    })
}

pub async fn list_files(args: Value) -> Value {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let path = resolve_path(path_str);

    if !super::viora::approved_call() && !is_within_root(&path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external access)", path.display())});
    }
    let entries = match fs::read_dir(&path).await {
        Ok(mut dir) => {
            let mut files = Vec::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                let meta = entry.metadata().await.ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta
                    .as_ref()
                    .map(|m| if m.is_file() { m.len() } else { 0 })
                    .unwrap_or(0);
                files.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "type": if is_dir { "directory" } else { "file" },
                    "size": size
                }));
            }
            files
        }
        Err(e) => return json!({"ok": false, "error": e.to_string()}),
    };
    json!({"ok": true, "path": path_str, "files": entries})
}

pub async fn glob_files(args: Value) -> Value {
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    if pattern.is_empty() {
        return json!({"ok": false, "error": "missing pattern"});
    }
    let base = resolve_path(path);
    if !super::viora::approved_call() && !is_within_root(&base) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external access)", base.display())});
    }

    let mut out = Vec::new();
    let _ = walk_glob(&base, &base, pattern, &mut out, 0).await;

    if out.len() > 100 {
        out.truncate(100);
    }
    json!({"ok": true, "pattern": pattern, "files": out, "truncated": out.len() >= 100})
}

async fn walk_glob(
    base: &PathBuf,
    dir: &PathBuf,
    pattern: &str,
    out: &mut Vec<String>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let mut rd = match fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if name.starts_with('.')
                || name == "target"
                || name == "build"
                || name == "node_modules"
            {
                continue;
            }
            if let Ok(rel) = p.strip_prefix(base) {
                if glob_match_path(&rel.to_string_lossy(), pattern) {
                    out.push(p.to_string_lossy().to_string());
                    if out.len() >= 120 {
                        return;
                    }
                }
            }
            Box::pin(walk_glob(base, &p, pattern, out, depth + 1)).await;
        } else if let Ok(rel) = p.strip_prefix(base) {
            if glob_match_path(&rel.to_string_lossy(), pattern) {
                let display = p.to_string_lossy().to_string();
                out.push(display);
                if out.len() >= 120 {
                    break;
                }
            }
        }
    }
}

/// Segment-aware glob: `*` spans within one path segment, `?` one char,
/// `**` spans any number of segments. A pattern without `/` matches the
/// basename at any depth (`*.cir` finds nested files too).
fn segment_match(seg: &str, pat: &str) -> bool {
    let s: Vec<char> = seg.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    let (mut si, mut pi) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            si += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn match_segments(path: &[&str], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        if pat.len() == 1 {
            return true;
        }
        return (0..=path.len()).any(|i| match_segments(&path[i..], &pat[1..]));
    }
    if path.is_empty() {
        return false;
    }
    segment_match(path[0], pat[0]) && match_segments(&path[1..], &pat[1..])
}

fn glob_match_path(rel: &str, pattern: &str) -> bool {
    let pat = pattern.trim().trim_start_matches("./");
    if pat.is_empty() {
        return false;
    }
    if !pat.contains('/') {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        return segment_match(name, pat);
    }
    let psegs: Vec<&str> = pat.split('/').collect();
    let rsegs: Vec<&str> = rel.split('/').collect();
    match_segments(&rsegs, &psegs)
}

pub async fn grep(args: Value) -> Value {
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    if pattern.is_empty() {
        return json!({"ok": false, "error": "missing pattern"});
    }

    let base = resolve_path(path);
    if !super::viora::approved_call() && !is_within_root(&base) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external access)", base.display())});
    }
    if let Ok(out) = tokio::process::Command::new("rg")
        .args([
            "--json",
            "--no-heading",
            "-n",
            pattern,
            base.to_string_lossy().as_ref(),
        ])
        .output()
        .await
    {
        // rg exit 0 = matches, 1 = no matches; other codes = real error.
        if !out.status.success() && out.status.code() != Some(1) {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err
                .lines()
                .next()
                .unwrap_or("search failed")
                .trim()
                .to_string();
            return json!({"ok": false, "error": format!("rg search failed: {msg}"), "pattern": pattern, "hits": []});
        }
        // rg answered (matches or clean no-match).
        let text = String::from_utf8_lossy(&out.stdout);
        let mut hits = Vec::new();
        for line in text.lines() {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("type").and_then(|t| t.as_str()) == Some("match") {
                let data = v.get("data").cloned().unwrap_or(json!({}));
                let path = data
                    .get("path")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let lines = data
                    .get("lines")
                    .and_then(|l| l.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let line_no = data
                    .get("line_number")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                hits.push(json!({"path": path, "line": line_no, "text": lines.trim_end()}));
                if hits.len() >= 100 {
                    break;
                }
            }
        }
        return json!({"ok": true, "pattern": pattern, "hits": hits, "truncated": hits.len() >= 100, "backend": "rg"});
    }

    // Fallback when rg is not installed: system grep -R. Same shape.
    // grep exit 0 = matches, 1 = no matches; spawn error = no grep either.
    if let Ok(out) = tokio::process::Command::new("grep")
        .args(["-RnI", "--", pattern, base.to_string_lossy().as_ref()])
        .output()
        .await
    {
        // grep exit 0 = matches, 1 = no matches; other codes = real error.
        if !out.status.success() && out.status.code() != Some(1) {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err
                .lines()
                .next()
                .unwrap_or("search failed")
                .trim()
                .to_string();
            return json!({"ok": false, "error": format!("grep search failed: {msg}"), "pattern": pattern, "hits": []});
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut hits = Vec::new();
        for line in text.lines() {
            let mut parts = line.splitn(3, ':');
            let (Some(path), Some(no), Some(text)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let line_no = no.parse::<u64>().unwrap_or(0);
            if line_no == 0 {
                continue;
            }
            hits.push(json!({"path": path, "line": line_no, "text": text.trim_end()}));
            if hits.len() >= 100 {
                break;
            }
        }
        return json!({"ok": true, "pattern": pattern, "hits": hits, "truncated": hits.len() >= 100, "backend": "grep"});
    }

    json!({"ok": false, "error": "no search backend available (tried `rg` and `grep -R`; install ripgrep for best results)", "pattern": pattern, "hits": []})
}

pub async fn read_image_base64(args: Value) -> Value {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let path = resolve_path(path_str);

    if !super::viora::approved_call() && !is_within_root(&path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y to allow external access)", path.display())});
    }
    let mut file = match fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return json!({"ok": false, "error": e.to_string()}),
    };
    let mut buf = Vec::new();
    if let Err(e) = file.read_to_end(&mut buf).await {
        return json!({"ok": false, "error": e.to_string()});
    }
    let b64 = BASE64.encode(&buf);
    json!({"ok": true, "path": path_str, "base64": b64, "size": buf.len()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn glob_double_star_spans_directories() {
        // P0 #1: `**/*` and `**/*.js` returned nothing.
        assert!(glob_match_path("app.js", "**/*"));
        assert!(glob_match_path("sub/inner.js", "**/*"));
        assert!(glob_match_path("sub/inner.js", "**/*.js"));
        assert!(glob_match_path("app.js", "*.js"));
        assert!(
            glob_match_path("sub/inner.js", "*.js"),
            "basename at any depth"
        );
        assert!(glob_match_path("anything", "*"));
        assert!(glob_match_path("sub/deck.cir", "sub/*.cir"));
        assert!(!glob_match_path("other/deck.cir", "sub/*.cir"));
        assert!(glob_match_path("ab", "a?"));
        assert!(!glob_match_path("app.js", "*.cir"));
        assert!(!glob_match_path("app.js", ""));
        assert!(glob_match_path("app.js", "app.js"));
        assert!(
            !glob_match_path("myapp.js", "app.js"),
            "exact, not contains"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn glob_end_to_end_nested() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
        let dir = std::env::temp_dir().join(format!("vh-glob2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("app.js"), "x").unwrap();
        std::fs::write(dir.join("sub").join("inner.js"), "y").unwrap();
        let d = dir.to_string_lossy().to_string();
        let r = glob_files(json!({"path": d, "pattern": "**/*.js"})).await;
        assert_eq!(r["ok"], true);
        assert_eq!(r["files"].as_array().unwrap().len(), 2, "{r}");
        let r = glob_files(json!({"path": d, "pattern": "sub/*.js"})).await;
        assert_eq!(r["files"].as_array().unwrap().len(), 1, "{r}");
        let _ = std::fs::remove_dir_all(&dir);
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn grep_falls_back_without_rg() {
        // P0 #2/#9: must return hits via system grep, never silent ok:true.
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
        let dir = std::env::temp_dir().join(format!("vh-grep2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello APPS world\n").unwrap();
        let d = dir.to_string_lossy().to_string();
        let r = grep(json!({"path": d, "pattern": "APPS"})).await;
        assert_eq!(r["ok"], true, "{r}");
        let hits = r["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{r}");
        assert_eq!(hits[0]["line"], 1);
        assert!(hits[0]["text"].as_str().unwrap().contains("APPS"));
        assert!(r["backend"]
            .as_str()
            .is_some_and(|b| b == "rg" || b == "grep"));
        let r = grep(json!({"path": d, "pattern": "zzz-no-match"}).clone()).await;
        assert_eq!(
            r["ok"], true,
            "no match is ok:true with empty hits, not an error"
        );
        assert!(r["hits"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }

    fn test_file(name: &str, content: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("vh-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.txt");
        std::fs::write(&p, content).unwrap();
        let s = p.to_string_lossy().to_string();
        (dir, s)
    }

    #[tokio::test]
    async fn edit_exact_once() {
        let (dir, s) = test_file("once", "alpha\nbeta\ngamma\n");
        let r = edit_file(json!({"path": s, "old_string": "beta\n", "new_string": "BETA\n"})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["replacements"], 1);
        assert!(r["diff"].as_str().unwrap().contains("-beta"));
        assert!(std::fs::read_to_string(dir.join("f.txt"))
            .unwrap()
            .contains("BETA"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn edit_zero_and_multi_match_errors() {
        let (dir, s) = test_file("multi", "x\nx\n");
        let r = edit_file(json!({"path": s.clone(), "old_string": "zzz", "new_string": "y"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("not found"),
            "{}",
            r["error"]
        );

        let r =
            edit_file(json!({"path": s.clone(), "old_string": "x\n", "new_string": "y\n"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("2 locations"),
            "{}",
            r["error"]
        );

        let r = edit_file(
            json!({"path": s, "old_string": "x\n", "new_string": "y\n", "replace_all": true}),
        )
        .await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["replacements"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn img_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vh-img-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn gradient_png(path: &std::path::Path, w: u32, h: u32) {
        let img = image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128u8])
        });
        img.save(path).unwrap();
    }

    #[test]
    fn screenshot_alias_matching() {
        for a in [
            "screenshot",
            "screenshot:latest",
            "last screenshot",
            "latest screenshot",
            "newest screenshot",
            "recent screenshot",
            "  Last Screenshot  ",
        ] {
            assert!(is_screenshot_alias(a), "{a:?} is an alias");
        }
        for n in [
            "",
            "screenshot2",
            "/home/jnd/Pictures/shot.png",
            "read screenshot",
            "screenshots",
        ] {
            assert!(!is_screenshot_alias(n), "{n:?} is not an alias");
        }
    }

    #[test]
    fn latest_screenshot_picks_newest_match() {
        let dir = std::env::temp_dir().join(format!("vh-shot-{}.db", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pics = dir.join("Pictures");
        std::fs::create_dir_all(&pics).unwrap();
        let mk = |name: &str, age_secs: u64| {
            let p = pics.join(name);
            std::fs::write(&p, b"x").unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
            p
        };
        let old = mk("Screenshot_old.png", 300);
        let new = mk("Screenshot_new.png", 60);
        mk("notes.txt", 10);
        mk("other.jpg", 5);
        mk("Screenshot_anim.gif", 5);
        let got = latest_screenshot_in(std::slice::from_ref(&pics)).expect("finds one");
        assert_eq!(got, new, "newest Screenshot_*.png wins");
        assert_ne!(got, old);

        assert!(latest_screenshot_in(&[dir.join("Nope")]).is_none());
        assert!(latest_screenshot_in(&[]).is_none());

        let other = dir.join("Desktop");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Screenshot_fresh.png"), b"x").unwrap();
        let got = latest_screenshot_in(&[pics, other]).expect("finds one");
        assert!(got.to_string_lossy().contains("Pictures"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_image_returns_vision() {
        let dir = img_dir("vision");
        let p = dir.join("big.png");
        gradient_png(&p, 1600, 900);
        let r = read_file(json!({"path": p.to_string_lossy()})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["mime"], "image/jpeg", "re-encoded for budget");
        let w = r["width"].as_u64().unwrap();
        assert!(w <= 1024 && w > 0, "downscaled: {r}");
        let b64 = r["base64"].as_str().unwrap();
        assert!(b64.len() <= 700_000, "fits vision budget: {}", b64.len());
        assert!(!b64.is_empty());
        assert!(r["note"].as_str().unwrap().contains("vision"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_jpeg_attaches_with_mime() {
        let dir = img_dir("jpeg");
        let p = dir.join("a.jpg");
        let img = image::ImageBuffer::from_fn(64, 64, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 200u8])
        });
        img.save(&p).unwrap();
        let r = read_file(json!({"path": p.to_string_lossy()})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["mime"], "image/jpeg");
        assert!(r["base64"].as_str().is_some_and(|s| !s.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_text_path_unchanged() {
        let dir = img_dir("text");
        let p = dir.join("n.txt");
        std::fs::write(&p, "hello\nworld\n").unwrap();
        let r = read_file(json!({"path": p.to_string_lossy()})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert!(r.get("base64").is_none(), "no vision for text");
        assert!(r["content"].as_str().unwrap().contains("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_image_refusals_are_clean_errors() {
        let dir = img_dir("refuse");

        let bmp = dir.join("a.bmp");
        std::fs::write(&bmp, vec![7u8; 100]).unwrap();
        let r = read_file(json!({"path": bmp.to_string_lossy()})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("BMP/TIFF"),
            "{}",
            r["error"]
        );

        let r = read_file(json!({"path": dir.join("nope.png").to_string_lossy()})).await;
        assert_eq!(r["ok"], false);

        let big = dir.join("big.webp");
        std::fs::write(&big, vec![7u8; 700_000]).unwrap();
        let r = read_file(json!({"path": big.to_string_lossy()})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("too large"),
            "{}",
            r["error"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn traversal_via_tmp_dotdot_denied() {
        // Depends on default env (no blanket approval): serialize against
        // tests that set VIORAHARNESS_APPROVED_CALL, even though this test
        // itself mutates nothing.
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let pid = std::process::id();

        let evil = format!("/tmp/../vh-outside-{pid}/x.txt");
        for (name, res) in [
            ("read", read_file(json!({"path": evil})).await),
            (
                "write",
                write_file(json!({"path": evil, "content": "x"})).await,
            ),
            (
                "edit",
                edit_file(json!({"path": evil, "old_string": "a", "new_string": "b"})).await,
            ),
            (
                "glob",
                glob_files(json!({"pattern": "*.rs", "path": format!("/tmp/../vh-outside-{pid}")}))
                    .await,
            ),
            (
                "grep",
                grep(json!({"pattern": "x", "path": format!("/tmp/../vh-outside-{pid}")})).await,
            ),
            (
                "list",
                list_files(json!({"path": format!("/tmp/../vh-outside-{pid}")})).await,
            ),
            ("image", read_image_base64(json!({"path": evil})).await),
        ] {
            assert_eq!(res["ok"], false, "{name} must deny");
            assert!(
                res["error"]
                    .as_str()
                    .unwrap()
                    .contains("outside project root"),
                "{name}: {res}"
            );
        }

        let r = read_file(json!({"path": "/tmp/vh-missing-xyz-12345"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            !r["error"]
                .as_str()
                .unwrap()
                .contains("outside project root"),
            "legit /tmp must pass jail: {r}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn edit_outside_root_denied() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let r = edit_file(json!({"path": "/etc/vh-nope-x", "old_string": "a", "new_string": "b"}))
            .await;
        assert_eq!(r["ok"], false);
        assert!(r["error"]
            .as_str()
            .unwrap()
            .contains("outside project root"));
    }
}
