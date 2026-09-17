// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::clipboard::{png_from_rgba, rgba_from_png, shrink_rgba};
use base64::Engine as _;

pub(crate) const CHAT_COLLAPSE_LINES: usize = 12;
pub(crate) const CHAT_COLLAPSE_HEAD: usize = 8;

pub(crate) fn image_ext_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub(crate) fn detect_image_path(text: &str) -> Option<std::path::PathBuf> {
    let mut t = text.trim();
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            t = &t[1..t.len() - 1];
            break;
        }
    }
    let t = t.strip_prefix("file://").unwrap_or(t).trim();
    if t.is_empty() {
        return None;
    }
    let expanded = if let Some(rest) = t.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/{rest}")
    } else {
        t.to_string()
    };
    let path = std::path::PathBuf::from(&expanded);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    image_ext_mime(&ext)?;
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

pub(crate) fn load_image_file(
    path: &std::path::Path,
) -> Result<(String, &'static str, String), String> {
    const RAW_IMAGE_CAP: usize = 600_000;
    const BUDGET_B64: usize = 700_000;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext == "bmp" {
        return Err(format!("{name}: BMP not supported — convert to PNG first"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: cannot read ({e})"))?;
    if ext == "png" {
        let (w, h, px) =
            rgba_from_png(&bytes).ok_or_else(|| format!("{name}: cannot decode PNG"))?;
        for max in [1024u32, 768, 512, 384] {
            let (w2, h2, small) = shrink_rgba(w, h, &px, max);
            if let Some(png) = png_from_rgba(w2, h2, &small) {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                if b64.len() <= BUDGET_B64 {
                    return Ok((b64, "image/png", format!("{name} · {w2}×{h2}")));
                }
            }
        }
        return Err(format!("{name}: too large even downscaled to 384px"));
    }
    let Some(mime) = image_ext_mime(&ext) else {
        return Err(format!("{name}: unsupported image type"));
    };
    if bytes.len() > RAW_IMAGE_CAP {
        return Err(format!(
            "{name}: too large ({}KB, JPEG/GIF can't downscale — screenshot to PNG instead)",
            bytes.len() / 1024
        ));
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((b64, mime, name))
}

pub(crate) fn collapse_long_content(content: &str, expanded: bool) -> (String, Option<usize>) {
    let total = content.lines().count();
    if expanded || total <= CHAT_COLLAPSE_LINES {
        return (content.to_string(), None);
    }
    let mut head: String = content
        .lines()
        .take(CHAT_COLLAPSE_HEAD)
        .collect::<Vec<_>>()
        .join("\n");
    if head.lines().filter(|l| l.trim().starts_with("```")).count() % 2 == 1 {
        head.push_str("\n```");
    }
    (head, Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_path_detection() {
        let dir = std::env::temp_dir().join(format!("vh_paste_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        let txt = dir.join("notes.txt");
        std::fs::write(&png, b"fakepng").unwrap();
        std::fs::write(&txt, b"hi").unwrap();
        let ps = png.to_string_lossy().to_string();
        assert_eq!(detect_image_path(&ps).unwrap(), png);
        assert_eq!(
            detect_image_path(&format!("\"{ps}\"")).unwrap(),
            png,
            "quoted drag-drop"
        );
        assert_eq!(
            detect_image_path(&format!("file://{ps}")).unwrap(),
            png,
            "file:// prefix"
        );
        assert!(
            detect_image_path(&txt.to_string_lossy()).is_none(),
            "txt rejected"
        );
        assert!(detect_image_path(&dir.join("missing.png").to_string_lossy()).is_none());
        assert!(
            detect_image_path("two\nlines.png").is_none(),
            "multiline rejected"
        );
        assert!(detect_image_path("").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_file_loading() {
        let dir = std::env::temp_dir().join(format!("vh_imgload_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let px: Vec<u8> = (0..(16 * 16 * 4)).map(|i| (i % 251) as u8).collect();
        let png_path = dir.join("a.png");
        std::fs::write(&png_path, png_from_rgba(16, 16, &px).unwrap()).unwrap();
        let (b64, mime, label) = load_image_file(&png_path).expect("png loads");
        assert_eq!(mime, "image/png");
        assert!(
            label.contains("a.png") && label.contains("16×16"),
            "{label}"
        );
        assert!(!b64.is_empty());

        let jpg_path = dir.join("b.jpg");
        std::fs::write(&jpg_path, vec![7u8; 1000]).unwrap();
        let (_, mime, _) = load_image_file(&jpg_path).expect("jpg loads");
        assert_eq!(mime, "image/jpeg");

        let bmp_path = dir.join("c.bmp");
        std::fs::write(&bmp_path, vec![7u8; 100]).unwrap();
        assert!(load_image_file(&bmp_path).is_err(), "bmp refused");
        let big_path = dir.join("d.jpg");
        std::fs::write(&big_path, vec![7u8; 700_000]).unwrap();
        assert!(load_image_file(&big_path).is_err(), "oversize refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
