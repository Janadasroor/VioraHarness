// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::{App, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

pub(crate) fn test_app() -> App {
    let mut app = App::new("unittest/test-model".to_string());
    app.busy = false;
    // Memory-only: unit tests must never read the user's real prompt
    // history into memory nor write test prompts back to disk.
    // App::new loads disk history; drop it here.
    app.input.persist = false;
    app.input.history_file = None;
    app.input.history.clear();
    app.input.hist_idx = None;
    app.input.draft.clear();
    app
}

/// Writable workdir for background-task tests. `std::env::temp_dir()` is
/// portable (`/tmp` does not exist on Windows runners).
pub(crate) fn test_workdir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

pub(crate) fn render_text(app: &mut App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn push_lines(app: &mut App, n: usize, start_at: usize) {
    for i in 0..n {
        app.messages
            .push(Msg::new("user", format!("line {:03}", start_at + i)));
    }
}

pub(crate) fn mouse_app() -> App {
    let mut app = test_app();
    app.chat_area = Rect::new(0, 1, 100, 20);
    app.view_start = 10;
    app.view_total = 100;

    app.vis_rows = (0..100usize).map(|i| (i, 0)).collect();
    app
}

pub(crate) fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

pub(crate) fn pending_question() -> (
    vioraharness_core::permissions::PendingQuestion,
    tokio::sync::oneshot::Receiver<vioraharness_core::permissions::QuestionResult>,
) {
    use vioraharness_core::permissions::{PendingQuestion, QuestionItem, QuestionOption};
    let (tx, rx) = tokio::sync::oneshot::channel();
    let q = PendingQuestion {
        id: "q1".into(),
        questions: vec![
            QuestionItem {
                question: "Pick one".into(),
                header: "H1".into(),
                multi_select: false,
                options: vec![
                    QuestionOption {
                        label: "A".into(),
                        description: "first".into(),
                    },
                    QuestionOption {
                        label: "B".into(),
                        description: String::new(),
                    },
                ],
            },
            QuestionItem {
                question: "Pick many".into(),
                header: String::new(),
                multi_select: true,
                options: vec![
                    QuestionOption {
                        label: "X".into(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "Y".into(),
                        description: String::new(),
                    },
                ],
            },
        ],
        tx,
    };
    (q, rx)
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

pub(crate) static DB_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

pub(crate) static ERR_REG_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Temp DB + env swap with the process-global env lock held. The returned
/// guard MUST be kept alive for the whole test: dropping it releases the
/// lock while `VIORAHARNESS_DB` still points at the temp file, which races
/// other tests (lock-then-set ordering is enforced structurally here).
pub(crate) fn with_temp_db(
    tag: &str,
) -> (
    std::path::PathBuf,
    Option<String>,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Unique per call (counter defeats pid reuse) and sidecar-free: a stale
    // -wal/-shm from a killed run would otherwise resurrect old state.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let db = std::env::temp_dir().join(format!("vh_tuidb_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_file(&db);
    // SQLite sidecars are `<db>-wal` / `<db>-shm` (suffix, not extension).
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = db.clone().into_os_string();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(sidecar);
    }
    let prev = std::env::var("VIORAHARNESS_DB").ok();
    std::env::set_var("VIORAHARNESS_DB", &db);
    (db, prev, guard)
}

pub(crate) fn restore_db_env(prev: Option<String>, db: &std::path::Path) {
    match prev {
        Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
        None => std::env::remove_var("VIORAHARNESS_DB"),
    }
    let _ = std::fs::remove_file(db);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = db.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(sidecar);
    }
}

pub(crate) async fn wait_task_done(id: &str) {
    use vioraharness_core::tools::tasks;
    let start = std::time::Instant::now();
    loop {
        if let Some(cur) = tasks::get_task(id) {
            if cur.status != tasks::BgStatus::Running {
                break;
            }
        }
        assert!(start.elapsed().as_secs() < 10, "task {id} finished");
        tokio::task::yield_now().await;
    }
}
