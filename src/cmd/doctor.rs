/// Run a short diagnostic probe: Some((success, stdout)) or None on spawn
/// failure / timeout. Callers check success themselves so a missing binary
/// (non-zero exit) reads as "not found". Never blocks doctor.
fn probe_output(program: &str, args: &[&str], secs: u64) -> Option<(bool, String)> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let (program, args) = (
        program.to_string(),
        args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    std::thread::spawn(move || {
        let out = std::process::Command::new(&program).args(&args).output();
        let _ = tx.send(out.ok());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
        Ok(Some(out)) => Some((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
        )),
        _ => None,
    }
}

pub(crate) async fn cmd_doctor(config: Option<String>) -> anyhow::Result<()> {
    println!("VioraHarness doctor");

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Binary resolution mirrors `resolve_viora`: explicit `VIORA_BIN` first,
    // otherwise the first `viora` on PATH. No checkout/build/output paths —
    // the harness invokes the installed binary and never enters source trees.
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(bin) = std::env::var("VIORA_BIN") {
        if !bin.trim().is_empty() {
            candidates.push(bin);
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let p = format!("{dir}/viora");
            if std::path::Path::new(&p).exists() {
                candidates.push(p);
                break;
            }
        }
    }
    candidates.push("viora".into());
    let mut found = None;
    for c in candidates {
        if std::path::Path::new(&c).exists() || c == "viora" {
            if let Ok(out) = std::process::Command::new(&c).arg("--version").output() {
                if out.status.success() {
                    found = Some((c, String::from_utf8_lossy(&out.stdout).trim().to_string()));
                    break;
                }
            }
        }
    }
    match found {
        Some((p, v)) => println!("  viora: {p} ({v})"),
        None => println!("  viora: not found (set VIORA_BIN or put `viora` on PATH)"),
    }

    // Examples live in the working project, or in an explicitly exported
    // `VIOSPICE_ROOT` checkout — never a hardcoded home-directory path.
    let cwd_examples = cwd.join("examples");
    let env_examples = std::env::var("VIOSPICE_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|r| format!("{}/examples", r.trim_end_matches('/')));
    let demo = if cwd_examples.exists() {
        cwd_examples.to_string_lossy().to_string()
    } else {
        env_examples.unwrap_or_else(|| cwd_examples.to_string_lossy().to_string())
    };
    println!(
        "  examples: {} ({})",
        if std::path::Path::new(&demo).exists() {
            "present"
        } else {
            "missing"
        },
        demo
    );
    let open_set = std::env::var("OPENROUTER_API_KEY")
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    let gem_set = std::env::var("GEMINI_API_KEY")
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let mask = |k: String| {
        if k.len() <= 8 {
            format!("{}…", &k[..k.len().min(4)])
        } else {
            format!("{}…{}", &k[..4], &k[k.len() - 4..])
        }
    };
    if open_set {
        let k = std::env::var("OPENROUTER_API_KEY").unwrap();
        println!("  openrouter: set ({}) — use /providers to manage", mask(k));

        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        {
            match client
                .get("https://openrouter.ai/api/v1/models")
                .header(
                    "Authorization",
                    format!("Bearer {}", std::env::var("OPENROUTER_API_KEY").unwrap()),
                )
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        println!("    → validated ✓ {} models", cnt);
                    }
                }
                Ok(resp) => println!("    → validation ✗ {} (check key)", resp.status()),
                Err(e) => println!("    → validation skipped ({e})"),
            }
        }
    } else {
        println!("  openrouter: missing (export OPENROUTER_API_KEY or /providers)");
    }
    if gem_set {
        let k = std::env::var("GEMINI_API_KEY").unwrap();
        println!("  gemini:     set ({})", mask(k.clone()));

        println!("    → debug: curl direct API isolation (like tui live fetch)");
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            let url1 = "https://generativelanguage.googleapis.com/v1beta/openai/models";
            println!(
                "      curl -H \"Authorization: Bearer $GEMINI_API_KEY\" {}",
                url1
            );
            match client
                .get(url1)
                .header("Authorization", format!("Bearer {k}"))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let sample: Vec<String> = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .take(5)
                                    .filter_map(|v| {
                                        v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        println!(
                            "        ✓ openai compat {} models — sample {:?}",
                            cnt, sample
                        );
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    println!(
                        "        ✗ openai compat {} — {}",
                        status,
                        txt.chars().take(200).collect::<String>()
                    );
                }
                Err(e) => println!("        ✗ openai compat request failed: {}", e),
            }

            let url2 = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000";
            println!(
                "      curl -H \"x-goog-api-key: $GEMINI_API_KEY\" \"{}\"",
                url2
            );
            match client.get(url2).header("x-goog-api-key", &k).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("models")
                            .and_then(|m| m.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let has36 = j
                            .get("models")
                            .and_then(|m| m.as_array())
                            .map(|a| {
                                a.iter().any(|v| {
                                    v.get("name")
                                        .and_then(|n| n.as_str())
                                        .map(|s| s.contains("3.6"))
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                        println!(
                            "        ✓ native header {} models — has 3.6={} nextToken={}",
                            cnt,
                            has36,
                            j.get("nextPageToken")
                                .and_then(|t| t.as_str())
                                .map(|s| !s.is_empty())
                                .unwrap_or(false)
                        );
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    println!(
                        "        ✗ native header {} — {}",
                        status,
                        txt.chars().take(200).collect::<String>()
                    );
                }
                Err(e) => println!("        ✗ native header request failed: {}", e),
            }

            let url3 = format!(
                "https://generativelanguage.googleapis.com/v1beta/models?key={k}&pageSize=5"
            );
            println!("      curl \"https://generativelanguage.googleapis.com/v1beta/models?key=\\$GEMINI_API_KEY&pageSize=5\"");
            match client.get(&url3).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("models")
                            .and_then(|m| m.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        println!(
                            "        ✓ native query param {} models (pageSize=5 sample)",
                            cnt
                        );
                    }
                }
                Ok(resp) => println!("        ✗ native query {} ", resp.status()),
                Err(e) => println!("        ✗ native query failed: {}", e),
            }
        }
    } else {
        println!("  gemini:     missing (export GEMINI_API_KEY or /providers)");
    }
    let gateway_set = std::env::var("OPENCODE_API_KEY")
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
        || std::env::var("ZEN_API_KEY")
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false);
    if gateway_set {
        let k = std::env::var("OPENCODE_API_KEY")
            .or_else(|_| std::env::var("ZEN_API_KEY"))
            .unwrap();
        println!(
            "  gateway:    set ({}) — managed pay-per-use + subscription tiers",
            mask(k.clone())
        );
        println!("    → debug: curl direct gateway isolation (like tui live fetch)");
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            let url_z = "https://opencode.ai/zen/v1/models";
            println!(
                "      curl -H \"Authorization: Bearer $OPENCODE_API_KEY\" {}",
                url_z
            );
            match client
                .get(url_z)
                .header("Authorization", format!("Bearer {k}"))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let sample: Vec<String> = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .take(4)
                                    .filter_map(|v| {
                                        v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let free: Vec<String> = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| {
                                        v.get("id")
                                            .and_then(|x| x.as_str())
                                            .filter(|s| s.contains("free"))
                                            .map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        println!(
                            "        ✓ gateway {} models — sample {:?} — free {:?}",
                            cnt, sample, free
                        );
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    println!(
                        "        ✗ gateway {} — {}",
                        status,
                        txt.chars().take(200).collect::<String>()
                    );
                }
                Err(e) => println!("        ✗ gateway request failed: {}", e),
            }
            let url_g = "https://opencode.ai/zen/go/v1/models";
            println!(
                "      curl -H \"Authorization: Bearer $OPENCODE_API_KEY\" {}",
                url_g
            );
            match client
                .get(url_g)
                .header("Authorization", format!("Bearer {k}"))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let sample: Vec<String> = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .take(4)
                                    .filter_map(|v| {
                                        v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        println!("        ✓ go {} models — sample {:?}", cnt, sample);
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    println!(
                        "        ✗ go {} — {}",
                        status,
                        txt.chars().take(200).collect::<String>()
                    );
                }
                Err(e) => println!("        ✗ go request failed: {}", e),
            }
        }
    } else {
        println!("  gateway:    no key — free-tier (*-free) models work with public");
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        {
            match client.get("https://opencode.ai/zen/v1/models").send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(j) = resp.json::<serde_json::Value>().await {
                        let cnt = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let free: Vec<String> = j
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| {
                                        v.get("id")
                                            .and_then(|x| x.as_str())
                                            .filter(|s| s.contains("free"))
                                            .map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        println!("    → gateway public ✓ {} models — free {:?}", cnt, free);
                        if let Some(first) = free.first() {
                            let id = if first.contains('/') {
                                first.clone()
                            } else {
                                format!("opencode/{first}")
                            };
                            println!("    → try: vioraharness run \"hi\" --model {id}  (no key needed, public)");
                        }
                    }
                }
                _ => {}
            }
        }
        println!("    → for paid tiers: get a key at https://opencode.ai/auth, then add it via TUI /providers or export OPENCODE_API_KEY=...");
    }

    let env_path = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let h = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(h).join(".config")
        })
        .join("vioraharness/.env");
    println!(
        "  providers env: {} ({})",
        env_path.display(),
        if env_path.exists() {
            "present"
        } else {
            "not yet — /providers will create"
        }
    );
    println!(
        "  config:     {}",
        config.unwrap_or_else(|| "vioraharness.json (hierarchical)".into())
    );
    for cand in vioraharness_core::loop_mod::config_candidates() {
        if std::path::Path::new(&cand).exists() {
            println!("  config file: {cand} (found)");
            if let Ok(s) = std::fs::read_to_string(&cand) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(perms) = v.get("permissions") {
                        println!(
                            "    permissions: {} rules",
                            perms.as_object().map(|o| o.len()).unwrap_or(0)
                        );
                    }
                    if let Some(prov) = v.get("provider") {
                        println!(
                            "    provider keys: {}",
                            prov.as_object()
                                .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                                .unwrap_or_default()
                        );
                    }
                }
            }
            break;
        }
    }
    println!("  desktop/x11:");
    {
        let display = std::env::var("DISPLAY").unwrap_or_default();
        if display.trim().is_empty() {
            println!("    DISPLAY: unset (headless-only; browser_screenshot still works)");
        } else {
            let sock = display
                .trim_start_matches(':')
                .split('.')
                .next()
                .unwrap_or("");
            let sock_path = format!("/tmp/.X11-unix/X{sock}");
            let visible = std::path::Path::new(&sock_path).exists();
            println!(
                "    DISPLAY={display} socket {sock_path} {}",
                if visible {
                    "visible ✓"
                } else {
                    "hidden ✗ (sandbox masks it — use browser_screenshot headless or browser_open host launcher)"
                }
            );
        }
        let xauth_env = std::env::var("XAUTHORITY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let fallback = format!("{home}/.Xauthority");
        let shim = "/tmp/vioraharness-xauth/Xauthority";
        println!(
            "    XAUTHORITY env: {}",
            xauth_env
                .as_deref()
                .unwrap_or("(unset — defaults to ~/.Xauthority)")
        );
        for p in [fallback.as_str(), shim] {
            println!(
                "      {p}: {}",
                if std::path::Path::new(p).exists() {
                    "present ✓"
                } else {
                    "missing"
                }
            );
        }
        let which = |bin: &str| {
            std::env::var("PATH")
                .map(|path| {
                    path.split(':').any(|d| {
                        !d.is_empty() && std::path::Path::new(&format!("{d}/{bin}")).exists()
                    })
                })
                .unwrap_or(false)
        };
        for bin in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ] {
            if which(bin) {
                println!("    chrome: {bin} found ✓");
                break;
            }
        }
        if ![
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ]
        .iter()
        .any(|b| which(b))
        {
            println!("    chrome: not found (install google-chrome/chromium or set CHROME_BIN)");
        }
        println!(
            "    xdotool: {}",
            if which("xdotool") {
                "present ✓"
            } else {
                "missing (optional)"
            }
        );
        println!(
            "    xwininfo: {}",
            if which("xwininfo") {
                "present ✓"
            } else {
                "missing (optional)"
            }
        );
        println!(
            "    wmctrl: {}",
            if which("wmctrl") {
                "present ✓"
            } else {
                "missing (optional — not bundled; use `xdotool search --onlyvisible --name <title>` or `xwininfo -root -tree` instead)"
            }
        );
    }
    {
        println!("  android:");
        let adb = vioraharness_core::tools::android::resolve_adb();
        match probe_output(&adb, &["--version"], 10) {
            Some((true, out)) => {
                let first = out.lines().next().unwrap_or("").trim();
                println!("    adb: {adb} ({first})");
            }
            _ => println!(
                "    adb: not found (set ADB_BIN or ANDROID_HOME/ANDROID_SDK_ROOT, or put platform-tools on PATH)"
            ),
        }
        match vioraharness_core::tools::android::sdk_root() {
            Some(r) => println!("    sdk: {r} (from ANDROID_HOME/ANDROID_SDK_ROOT)"),
            None => println!(
                "    sdk: no ANDROID_HOME/ANDROID_SDK_ROOT (adb/emulator resolve via PATH)"
            ),
        }
        let emu = vioraharness_core::tools::android::resolve_emulator();
        let emu_ok = std::path::Path::new(&emu).exists() || {
            std::env::var("PATH")
                .map(|p| {
                    p.split(':').any(|d| {
                        !d.is_empty() && std::path::Path::new(&format!("{d}/emulator")).exists()
                    })
                })
                .unwrap_or(false)
        };
        if emu_ok {
            match probe_output(&emu, &["-list-avds"], 15) {
                Some((true, out)) => {
                    let avds: Vec<&str> = out
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect();
                    println!(
                        "    emulator: {emu} — AVDs: {}",
                        if avds.is_empty() {
                            "(none)".into()
                        } else {
                            avds.join(", ")
                        }
                    );
                }
                _ => println!("    emulator: {emu} (AVD list timed out)"),
            }
        } else {
            println!("    emulator: not found ($SDK/emulator/emulator or PATH)");
        }
        // Fast device poll; never fails doctor.
        match probe_output(&adb, &["devices", "-l"], 10) {
            Some((true, out)) => {
                let devs = vioraharness_core::tools::android::parse_devices(&out);
                if devs.is_empty() {
                    println!("    devices: none attached (start an emulator or plug one in)");
                } else {
                    let list = devs
                        .iter()
                        .map(|d| format!("{} ({})", d.serial, d.state))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("    devices: {list}");
                }
            }
            _ => println!("    devices: (adb query failed or timed out)"),
        }
        let gradle_probe = ["gradle", "java"];
        for bin in gradle_probe {
            let found = std::env::var("PATH")
                .map(|p| {
                    p.split(':').any(|d| {
                        !d.is_empty() && std::path::Path::new(&format!("{d}/{bin}")).exists()
                    })
                })
                .unwrap_or(false);
            println!("    {bin}: {}", if found { "found ✓" } else { "missing" });
        }
    }
    let (mode, mode_src) = vioraharness_core::mode::resolve_mode_with_source(None);
    let mode_tools = vioraharness_core::mode::registry_for_mode(&mode)
        .all()
        .len();
    println!("  mode: {mode} (from {mode_src}, {mode_tools} tools) — override with --mode <name> or /mode");
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    println!("  db: {db}");
    match vioraharness_core::session::SessionStore::new(&db) {
        Ok(store) => {
            let list = store.list_sessions().unwrap_or_default();
            println!("    sessions: {}", list.len());
        }
        Err(e) => println!("    db error: {e}"),
    }
    Ok(())
}
