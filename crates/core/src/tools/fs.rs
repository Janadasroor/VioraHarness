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

    let _rel =
        crate::tools::viora::relativize_if_under_base(&path, &crate::tools::viora::viospice_root());

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
        snap.push(sid, 0, &path.to_string_lossy()).await;
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
    let _ = walk_glob(&base, pattern, &mut out, 0).await;

    if out.len() > 100 {
        out.truncate(100);
    }
    json!({"ok": true, "pattern": pattern, "files": out, "truncated": out.len() >= 100})
}

async fn walk_glob(dir: &PathBuf, pattern: &str, out: &mut Vec<String>, depth: usize) {
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
            Box::pin(walk_glob(&p, pattern, out, depth + 1)).await;
        } else if glob_match(&name, pattern) {
            let display = p.to_string_lossy().to_string();
            out.push(display);
            if out.len() >= 120 {
                break;
            }
        }
    }
}

fn glob_match(name: &str, pattern: &str) -> bool {
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            return name.starts_with(parts[0]) && name.ends_with(parts[1]);
        }

        return name.contains(&pattern.replace('*', ""));
    }

    if pattern.starts_with("*.") {
        return name.ends_with(&pattern[1..]);
    }
    name.contains(pattern)
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
    let rg = tokio::process::Command::new("rg")
        .args([
            "--json",
            "--no-heading",
            "-n",
            pattern,
            base.to_string_lossy().as_ref(),
        ])
        .output()
        .await;
    if let Ok(out) = rg {
        if out.status.success() || !out.stdout.is_empty() {
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
            return json!({"ok": true, "pattern": pattern, "hits": hits, "truncated": hits.len() >= 100});
        }
    }

    json!({"ok": true, "pattern": pattern, "hits": [], "note": "rg not available, no hits"})
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
        let got = latest_screenshot_in(&[pics.clone()]).expect("finds one");
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
    async fn traversal_via_tmp_dotdot_denied() {
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
    async fn edit_outside_root_denied() {
        let r = edit_file(json!({"path": "/etc/vh-nope-x", "old_string": "a", "new_string": "b"}))
            .await;
        assert_eq!(r["ok"], false);
        assert!(r["error"]
            .as_str()
            .unwrap()
            .contains("outside project root"));
    }
}
