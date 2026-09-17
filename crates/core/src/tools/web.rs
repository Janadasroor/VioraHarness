// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde_json::{json, Value};

const UA: &str = "VioraHarness/0.1 (+coding-agent; docs fetch)";
const SEARCH_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
const SEARCH_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const FETCH_TIMEOUT_SECS: u64 = 20;
const DEFAULT_MAX_CHARS: usize = 30_000;
const HARD_MAX_CHARS: usize = 100_000;

pub fn validate_url(url: &str) -> Result<String, String> {
    let t = url.trim();
    if t.is_empty() {
        return Err("empty url".into());
    }
    let lower = t.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!(
            "refused non-http(s) url (scheme not allowed): {}…",
            t.chars().take(60).collect::<String>()
        ));
    }
    if t.chars().count() > 2000 {
        return Err("url too long".into());
    }
    if let Some(host) = url_host(&lower) {
        if is_blocked_host(&host) {
            return Err(format!("refused blocked host: {host}"));
        }
    }
    Ok(t.to_string())
}

fn url_host(lower_url: &str) -> Option<String> {
    let after_scheme = lower_url.split("://").nth(1)?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let hostport = authority.rsplit('@').next().unwrap_or(authority);

    if let Some(rest) = hostport.strip_prefix('[') {
        return rest.split(']').next().map(|s| s.to_string());
    }

    let host = match hostport.rfind(':') {
        Some(i) if hostport[i + 1..].chars().all(|c| c.is_ascii_digit()) => &hostport[..i],
        _ => hostport,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.trim_end_matches('.').to_string())
}

fn parse_ipv4(host: &str) -> Option<u32> {
    fn part(s: &str) -> Option<u32> {
        if s.is_empty() {
            return None;
        }
        if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            if h.is_empty() || h.len() > 8 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            return u32::from_str_radix(h, 16).ok();
        }
        if s.len() > 1 && s.starts_with('0') {
            if !s.chars().all(|c| matches!(c, '0'..='7')) {
                return None;
            }
            return u32::from_str_radix(s, 8).ok();
        }
        if !s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok()
    }
    if host.contains('.') {
        let parts: Vec<&str> = host.split('.').collect();
        let nums: Option<Vec<u32>> = parts.iter().map(|p| part(p)).collect();
        let nums = nums?;
        return match nums.len() {
            2 if nums[0] <= 255 && nums[1] <= 0xffffff => Some((nums[0] << 24) | nums[1]),
            3 if nums[0] <= 255 && nums[1] <= 255 && nums[2] <= 0xffff => {
                Some((nums[0] << 24) | (nums[1] << 16) | nums[2])
            }
            4 if nums.iter().all(|n| *n <= 255) => {
                Some((nums[0] << 24) | (nums[1] << 16) | (nums[2] << 8) | nums[3])
            }
            _ => None,
        };
    }

    if host.starts_with("0x") || host.starts_with("0X") {
        let h = &host[2..];
        if h.is_empty() || h.len() > 8 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return u32::from_str_radix(h, 16).ok();
    }
    if host.len() > 1 && host.starts_with('0') {
        if !host.chars().all(|c| matches!(c, '0'..='7')) {
            return None;
        }
        return u32::from_str_radix(host, 8).ok();
    }
    if host.chars().all(|c| c.is_ascii_digit()) {
        return host.parse::<u32>().ok();
    }
    None
}

fn is_blocked_host(host: &str) -> bool {
    if host == "metadata.google.internal" || host == "metadata.google.internal." {
        return true;
    }
    if host.starts_with('[') {
        return false;
    }

    if host.contains(':') {
        let h = host.to_lowercase();
        return h.starts_with("fe80:") || h.starts_with("fe90:") || h.starts_with("fea");
    }
    match parse_ipv4(host) {
        Some(ip) => {
            let link_local = (169u32 << 24) | (254u32 << 16);
            (ip & 0xffff0000) == link_local || (ip & 0xf0000000) == 0xe0000000
        }
        None => false,
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let mut ent = String::new();
        let mut closed = false;
        for _ in 0..32 {
            match it.next() {
                Some(';') => {
                    closed = true;
                    break;
                }
                Some(ch) if ch.is_alphanumeric() || ch == '#' || ch == 'x' || ch == 'X' => {
                    ent.push(ch)
                }
                Some(ch) => {
                    ent.push(ch);
                    break;
                }
                None => break,
            }
        }
        if !closed {
            out.push('&');
            out.push_str(&ent);
            continue;
        }
        match ent.as_str() {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" | "#39" => out.push('\''),
            "nbsp" => out.push(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                if let Ok(n) = u32::from_str_radix(ent.trim_start_matches(['#', 'x', 'X']), 16) {
                    out.push(char::from_u32(n).unwrap_or('\u{FFFD}'));
                } else {
                    out.push_str(&format!("&{ent};"));
                }
            }
            _ if ent.starts_with('#') => {
                if let Ok(n) = ent[1..].parse::<u32>() {
                    out.push(char::from_u32(n).unwrap_or('\u{FFFD}'));
                } else {
                    out.push_str(&format!("&{ent};"));
                }
            }
            _ => {
                out.push_str(&format!("&{ent};"));
            }
        }
    }
    out
}

pub fn html_to_text(html: &str) -> String {
    fn find_insensitive(hay: &str, needle: &[u8]) -> Option<usize> {
        let b = hay.as_bytes();
        if needle.is_empty() || needle.len() > b.len() {
            return None;
        }
        (0..=b.len() - needle.len()).find(|&i| b[i..i + needle.len()].eq_ignore_ascii_case(needle))
    }
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "head", "iframe"] {
        loop {
            let open_needle = format!("<{tag}");
            let open = match find_insensitive(&s, open_needle.as_bytes()) {
                Some(i) => i,
                None => break,
            };
            let close_needle = format!("</{tag}>");
            let end = match find_insensitive(&s[open..], close_needle.as_bytes()) {
                Some(i) => open + i + close_needle.len(),
                None => break,
            };
            s.replace_range(open..end, " ");
        }
    }

    while let Some(a) = s.find("<!--") {
        if let Some(b) = s[a..].find("-->") {
            s.replace_range(a..a + b + 3, " ");
        } else {
            s.truncate(a);
            break;
        }
    }

    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    let mut link_stack: Vec<Option<String>> = Vec::new();
    while i < s.len() {
        if s.as_bytes()[i] == b'<' {
            if let Some(end_rel) = find_tag_end(&s, i) {
                let end = end_rel - i;
                let tag = s[i + 1..i + end].trim().to_lowercase();
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '!')
                    .collect();
                let closing = tag.starts_with('/');
                if !closing && (name == "a") {
                    link_stack.push(extract_href(&s[i + 1..i + end]));
                } else if closing && name == "a" {
                    if let Some(href) = link_stack.pop().flatten() {
                        if href.starts_with("http://") || href.starts_with("https://") {
                            out.push_str(&format!(" ({href})"));
                        }
                    }
                    out.push(' ');
                } else if [
                    "br",
                    "hr",
                    "tr",
                    "li",
                    "p",
                    "div",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "section",
                    "article",
                    "table",
                    "pre",
                    "blockquote",
                    "ul",
                    "ol",
                    "header",
                    "footer",
                    "nav",
                    "td",
                    "th",
                ]
                .contains(&name.as_str())
                {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                i += end + 1;
                continue;
            }

            out.push('<');
            i += 1;
            continue;
        }

        let c = s[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(c);
        i += c.len_utf8();
    }

    let decoded = decode_entities(&out);
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            if !matches!(lines.last(), Some(s) if s.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(collapsed);
        }
    }

    while lines.first().map(|l| l.is_empty()).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

fn find_tag_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start + 1;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == b'"' || c == b'\'' {
            quote = Some(c);
        } else if c == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn extract_href(tag_inner: &str) -> Option<String> {
    let b = tag_inner.as_bytes();
    let mut i = 0;
    while i + 4 <= b.len() {
        if b[i..i + 4].eq_ignore_ascii_case(b"href") {
            let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-');
            let mut j = i + 4;
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            if prev_ok && j < b.len() && b[j] == b'=' {
                j += 1;
                while j < b.len() && (b[j] as char).is_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return None;
                }

                let decode = |raw: &str| percent_decode(&decode_entities(raw));
                let q = b[j];
                if q == b'"' || q == b'\'' {
                    let rest = &tag_inner[j + 1..];
                    if let Some(end) = rest.find(q as char) {
                        return Some(decode(&rest[..end]));
                    }
                    return None;
                }
                let mut k = j;
                while k < b.len() && !(b[k] as char).is_whitespace() && b[k] != b'>' {
                    k += 1;
                }
                if k > j {
                    return Some(decode(&tag_inner[j..k]));
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            out.push(' ');
        } else {
            out.push(b[i] as char);
        }
        i += 1;
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn truncate_chars_capped(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    (s.chars().take(max).collect(), true)
}

pub async fn webfetch(url: &str, max_chars: Option<usize>) -> Value {
    let url = match validate_url(url) {
        Ok(u) => u,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let cap = max_chars
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(100, HARD_MAX_CHARS);
    let client = match reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("http client: {e}")}),
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": format!("fetch failed: {e}")}),
    };
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return json!({"ok": false, "error": format!("HTTP {status} for {url}")});
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let final_url = resp.url().to_string();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return json!({"ok": false, "error": format!("read body: {e}")}),
    };

    const MAX_BYTES: usize = 4 * 1024 * 1024;
    if bytes.len() > MAX_BYTES {
        return json!({"ok": false, "error": format!("page too large ({} bytes)", bytes.len())});
    }
    let raw = String::from_utf8_lossy(&bytes);
    let (text, is_json) = if ctype.contains("json") {
        (raw.to_string(), true)
    } else if ctype.contains("html")
        || (!ctype.contains("text") && raw.trim_start().starts_with('<'))
    {
        (html_to_text(&raw), false)
    } else {
        (raw.to_string(), false)
    };
    let (out, truncated) = truncate_chars_capped(&text, cap);
    json!({
        "ok": true,
        "url": final_url,
        "status": status,
        "content_type": ctype,
        "is_json": is_json,
        "chars": text.chars().count(),
        "truncated": truncated,
        "content": out,
    })
}

pub fn parse_ddg(html: &str) -> Vec<(String, String, String)> {
    fn anchors_with_class(html: &str, class: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = html;
        let needle = format!("class=\"{class}\"");
        let needle2 = format!("class='{class}'");
        while let Some(pos) = rest.find(&needle).or_else(|| rest.find(&needle2)) {
            let before = &rest[..pos];
            let a_start = match before.rfind("<a") {
                Some(i) => i,
                None => {
                    rest = &rest[pos + needle.len()..];
                    continue;
                }
            };
            let tag_end = match rest[a_start..].find('>') {
                Some(i) => a_start + i,
                None => break,
            };
            let close = match rest[tag_end..].find("</a>") {
                Some(i) => tag_end + i,
                None => break,
            };
            let inner = &rest[tag_end + 1..close];
            let href = extract_href(&rest[a_start + 2..tag_end]).unwrap_or_default();

            let title = html_to_text(inner)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push((href, title));
            rest = &rest[close + 4..];
        }
        out
    }
    let links = anchors_with_class(html, "result__a");
    let snippets = anchors_with_class(html, "result__snippet");
    let mut out = Vec::new();
    for (i, (href, title)) in links.into_iter().enumerate() {
        if title.is_empty() {
            continue;
        }
        let url = unwrap_ddg(href);
        if url.is_empty() || url.contains("duckduckgo.com/y.js") {
            continue;
        }
        let snippet = snippets.get(i).map(|(_, s)| s.clone()).unwrap_or_default();
        out.push((title, url, snippet));
    }
    out
}

fn unwrap_ddg(href: String) -> String {
    if !(href.contains("duckduckgo.com/l/") || href.starts_with("//duckduckgo.com/l/")) {
        return href;
    }

    if let Some(i) = href.find("uddg=") {
        let rest = &href[i + 5..];

        let end = rest.find("&rut=").unwrap_or(rest.len());
        let end = rest[..end].find("&hov=").map(|j| j.min(end)).unwrap_or(end);
        return percent_decode(&rest[..end]);
    }
    href
}

pub fn is_ddg_error_page(html: &str) -> bool {
    html.contains("error-lite@duckduckgo.com")
        || html.contains("error-lite+")
        || (html.len() < 2000 && html.contains("email us") && !html.contains("result__a"))
}

pub async fn websearch(query: &str, count: Option<usize>) -> Value {
    let q = query.trim();
    if q.is_empty() {
        return json!({"ok": false, "error": "empty query"});
    }
    if q.chars().count() > 500 {
        return json!({"ok": false, "error": "query too long (max 500 chars)"});
    }
    let n = count.unwrap_or(8).clamp(1, 10);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static(SEARCH_ACCEPT),
    );
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
    );
    let client = match reqwest::Client::builder()
        .user_agent(SEARCH_UA)
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("http client: {e}")}),
    };
    let resp = match client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", q)])
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return json!({"ok": false, "error": format!("search failed: {e}. Hint: check network/proxy.")})
        }
    };
    if !resp.status().is_success() {
        return json!({"ok": false, "error": format!("search HTTP {} from html.duckduckgo.com — the search endpoint is rate-limiting/blocking this network. Hint: retry later, or webfetch a known URL directly.", resp.status())});
    }
    let html = match resp.text().await {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": format!("read results: {e}")}),
    };
    if is_ddg_error_page(&html) {
        return json!({"ok": false, "error": "search blocked by the results provider (bot check). Hint: retry later, or webfetch a known URL directly."});
    }
    let results: Vec<Value> = parse_ddg(&html)
        .into_iter()
        .take(n)
        .map(|(title, url, snippet)| json!({"title": title, "url": url, "snippet": snippet}))
        .collect();
    json!({"ok": true, "query": q, "count": results.len(), "results": results})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_blocklist() {
        for u in [
            "http://169.254.169.254/latest/meta-data/",
            "http://2852039166/",
            "http://0xA9FEA9FE/",
            "http://0251.0376.0251.0376/",
            "http://0xA9.0xFE.0xA9.0xFE/",
            "http://169.254.169.254./x",
            "http://metadata.google.internal/a",
            "http://[fe80::1]/",
            "https://169.254.10.20/",
        ] {
            assert!(validate_url(u).is_err(), "blocked: {u}");
        }

        for u in [
            "https://example.com/docs",
            "http://localhost:8077/api/simulate",
            "http://127.0.0.1:4096/health",
            "http://[::1]:8080/",
            "http://192.168.1.50:3000/",
            "http://10.0.0.5/",
            "http://user:pass@example.com:8080/x",
        ] {
            assert!(validate_url(u).is_ok(), "allowed: {u}");
        }
    }

    #[test]
    fn url_validation() {
        assert!(validate_url("https://example.com/x").is_ok());
        assert!(validate_url("http://a.b/").is_ok());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("data:text/plain,hi").is_err());
        assert!(validate_url("  ").is_err());
        assert!(validate_url("HTTPS://EXAMPLE.COM").is_ok());
    }

    #[test]
    fn html_to_text_basics() {
        let html = r#"<html><head><title>T</title><style>.x{}</style></head><body>
<script>alert(1)</script>
<h1>Hello <b>World</b></h1>
<p>See <a href="https://example.com/d">docs</a> for more.</p>
<!-- comment -->
<br>line2 &amp; &lt;tag&gt; &#65; &#x42;
</body></html>"#;
        let t = html_to_text(html);
        assert!(!t.contains("alert"), "{t}");
        assert!(!t.contains("comment"), "{t}");
        assert!(t.contains("Hello World"), "{t}");
        assert!(t.contains("docs (https://example.com/d)"), "{t}");
        assert!(t.contains("line2 & <tag> A B"), "{t}");

        assert!(!t.contains("\n\n\n"), "{t:?}");
        assert_eq!(t, t.trim());
    }

    #[test]
    fn ddg_parse_fixture() {
        let html = r#"<div>
<h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=xx">First <b>Result</b></a></h2>
<a class="result__snippet" href="x">snippet one here</a>
<h2 class="result__title"><a rel="nofollow" class="result__a" href="https://plain.example/b">Second</a></h2>
<h2 class="result__title"><a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=x">Ad Title</a></h2>
</div>"#;
        let r = parse_ddg(html);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0].0, "First Result");
        assert_eq!(r[0].1, "https://example.com/a");
        assert_eq!(r[0].2, "snippet one here");
        assert_eq!(r[1].1, "https://plain.example/b");
    }

    #[test]
    fn unwrap_ddg_links() {
        assert_eq!(
            unwrap_ddg("//duckduckgo.com/l/?uddg=https%3A%2F%2Fx.y%2Fz%3Fq%3D1&rut=ab".into()),
            "https://x.y/z?q=1"
        );
        assert_eq!(
            unwrap_ddg("https://example.com/keep".into()),
            "https://example.com/keep"
        );
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("a%20b+c%2F"), "a b c/");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn ddg_error_page_detection() {
        let blocked = "If this persists, please <a href=\"mailto:error-lite+4a8a@duckduckgo.com?subject=Error\">email us</a>.";
        assert!(is_ddg_error_page(blocked));
        let ok = r#"<a rel="nofollow" class="result__a" href="https://example.com/">Hi</a>"#;
        assert!(!is_ddg_error_page(ok));
        assert!(!is_ddg_error_page(""));
    }
}
