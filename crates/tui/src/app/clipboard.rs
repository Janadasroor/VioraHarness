use base64::Engine as _;

pub(crate) fn osc52_copy_text(text: &str) -> Result<(usize, bool), String> {
    use std::io::Write as _;
    const OSC52_MAX_CHARS: usize = 100_000;
    let capped: String = text.chars().take(OSC52_MAX_CHARS).collect();
    let truncated = capped.chars().count() < text.chars().count();
    let b64 = base64::engine::general_purpose::STANDARD.encode(capped.as_bytes());
    let seq = format!("\x1b]52;c;{b64}\x07");
    match std::io::stdout()
        .write_all(seq.as_bytes())
        .and_then(|_| std::io::stdout().flush())
    {
        Ok(()) => Ok((capped.chars().count(), truncated)),
        Err(e) => Err(format!("OSC52 write: {e}")),
    }
}

pub(crate) static CLIPBOARD_HOLDER: std::sync::OnceLock<
    std::sync::Mutex<Option<arboard::Clipboard>>,
> = std::sync::OnceLock::new();

pub(crate) fn pipe_to_clipboard(prog: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: spawn failed ({e})"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{prog}: stdin write failed ({e})"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{prog}: wait failed ({e})"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{prog}: exited {:?} ({})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub(crate) fn clipboard_copy_text(text: &str) -> Result<(usize, &'static str), String> {
    let n = text.chars().count();
    let mut errs: Vec<String> = Vec::new();
    match pipe_to_clipboard("wl-copy", &[], text).and(pipe_to_clipboard(
        "wl-copy",
        &["--primary"],
        text,
    )) {
        Ok(()) => return Ok((n, "wl-copy")),
        Err(e) => errs.push(e),
    }

    match pipe_to_clipboard("xclip", &["-selection", "clipboard"], text).and(pipe_to_clipboard(
        "xclip",
        &["-selection", "primary"],
        text,
    )) {
        Ok(()) => return Ok((n, "xclip")),
        Err(e) => {
            errs.push(e);
            match pipe_to_clipboard("xsel", &["--clipboard", "--input"], text)
                .and(pipe_to_clipboard("xsel", &["--primary", "--input"], text))
            {
                Ok(()) => return Ok((n, "xsel")),
                Err(e) => errs.push(e),
            }
        }
    }
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(text.to_string()) {
            Ok(()) => {
                if let Ok(mut slot) = CLIPBOARD_HOLDER
                    .get_or_init(|| std::sync::Mutex::new(None))
                    .lock()
                {
                    *slot = Some(cb);
                }
                return Ok((n, "clipboard"));
            }
            Err(e) => errs.push(format!("arboard set_text failed ({e})")),
        },
        Err(e) => errs.push(format!("arboard unavailable ({e})")),
    }
    match osc52_copy_text(text) {
        Ok((m, t)) => {
            return Ok((
                m,
                if t {
                    "OSC52 unverified (truncated 100k)"
                } else {
                    "OSC52 unverified"
                },
            ));
        }
        Err(e) => errs.push(e),
    }
    tracing::warn!("clipboard copy failed: {}", errs.join("; "));
    Err(format!("no working clipboard ({})", errs.join("; ")))
}

pub(crate) fn read_from_clipboard(prog: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let txt = String::from_utf8_lossy(&out.stdout).to_string();
    if txt.trim().is_empty() {
        None
    } else {
        Some(txt)
    }
}

pub(crate) fn clipboard_paste_text() -> Option<String> {
    if let Some(t) = read_from_clipboard("wl-paste", &["--no-newline"]) {
        return Some(t);
    }
    if let Some(t) = read_from_clipboard("xclip", &["-o", "-selection", "clipboard"]) {
        return Some(t);
    }
    if let Some(t) = read_from_clipboard("xsel", &["--clipboard", "--output"]) {
        return Some(t);
    }

    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(txt) = cb.get_text() {
            if !txt.trim().is_empty() {
                return Some(txt);
            }
        }
    }
    None
}

pub(crate) fn clipboard_paste_image_base64() -> Option<(String, u32, u32)> {
    let rgba: Option<(u32, u32, Vec<u8>)> = if let Ok(mut cb) = arboard::Clipboard::new() {
        cb.get_image()
            .ok()
            .map(|img| (img.width as u32, img.height as u32, img.bytes.to_vec()))
    } else {
        None
    };
    let rgba = rgba.or_else(|| clipboard_image_png_bytes().and_then(|png| rgba_from_png(&png)));
    let (w, h, px) = rgba?;
    if w == 0 || h == 0 || px.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let (w, h, px) = shrink_rgba(w, h, &px, PASTE_IMAGE_MAX_DIM);
    let png_bytes = png_from_rgba(w, h, &px)?;
    if png_bytes.is_empty() {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Some((b64, w, h))
}

pub(crate) const PASTE_IMAGE_MAX_DIM: u32 = 1024;

pub(crate) fn clipboard_image_png_bytes() -> Option<Vec<u8>> {
    const CAP: usize = 20 << 20;
    for (prog, args) in [
        ("wl-paste", &["--no-newline", "-t", "image/png"][..]),
        (
            "xclip",
            &["-o", "-selection", "clipboard", "-t", "image/png"][..],
        ),
    ] {
        if let Ok(out) = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
        {
            if out.status.success()
                && !out.stdout.is_empty()
                && out.stdout.len() <= CAP
                && out.stdout.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10])
            {
                return Some(out.stdout);
            }
        }
    }
    None
}

pub(crate) fn rgba_from_png(png_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    let mut raw = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).ok()?;
    let px = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => raw,
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
            let (chunks, _) = raw.as_chunks::<3>();
            for rgb in chunks {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        }
        _ => return None,
    };
    Some((w, h, px))
}

pub(crate) fn shrink_rgba(w: u32, h: u32, px: &[u8], max_dim: u32) -> (u32, u32, Vec<u8>) {
    let m = w.max(h);
    if m <= max_dim || w == 0 || h == 0 {
        return (w, h, px.to_vec());
    }
    let scale = max_dim as f64 / m as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    for y in 0..nh {
        for x in 0..nw {
            let x0 = (x as f64 / scale) as u32;
            let x1 = (((x + 1) as f64 / scale).ceil() as u32).min(w).max(x0 + 1);
            let y0 = (y as f64 / scale) as u32;
            let y1 = (((y + 1) as f64 / scale).ceil() as u32).min(h).max(y0 + 1);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * w + sx) as usize) * 4;
                    for c in 0..4 {
                        acc[c] += px[i + c] as u64;
                    }
                    n += 1;
                }
            }
            let o = ((y * nw + x) as usize) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (nw, nh, out)
}

pub(crate) fn png_from_rgba(w: u32, h: u32, px: &[u8]) -> Option<Vec<u8>> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(px).ok()?;
    }
    if png_bytes.is_empty() {
        None
    } else {
        Some(png_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_holder_survives_for_paste() {
        let probe = "vioraharness-paste-probe";
        let Ok((_, _)) = clipboard_copy_text(probe) else {
            return;
        };
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return;
        };
        if let Ok(back) = cb.get_text() {
            if back == probe {
                assert_eq!(back, probe);
            }
        }
    }

    #[test]
    fn pipe_to_clipboard_success_and_failure() {
        assert!(pipe_to_clipboard("cat", &[], "hi").is_ok());
        assert!(pipe_to_clipboard("vioraharness-no-such-prog", &[], "hi").is_err());
    }

    #[test]
    fn osc52_copy_reports_chars() {
        let (n, via) = osc52_copy_text("hi ✓").expect("osc52 writes");
        assert_eq!(n, 4);
        assert!(!via);
    }

    #[test]
    fn shrink_rgba_averages_and_caps() {
        let px: Vec<u8> = vec![0, 0, 0, 255, 100, 0, 0, 255, 0, 100, 0, 255, 0, 0, 100, 255];
        let (w, h, out) = shrink_rgba(2, 2, &px, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![25, 25, 25, 255]);

        let (w, h, out) = shrink_rgba(2, 2, &px, 1024);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, px);

        let big = vec![128u8; 200 * 100 * 4];
        let (w, h, out) = shrink_rgba(200, 100, &big, 100);
        assert_eq!((w, h), (100, 50));
        assert_eq!(out.len(), 100 * 50 * 4);
    }

    #[test]
    fn png_encode_decode_roundtrip() {
        let px: Vec<u8> = (0..(8 * 6 * 4)).map(|i| (i % 251) as u8).collect();
        let png = png_from_rgba(8, 6, &px).expect("encodes");
        assert!(png.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        let (w, h, back) = rgba_from_png(&png).expect("decodes");
        assert_eq!((w, h), (8, 6));
        assert_eq!(back, px);

        let rgb: Vec<u8> = vec![10, 20, 30, 40, 50, 60];
        let mut rgb_png: Vec<u8> = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut rgb_png, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        let (w, h, back) = rgba_from_png(&rgb_png).expect("rgb decodes");
        assert_eq!((w, h), (2, 1));
        assert_eq!(back, vec![10, 20, 30, 255, 40, 50, 60, 255]);

        assert!(rgba_from_png(b"not a png").is_none());
        assert!(rgba_from_png(&[]).is_none());
    }
}
