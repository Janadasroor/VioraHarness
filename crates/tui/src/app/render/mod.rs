mod chat;
mod chrome;
mod dialogs;

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::app::testkit::*;
    use crate::theme::Theme;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Span;
    use unicode_width::UnicodeWidthStr;
    fn long_tool_app() -> (App, String) {
        let mut app = test_app();
        let body = serde_json::json!({
            "ok": false,
            "error": (0..20).map(|i| format!("e{i:02}")).collect::<Vec<_>>().join("\n"),
        })
        .to_string();
        let mut m = Msg::new("assistant", "running");
        m.items.push(Content::ToolCall {
            id: "call_bash_1".into(),
            name: "bash".into(),
            args: "{\"command\": \"noisy-fail\"}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_bash_1".into(),
            content: body.clone(),
            ok: false,
        });
        app.messages.push(m);
        (app, body)
    }

    #[test]
    fn long_tool_output_collapses_with_hint() {
        let (mut app, _) = long_tool_app();
        let text = render_text(&mut app, 100, 40);
        assert!(text.contains("e00"), "head shown");
        assert!(!text.contains("e19"), "tail hidden");
        assert!(text.contains("Ctrl+X to expand"), "hint shown");
        assert!(text.contains("+12 more lines"), "remaining count");
    }

    #[test]
    fn ctrl_x_expands_most_recent_tool_output() {
        let (mut app, _) = long_tool_app();
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("e19"), "expanded reveals all");
        assert!(!text.contains("Ctrl+X to expand"), "hint gone when open");
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(!text.contains("e19"), "toggles back to collapsed");
    }

    #[test]
    fn read_tool_card_shows_only_line_counts() {
        fn read_preview(content: serde_json::Value) -> String {
            let app = test_app();
            let items = vec![Content::ToolCall {
                id: "call_read_9".into(),
                name: "read".into(),
                args: "{}".into(),
                status: ToolStatus::Done,
            }];
            app.tool_result_preview(&items, "call_read_9", &content.to_string(), true)
                .expect("read always previews")
        }
        let full = serde_json::json!({
            "ok": true, "path": "deck.cir",
            "content": "R1 1 0 1k\nC1 1 0 1u\n", "total_lines": 2,
        });
        assert_eq!(read_preview(full), "deck.cir (2 lines)");
        let paged = serde_json::json!({
            "ok": true, "path": "deck.cir",
            "content": "R1 1 0 1k\nC1 1 0 1u\nR2 2 0 2k\n",
            "offset": 10, "limit": 20, "total_lines": 100,
        });
        assert_eq!(read_preview(paged), "deck.cir (lines 11–13 of 100)");
        let bare = serde_json::json!({
            "ok": true, "path": "deck.cir", "content": "R1 1 0 1k\n",
        });
        let preview = read_preview(bare);
        assert!(!preview.contains("R1"), "no content leak: {preview}");
        let empty = serde_json::json!({
            "ok": true, "path": "empty.txt", "content": "", "total_lines": 0,
        });
        assert_eq!(read_preview(empty), "empty.txt (0 lines)");
        // Chat render carries no read content either.
        let mut app = test_app();
        let mut m = Msg::new("assistant", "reading");
        m.items.push(Content::ToolCall {
            id: "call_read_9".into(),
            name: "read".into(),
            args: "{}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_read_9".into(),
            content: full_content(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("R1 1 0"), "chat shows no read content");
        assert!(text.contains("50 lines"), "chat shows the count");
    }

    fn full_content() -> String {
        let body = (0..50)
            .map(|i| format!("R{i} 1 0 1k"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::json!({
            "ok": true, "path": "deck.cir", "content": body, "total_lines": 50,
        })
        .to_string()
    }

    #[test]
    fn ctrl_x_prefers_tool_output_over_earlier_user_text() {
        let (mut app, _) = long_tool_app();
        let long = (0..20)
            .map(|i| format!("u{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut m0 = Msg::new("user", long);
        m0.timestamp = "04:12".into();
        app.messages.insert(0, m0);
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 80);
        assert!(text.contains("e19"), "later tool output wins");
        assert!(!text.contains("u19"), "earlier user text stays collapsed");
    }
    #[tokio::test]
    async fn chat_pageup_scrolls_view() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let before = render_text(&mut app, 80, 24);
        assert!(before.contains("line 059"), "tail visible at bottom");
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 10);
        let after = render_text(&mut app, 80, 24);
        assert!(!after.contains("line 059"), "pageup moved tail out of view");
        assert!(after.contains("line 049"), "pageup shows earlier lines");

        app.handle_key(KeyCode::PageDown).await.unwrap();
        assert_eq!(app.scroll, 0);
        let back = render_text(&mut app, 80, 24);
        assert!(back.contains("line 059"), "pagedown returns to tail");
    }

    #[tokio::test]
    async fn chat_holds_position_on_new_content() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let _ = render_text(&mut app, 80, 24);
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 30);
        let first = render_text(&mut app, 80, 24);

        let first_row = first.lines().nth(2).unwrap_or("").to_string();

        push_lines(&mut app, 5, 60);
        let second = render_text(&mut app, 80, 24);
        let second_row = second.lines().nth(2).unwrap_or("").to_string();
        assert_eq!(
            first_row, second_row,
            "scrolled-up view holds position on new content"
        );
        assert_eq!(app.scroll, 35, "scroll compensates for new lines");
    }

    #[test]
    fn picker_cursor_stays_visible_on_short_terminal() {
        let mut app = test_app();
        app.available_models = (0..50).map(|i| format!("openai/model-{i:02}")).collect();
        app.popup = Popup::ModelPicker;
        app.model_cursor = 40;

        let text = render_text(&mut app, 80, 20);
        assert!(
            text.contains("openai/model-40"),
            "selected model visible with height-aware viewport"
        );
        assert!(text.contains("▶"), "selection marker visible");
    }

    #[test]
    fn table_borders_align_on_screen() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich | Dart is less common than JS/Swift/Kotlin |\n\n```bash\necho hi\n```",
        ));
        let text = render_text(&mut app, 120, 40);
        let rows: Vec<String> = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with('┌') || t.starts_with('│') || t.starts_with('├') || t.starts_with('└')
            })
            .map(|l| l.trim_end().to_string())
            .collect();
        assert!(
            rows.len() >= 8,
            "table + fence rows present: {}",
            rows.len()
        );

        let nogutter = |l: &&String| l.chars().skip(2).collect::<String>();
        let table: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).starts_with(['┌', '│', '├', '└']) && !l.contains("echo"))
            .collect();
        let widths: Vec<usize> = table.iter().map(|l| nogutter(l).width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "on-screen table rows align: {widths:?}"
        );

        let fence: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).contains('┌') || nogutter(l).starts_with('└'))
            .collect();
        assert_eq!(fence.len(), 2);
        let fence_w: Vec<usize> = fence.iter().map(|l| nogutter(l).width()).collect();
        assert_eq!(fence_w[0], fence_w[1], "fence top/bottom align");
    }

    #[test]
    fn code_box_contains_long_lines_and_closes() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "Switch avg current: `Isw_avg` = `IL_avg` * `D`\n```\nIL_avg = Iin = Iout / (1-D) extra tail that must not escape\nΔIL = Vin * D / (L * fsw) = (Vout - Vin)*(1-D) / (L * fsw)\n```",
        ));
        let text = render_text(&mut app, 100, 40);
        let rows: Vec<String> = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                // Box rows sit inside the chat border: `│  ┌…`, `│  │…`, `│  └…`.
                t.starts_with("│  ┌") || t.starts_with("│  │") || t.starts_with("│  └")
            })
            .map(|l| l.trim_end().to_string())
            .collect();
        // Open + 2 body + close (chat chrome excluded by the filter).
        assert_eq!(rows.len(), 4, "closed box rows:\n{text}");
        let noborder = |l: &String| {
            let t = l.trim_start();
            t.strip_prefix('│')
                .unwrap_or(t)
                .strip_prefix("  ")
                .unwrap_or(t)
                .to_string()
        };
        let widths: Vec<usize> = rows.iter().map(|l| noborder(l).width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all box rows same width: {widths:?}"
        );
        assert!(widths[0] <= 98, "box fits the terminal: {widths:?}");
        assert!(rows[0].contains('┐'), "top-right corner");
        assert!(rows[3].contains('┘'), "bottom-right corner");
        for r in &rows[1..3] {
            assert!(noborder(r).trim_end().ends_with('│'), "right edge: {r:?}");
        }
        assert!(text.contains("Iout / (1-D)"), "long line kept");
    }

    #[test]
    fn headers_parse_inline_markdown() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "### 2. **Declarative UI**\nBody with **bold** and `code`.\n## Plain Header",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("**"), "no raw markers leak:\n{text}");
        assert!(text.contains("Declarative UI"));
        assert!(text.contains("Plain Header"));
    }

    #[test]
    fn unclosed_fence_gets_closing_rule() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "```bash\necho oops, never closed\n- a bullet after",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains('└'), "auto-closed fence renders bottom rule");
    }

    #[test]
    fn bullet_wraps_with_hanging_indent() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "- Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));

        let text = render_text(&mut app, 40, 20);
        let rows: Vec<&str> = text.lines().collect();

        let first = rows.iter().find(|l| l.contains("Alpha")).unwrap();
        assert!(first.contains('-'), "marker on first row: {first:?}");
        let strip_border = |l: &&str| {
            l.trim_start()
                .strip_prefix('│')
                .unwrap_or(l.trim_start())
                .to_string()
        };
        let cont = rows
            .iter()
            .skip_while(|l| !l.contains("Alpha"))
            .nth(1)
            .unwrap();
        let inner = strip_border(cont);
        assert!(
            inner.trim_start().starts_with(|c: char| c.is_alphabetic()),
            "continuation has no marker: {cont:?}"
        );
        assert!(
            inner.starts_with("    "),
            "continuation hangs under item text: {cont:?}"
        );
    }

    #[test]
    fn blank_lines_render_singly() {
        let mut app = test_app();
        app.messages.push(Msg::new("assistant", "aaa\n\nbbb"));
        let text = render_text(&mut app, 100, 14);
        let chat: Vec<&str> = text.lines().skip(1).take(7).collect();
        let joined = chat.join("\n");
        let aaa_at = chat.iter().position(|l| l.contains("aaa")).unwrap();
        let bbb_at = chat.iter().position(|l| l.contains("bbb")).unwrap();
        assert_eq!(
            bbb_at,
            aaa_at + 2,
            "exactly one blank row between paragraphs:\n{joined}"
        );
    }

    #[test]
    fn full_render_stays_fast_on_large_chats() {
        let mut app = test_app();
        let body = "## Section\n\n### 2. **Declarative UI**\nBody with **bold**, *italic* and `code` spans across several words.\n\n- Alpha bullet with enough words to wrap across rows on narrow screens\n- Beta bullet\n\n| A | B |\n|---|---|\n| one | two |\n| three | four |\n\n```rust\nfn main() {\n    // comment\n    let s = \"hi\";\n    println!(\"{s}\");\n}\n```\n";
        for i in 0..60 {
            app.messages
                .push(Msg::new("user", format!("prompt number {i}")));
            app.messages.push(Msg::new("assistant", body));
        }
        let t0 = std::time::Instant::now();
        let text = render_text(&mut app, 120, 40);
        let ms = t0.elapsed().as_millis();
        println!("full render of {} msgs took {ms}ms", app.messages.len());
        assert!(text.contains("Declarative UI"));
        assert!(
            ms < 3000,
            "render stall: {ms}ms for a large chat (theme cache? memo?)"
        );
    }

    #[test]
    fn visual_dump_table_and_code() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "## Core Concepts\n\n### 2. **Declarative UI**\nYou describe **what** the UI should look like for a given state, not **how** to transition between states. Flutter handles the rendering pipeline.\n\n### 4. **Rendering Engine (Skia)**\nFlutter draws everything using **Skia**. This means:\n- **Consistent** look and feel across platforms with pixel-perfect control over every pixel on the screen.\n- No bridge overhead unlike React Native and other cross-platform frameworks.\n\n| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich, customizable widgets | Dart is less common than JS/Swift/Kotlin |\n\n## Quick Start\n\n```bash\ngit clone https://github.com/flutter/flutter.git\nexport PATH=\"$PATH:pwd/flutter/bin\"\nflutter create my_app\n```",
        ));
        let text = render_text(&mut app, 100, 48);
        println!("=== VISUAL DUMP START ===\n{text}\n=== VISUAL DUMP END ===");
        assert!(text.contains('┌') && text.contains('┐'));
        assert!(text.contains('└') && text.contains('┘'));
        assert!(!text.contains("|---|"), "no raw delimiter row leaks");
        assert!(!text.contains("**"), "no raw bold markers leak");
    }

    #[test]
    fn thinking_header_renders_distinct_marker() {
        let mut app = test_app();
        app.show_thinking = true;
        let mut m = Msg::new("assistant", "hello");
        m.reasoning = Some("private chain of thought".into());
        app.messages.push(m);
        let text = render_text(&mut app, 80, 30);
        assert!(
            text.contains("✦"),
            "collapsed thinking header carries the distinct marker"
        );
        assert!(text.contains("Thinking"), "thinking title renders");
        assert!(
            !text.contains("(Reasoning)"),
            "redundant parenthetical label is gone from chat lines"
        );
    }

    #[test]
    fn tool_cards_respect_verbosity() {
        fn card_app(level: &str) -> App {
            let mut app = test_app();
            app.tool_display.insert("bash".into(), level.into());
            let mut m = Msg::new("assistant", "working");
            m.items.push(Content::ToolCall {
                id: "call_hidden_test_1".into(),
                name: "bash".into(),
                args: "{\"command\": \"ls -la myproject\"}".into(),
                status: ToolStatus::Done,
            });
            m.items.push(Content::ToolResult {
                id: "call_hidden_test_1".into(),
                content: "{\"ok\":true,\"code\":0,\"stdout\":\"total 8\"}".into(),
                ok: true,
            });
            app.messages.push(m);
            app
        }

        let hidden = render_text(&mut card_app("hidden"), 100, 30);
        assert!(
            !hidden.contains("bash"),
            "hidden tool leaves no card: {hidden}"
        );

        let quiet = render_text(&mut card_app("quiet"), 100, 30);
        assert!(quiet.contains("bash"), "quiet keeps the name");
        assert!(
            !quiet.contains("ls -la"),
            "quiet drops the preview: {quiet}"
        );
        assert!(
            !quiet.contains("total 8"),
            "quiet drops the output: {quiet}"
        );

        let compact = render_text(&mut card_app("compact"), 100, 30);
        assert!(compact.contains("ls -la"), "compact shows the summary");
    }

    #[test]
    fn tool_result_line_shows_tool_name_not_call_id() {
        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            name: "write".into(),
            args: "{}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            content: "{\"ok\":true,\"path\":\"./index.html\",\"old_lines\":0,\"new_lines\":136,\"bytes\":5082}".into(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("✔ write"), "result line uses tool name");
        assert!(
            !text.contains("call_01a07"),
            "raw provider id hidden from result line"
        );
        assert!(
            text.contains("136 lines"),
            "write summary renders on the card"
        );
    }

    #[test]
    fn tool_cards_show_name_summary_id() {
        let mut app = test_app();
        let mut m = Msg::new("assistant", "working");
        m.items.push(Content::ToolCall {
            id: "call_abc123def456".into(),
            name: "bash".into(),
            args: "{\"command\": \"npm test\"}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_abc123def456".into(),
            content: "{\"ok\":true,\"code\":0,\"stdout\":\"ok\"}".into(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(
            text.contains("● bash · npm test · #def456"),
            "call title is Name + summary + ID: {text}"
        );
        assert!(
            text.contains("✔ bash · #def456"),
            "result title carries Name + ID: {text}"
        );
        assert!(!text.contains("call_abc"), "raw provider id hidden: {text}");
    }

    #[test]
    fn edit_card_shows_inline_mini_diff() {
        let diff = "--- a/AGENTS.md\n+++ b/AGENTS.md\n@@ -43 +43 @@\n-old line here\n+new line here\n context\n+second add\n+third add\n+fourth add\n+fifth add\n";
        let (shown, rest) = diff_card_lines(diff, 4);
        assert_eq!(shown.len(), 4, "caps shown lines");
        assert_eq!(rest, 2, "counts the remainder");
        assert_eq!(shown[0], (false, "old line here".to_string()));
        assert_eq!(shown[1], (true, "new line here".to_string()));

        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_edit_diff_1".into(),
            name: "edit".into(),
            args: "{\"path\": \"AGENTS.md\"}".into(),
            status: ToolStatus::Done,
        });
        let res = serde_json::json!({
            "ok": true, "path": "AGENTS.md",
            "old_lines": 69, "new_lines": 70, "bytes": 3000,
            "diff_preview": diff,
        })
        .to_string();
        m.items.push(Content::ToolResult {
            id: "call_edit_diff_1".into(),
            content: res,
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("+ new line here"), "added line on card");
        assert!(text.contains("- old line here"), "removed line on card");
        assert!(text.contains("+2 more"), "remainder hint on card");
    }

    #[test]
    fn long_chat_messages_collapse_with_hint() {
        let short = "hello";
        let (t, h) = collapse_long_content(short, false);
        assert_eq!(t, short);
        assert!(h.is_none());
        let long: String = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (t, h) = collapse_long_content(&long, false);
        assert_eq!(h, Some(20));
        assert_eq!(t.lines().count(), CHAT_COLLAPSE_HEAD);
        assert!(t.contains("line 0") && !t.contains("line 19"));
        let (t, h) = collapse_long_content(&long, true);
        assert!(h.is_none() && t == long, "expanded shows all");

        let code = "```rust\n".to_string()
            + &(0..20)
                .map(|i| format!("l{i}"))
                .collect::<Vec<_>>()
                .join("\n");
        let (t, _) = collapse_long_content(&code, false);
        assert_eq!(
            t.lines().filter(|l| l.trim().starts_with("```")).count() % 2,
            0,
            "fence balanced"
        );

        let mut app = test_app();
        let mut m = Msg::new("user", long.clone());
        m.timestamp = "04:10".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 40);
        assert!(text.contains("line 0"), "head shown");
        assert!(!text.contains("line 19"), "tail hidden");
        assert!(text.contains("Ctrl+X to expand"), "hint shown");
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "expanded reveals all");
        assert!(!text.contains("Ctrl+X to expand"), "hint gone when open");

        let mut app = test_app();
        let mut m = Msg::new("assistant", long.clone());
        m.timestamp = "04:11".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "model answer fully visible");
        assert!(!text.contains("Ctrl+X to expand"), "no hint on responses");
    }

    #[test]
    fn wrapped_continuation_maps_via_drawn_frame() {
        let mut app = test_app();
        app.show_thinking = false;
        app.messages.push(Msg::new(
            "assistant",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));
        let _ = render_text(&mut app, 40, 16);
        assert!(app.vis_rows.len() >= 2, "wraps: {:?}", app.vis_rows);

        let body: Vec<(usize, usize)> = app.vis_rows.clone();
        assert!(body.windows(2).all(|w| w[0].0 == w[1].0 || w[1].1 == 0));
        let inner = app.chat_inner().unwrap();
        let pos = app.screen_to_content(inner.x, inner.y + 1).expect("maps");
        assert_eq!(pos.line, body[1].0);
        assert_eq!(pos.col, body[1].1);
    }

    #[test]
    fn selection_paints_full_range_including_gutter() {
        let sel_bg = Theme::text_selection().bg;
        let gutter = Span::styled("  ", Style::default());
        let rest = highlighted_code_spans("  Widget build(BuildContext context) {");
        let mut spans = vec![gutter];
        spans.extend(rest);
        spans.insert(1, Span::styled("│ ", Theme::code_block()));
        let out = highlight_range(&spans, 0, usize::MAX, Theme::text_selection());
        assert!(!out.is_empty());
        for s in &out {
            assert_eq!(s.style.bg, sel_bg, "all cols selected: {:?}", s.content);
        }

        let out = highlight_range(&spans, 4, 8, Theme::text_selection());
        let painted: String = out
            .iter()
            .filter(|s| s.style.bg == sel_bg)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(painted.chars().count(), 4);
    }

    #[test]
    fn hint_bar_chips_keys_and_labels() {
        let line = App::hint_bar(&[("↑/↓", "Navigate"), ("enter", "Select"), ("esc", "Cancel")]);
        let texts: Vec<String> = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(texts.iter().any(|t| t.contains("↑/↓")), "key chip");
        assert!(texts.iter().any(|t| t.contains("Navigate")), "label");
        let chip = line
            .spans
            .iter()
            .find(|s| s.content.contains("enter"))
            .expect("enter chip");
        assert_eq!(chip.style.fg, Some(Color::Black));
        assert_eq!(chip.style.bg, Some(Color::Cyan));
        assert!(chip.style.add_modifier.contains(Modifier::BOLD));
        let label = line
            .spans
            .iter()
            .find(|s| s.content.contains("Cancel"))
            .expect("cancel label");
        assert_eq!(label.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn user_prompt_renders_bold_white() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = test_app();
        app.messages.push(Msg::new("user", "do the thing"));
        app.messages.push(Msg::new("assistant", "do the thing"));
        let backend = TestBackend::new(60, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut styled_rows = 0;
        let mut plain_rows = 0;
        for y in 0..buf.area.height {
            let mut row_text = String::new();
            let mut row_styled = String::new();
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                row_text.push_str(cell.symbol());
                if cell.fg == Color::White && cell.modifier.contains(Modifier::BOLD) {
                    row_styled.push_str(cell.symbol());
                }
            }
            if row_text.contains("do the thing") {
                if row_styled.replace(' ', "").contains("dothething") {
                    styled_rows += 1;
                } else {
                    plain_rows += 1;
                }
            }
        }
        assert_eq!(styled_rows, 1, "exactly the user prompt is bold white");
        assert_eq!(plain_rows, 1, "assistant copy stays plain");
    }

    #[test]
    fn input_selection_paints_highlight() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = test_app();
        app.input.text = "hello".into();
        app.input.cursor = 5;
        app.input.sel_anchor = Some(2);
        let backend = TestBackend::new(40, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let sel_bg = Theme::selection().bg;
        let mut painted = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                if Some(cell.bg) == sel_bg {
                    painted.push_str(cell.symbol());
                }
            }
        }
        assert!(
            painted.contains("llo"),
            "selected input text highlighted: {painted:?}"
        );
        assert!(!painted.contains('h'), "unselected head not painted");
    }
}
