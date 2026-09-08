use super::*;

pub(crate) fn bash_command(args_str: &str) -> String {
    if args_str.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(args_str) {
            if let Some(c) = v.get("command").and_then(|c| c.as_str()) {
                return c.to_string();
            }
        }
    }
    args_str.to_string()
}

pub(crate) fn redirect_writes_external(seg: &str) -> bool {
    let b = seg.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'>' {
            i += 1;
            continue;
        }

        if i + 1 < b.len() && b[i + 1] == b'&' {
            i += 2;
            continue;
        }

        let mut j = i + 1;
        if j < b.len() && b[j] == b'>' {
            j += 1;
        }
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        let mut k = j;
        while k < b.len() && !b" \t;&|<>()".contains(&b[k]) {
            k += 1;
        }
        let mut target = seg[j..k].trim().to_string();

        target = target.trim_start_matches('&').to_string();
        for q in ['\'', '"'] {
            if target.len() >= 2 && target.starts_with(q) && target.ends_with(q) {
                target = target[1..target.len() - 1].to_string();
            }
        }
        i = k.max(i + 1);
        if target.is_empty() {
            continue;
        }
        if matches!(
            target.as_str(),
            "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
        ) {
            continue;
        }
        let resolved = crate::tools::viora::resolve_path(&target);
        if !crate::tools::viora::is_within_root(&resolved) {
            return true;
        }
    }
    false
}

/// (every `2>&1` command would look compound and fail safety checks).

pub(crate) fn split_shell_segments(cmd: &str) -> Vec<&str> {
    let b = cmd.as_bytes();
    let mut segs: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];

        if c == b'\\' && i + 1 < b.len() {
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            i += 1;
            continue;
        }
        match c {
            b';' => {
                segs.push(&cmd[start..i]);
                i += 1;
                start = i;
            }
            b'|' => {
                segs.push(&cmd[start..i]);
                i += 1;
                if i < b.len() && b[i] == b'|' {
                    i += 1;
                }
                start = i;
            }
            b'&' => {
                if i + 1 < b.len() && b[i + 1] == b'&' {
                    segs.push(&cmd[start..i]);
                    i += 2;
                    start = i;
                } else {
                    let prev_gt = i > 0 && b[i - 1] == b'>';
                    let next_gt = i + 1 < b.len() && b[i + 1] == b'>';
                    if prev_gt || next_gt {
                        i += 1;
                    } else {
                        segs.push(&cmd[start..i]);
                        i += 1;
                        start = i;
                    }
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    segs.push(&cmd[start..]);
    segs.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn is_dangerous_bash(args_str: &str) -> bool {
    let raw = bash_command(args_str);
    let lower = raw.trim().to_lowercase();
    if lower.contains(":(){") || lower.contains(": (){") {
        return true;
    }

    let mut cmd = lower.as_str();
    for wrapper in ["sh -c ", "bash -c "] {
        if let Some(rest) = cmd.strip_prefix(wrapper) {
            let rest = rest.trim();
            cmd = rest
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .unwrap_or(rest);
            break;
        }
    }
    for seg in split_shell_segments(cmd) {
        if segment_is_dangerous(seg) {
            return true;
        }
    }
    false
}

pub(crate) const DANGEROUS_CMDS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "wipe", "srm", "dd", "mkfs", "wipefs", "mkswap", "fdisk",
    "parted", "shutdown", "reboot", "halt", "poweroff",
];

pub(crate) fn interpreter_danger(seg: &str) -> bool {
    const INTERP: &[&str] = &[
        "python", "python2", "python3", "node", "nodejs", "perl", "ruby", "php", "lua", "R",
        "Rscript", "deno", "bun",
    ];
    const PATTERNS: &[&str] = &[
        "os.remove",
        "os.unlink",
        "os.removedirs",
        "os.rmdir",
        "shutil.rmtree",
        "unlink",
        "rmtree",
        ".unlink(",
        "fs.rm",
        "fs.rmdir",
        "rimraf",
        "child_process",
        "file.delete",
        "fileutils",
        "shell_exec",
        "passthru",
        "os.system",
        "os.popen",
        "os.spawn",
        "subprocess.",
        "exec(",
        "system(",
        "popen(",
        "spawn(",
    ];
    let seg = strip_wrappers(seg.trim_start_matches('!').trim());
    let first = seg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches("./");
    let base = first.rsplit('/').next().unwrap_or(first);
    let is_interp =
        INTERP.contains(&base) || base.starts_with("python") || base.starts_with("node");
    if !is_interp {
        return false;
    }
    PATTERNS.iter().any(|p| seg.contains(p))
}

pub(crate) fn segment_is_dangerous(seg: &str) -> bool {
    let seg = strip_wrappers(seg.trim_start_matches('!').trim());

    let first = seg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches("./");
    let base = first.rsplit('/').next().unwrap_or(first);

    if base == "exec" {
        let raw_first = seg.split_whitespace().next().unwrap_or("");
        let rest = seg[raw_first.len()..].trim();
        if !rest.is_empty() && !rest.starts_with('>') {
            return segment_is_dangerous(rest);
        }
        return false;
    }

    if DANGEROUS_CMDS.contains(&base) || base.starts_with("mkfs.") || base.starts_with("mkfs-") {
        return true;
    }

    if (base == "systemctl" || base == "service")
        && ["poweroff", "reboot", "halt"]
            .iter()
            .any(|w| seg.contains(w))
    {
        return true;
    }

    let toks: Vec<&str> = seg.split_whitespace().collect();
    if base == "find"
        && toks
            .iter()
            .any(|t| matches!(*t, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        return true;
    }
    if base == "xargs"
        && toks[1..].iter().any(|t| {
            let b = t.trim_start_matches("./").rsplit('/').next().unwrap_or(t);
            DANGEROUS_CMDS.contains(&b)
        })
    {
        return true;
    }

    if interpreter_danger(seg) {
        return true;
    }
    false
}

pub(crate) fn strip_wrappers(mut seg: &str) -> &str {
    const WRAPPERS: &[&str] = &[
        "sudo", "nohup", "timeout", "nice", "ionice", "stdbuf", "setsid", "env", "command",
    ];
    fn is_assign(tok: &str) -> bool {
        tok.contains('=')
            && !tok.starts_with('=')
            && tok
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && tok
                .chars()
                .take_while(|c| *c != '=')
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn is_flag_or_num(tok: &str) -> bool {
        if tok.len() > 1 && tok.starts_with('-') {
            return true;
        }
        let num = match tok.strip_suffix(['s', 'm', 'h', 'd']) {
            Some(n) if !n.is_empty() => n,
            _ => tok,
        };
        !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.')
    }
    fn strip_leading_assigns(mut seg: &str) -> &str {
        loop {
            let tok_end = seg.find([' ', '\t']).unwrap_or(seg.len());
            if !is_assign(&seg[..tok_end]) {
                break;
            }
            seg = seg[tok_end..].trim();
        }
        seg
    }

    seg = strip_leading_assigns(seg);
    loop {
        let mut progress = false;
        for w in WRAPPERS {
            if let Some(rest) = seg
                .strip_prefix(w)
                .filter(|r| r.starts_with(' ') || r.starts_with('\t'))
            {
                seg = rest.trim();
                progress = true;
                break;
            }
        }

        if progress {
            loop {
                let tok_end = seg.find([' ', '\t']).unwrap_or(seg.len());
                let tok = &seg[..tok_end];
                if tok == "--" {
                    seg = seg[tok_end..].trim();
                    break;
                }
                if is_assign(tok) || is_flag_or_num(tok) {
                    seg = seg[tok_end..].trim();
                } else {
                    break;
                }
            }
        }
        if !progress {
            break;
        }
    }
    seg
}

pub(crate) fn is_safe_bash(args_str: &str) -> bool {
    let cmd = if args_str.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(args_str) {
            v.get("command")
                .and_then(|c| c.as_str())
                .unwrap_or(args_str)
                .to_string()
        } else {
            args_str.to_string()
        }
    } else {
        args_str.to_string()
    };
    let lower = cmd.trim().to_lowercase();

    let segments: Vec<&str> = split_shell_segments(&lower);

    let safe_prefixes = &[
        "ls",
        "pwd",
        "cat ",
        "echo ",
        "head ",
        "tail ",
        "wc ",
        "find ",
        "tree",
        "grep ",
        "grep -",
        "awk ",
        "sed ",
        "sort ",
        "uniq ",
        "cut ",
        "tr ",
        "seq ",
        "seq",
        "git status",
        "git log",
        "git branch",
        "git diff",
        "git show",
        "git add",
        "git commit",
        "git clone",
        "git fetch",
        "git pull",
        "git push",
        "git stash",
        "date",
        "uname",
        "env ",
        "printenv",
        "which ",
        "whereis ",
        "command ",
        "type ",
        "file ",
        "stat ",
        "du ",
        "df ",
        "free ",
        "uptime ",
        "uptime",
        "whoami",
        "id ",
        "id",
        "hostname",
        "diff ",
        "cmp ",
        "cd ",
        "cd",
        "mkdir ",
        "touch ",
        "cp ",
        "mv ",
        "chmod ",
        "ln ",
        "tar ",
        "zip ",
        "unzip ",
        "xdg-open",
        "open ",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "firefox",
        "brave",
        "sensible-browser",
        "npm run",
        "npm ",
        "npx ",
        "node ",
        "cargo run",
        "cargo check",
        "cargo test",
        "python ",
        "python3 ",
        "pytest ",
        "pytest",
        "pip ",
        "pip3 ",
        "perl ",
        "ruby ",
        "php ",
        "lua ",
        "go ",
        "make ",
        "ls ",
    ];

    let seg_is_safe = |seg: &str| {
        if redirect_writes_external(seg) {
            return false;
        }

        if interpreter_danger(seg) {
            return false;
        }
        let seg = strip_wrappers(seg);

        if seg == "find" || seg.starts_with("find ") {
            let bad = seg.split_whitespace().any(|t| {
                matches!(
                    t,
                    "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fls" | "-fprint"
                )
            });
            if bad {
                return false;
            }
        }

        if seg == "xargs" || seg.starts_with("xargs ") {
            let mut toks = seg.split_whitespace().skip(1).peekable();
            while let Some(t) = toks.peek() {
                if t.starts_with('-') {
                    let flag = toks.next().unwrap();
                    if matches!(flag, "-I" | "--replace" | "-a" | "--arg-file") {
                        toks.next();
                    }
                } else {
                    break;
                }
            }
            let rest: Vec<&str> = toks.collect();
            if rest.is_empty() {
                return true;
            }
            let raw_first = rest[0];
            let base = raw_first
                .trim_start_matches("./")
                .rsplit('/')
                .next()
                .unwrap_or(raw_first);
            let rebuilt = format!("{base}{}", &rest.join(" ")[raw_first.len()..]);
            return safe_prefixes.iter().any(|p| rebuilt.starts_with(p));
        }
        if safe_prefixes.iter().any(|p| seg.starts_with(p)) {
            return true;
        }
        let (first, rest) = match seg.find(' ') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if first.contains('/') {
            let base = first.rsplit('/').next().unwrap_or(first);
            let rebuilt = format!("{base}{rest}");
            return safe_prefixes.iter().any(|p| rebuilt.starts_with(p));
        }
        false
    };

    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|s| seg_is_safe(s))
}
