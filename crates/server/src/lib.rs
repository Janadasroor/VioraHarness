// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as TokioStreamExt;

pub mod shim;

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<HashMap<String, SessionMeta>>>,
    bus: broadcast::Sender<BusEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMeta {
    id: String,
    model: String,
    created_at: i64,
    harness: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreateSessionReq {
    #[serde(default = "default_harness")]
    harness: String,
    #[serde(default = "default_model")]
    model: String,
    title: Option<String>,
}
fn default_harness() -> String {
    "vioraharness".into()
}
fn default_model() -> String {
    String::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptReq {
    prompt: String,
    #[serde(default)]
    model: Option<String>,

    /// Agent mode for this turn (`eda` default). Unknown values are 400.
    /// Omitted → the session's stored mode → project default → `eda`.
    #[serde(default)]
    mode: Option<String>,

    #[serde(default)]
    wait: bool,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    q: Option<String>,
    project: Option<String>,
    include_archived: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RenameReq {
    title: String,
}

#[derive(Debug, Deserialize)]
struct ForkReq {
    at_seq: Option<i64>,
    new_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BusEvent {
    session_id: String,
    event: String,
    data: serde_json::Value,
}

#[derive(Debug)]
struct Authed;

fn server_token() -> Option<String> {
    std::env::var("VIORAHARNESS_API_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[axum::async_trait]
impl<S> axum::extract::FromRequestParts<S> for Authed
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match server_token() {
            None => Ok(Authed),
            Some(expected) => {
                let got = parts
                    .headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                let token = got
                    .strip_prefix("Bearer ")
                    .or_else(|| got.strip_prefix("bearer "))
                    .unwrap_or("");
                if constant_time_eq(token, &expected) {
                    Ok(Authed)
                } else {
                    Err((
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({"error": "missing or invalid bearer token"})),
                    ))
                }
            }
        }
    }
}

pub async fn serve(port: u16) -> anyhow::Result<()> {
    let (tx, _rx) = broadcast::channel::<BusEvent>(2048);
    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        bus: tx,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/session", post(create_session))
        .route("/sessions", get(list_sessions))
        .route("/session/:id", get(get_session))
        .route("/session/:id/messages", get(get_messages))
        .route("/session/:id/export", get(export_session))
        .route("/session/:id/prompt", post(prompt_async))
        .route("/session/:id/fork", post(fork_session))
        .route("/session/:id/archive", post(archive_session))
        .route("/session/:id/rename", post(rename_session))
        .route("/session/:id", delete(delete_session))
        .route("/session/:id/events", get(session_events))
        .route("/events", get(events))
        .with_state(state);

    let addr = if server_token().is_some() {
        tracing::info!(
            "VioraHarness server on 0.0.0.0:{port} WITH bearer auth (VIORAHARNESS_API_TOKEN)"
        );
        format!("0.0.0.0:{port}")
    } else {
        tracing::info!(
            "VioraHarness server on 127.0.0.1:{port} (loopback only — set VIORAHARNESS_API_TOKEN to expose it, then Authorization: Bearer is required)"
        );
        format!("127.0.0.1:{port}")
    };
    tracing::info!("chat management: /sessions, /session/:id/fork|archive|rename|export");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "service": "vioraharness",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn list_models(_auth: Authed) -> impl IntoResponse {
    let mut models: Vec<String> = Vec::new();
    for cand in vioraharness_core::loop_mod::config_candidates() {
        if let Ok(s) = std::fs::read_to_string(&cand) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(prov) = v.get("provider") {
                    for key in ["openrouter", "gemini", "opencode", "opencode-go"] {
                        if let Some(arr) = prov
                            .get(key)
                            .and_then(|p| p.get("models"))
                            .and_then(|m| m.as_array())
                        {
                            for x in arr.iter().filter_map(|x| x.as_str()) {
                                let id = if key == "gemini" && !x.contains('/') {
                                    format!("google/{x}")
                                } else {
                                    x.to_string()
                                };
                                if !models.contains(&id) {
                                    models.push(id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for id in vioraharness_core::provider::catalog::gateway_models_cached().await {
        let full = format!("opencode/{id}");
        if !models.contains(&full) {
            models.push(full);
        }
        if !models.contains(&id) {
            models.push(id);
        }
    }
    for id in vioraharness_core::provider::catalog::openrouter_context_cached()
        .await
        .keys()
    {
        if !models.contains(id) {
            models.push(id.clone());
        }
    }
    models.sort();
    Json(
        serde_json::json!({"data": models.iter().map(|m| serde_json::json!({"id": m, "object": "model"})).collect::<Vec<_>>() }),
    )
}

async fn create_session(
    _auth: Authed,
    State(st): State<AppState>,
    Json(req): Json<CreateSessionReq>,
) -> impl IntoResponse {
    let id = format!(
        "sess_{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let meta = SessionMeta {
        id: id.clone(),
        model: req.model.clone(),
        created_at: chrono_now(),
        harness: req.harness.clone(),
    };
    {
        let mut g = st.sessions.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(id.clone(), meta.clone());
    }
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
        let _ = store.create_session(&id, &req.model, req.title.as_deref());
    }
    let ev = BusEvent {
        session_id: id.clone(),
        event: "session.create".into(),
        data: serde_json::to_value(&meta).unwrap(),
    };
    let _ = st.bus.send(ev);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "model": meta.model, "harness": meta.harness})),
    )
}

async fn get_session(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    {
        let g = st.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(meta) = g.get(&id) {
            return Json(serde_json::json!(meta)).into_response();
        }
    }
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
        if let Ok(Some(sess)) = store.get_session(&id) {
            return Json(serde_json::json!({
                "id": sess.id,
                "model": sess.model_last.clone().unwrap_or(sess.model),
                "created_at": sess.created_at,
                "updated_at": sess.updated_at,
                "title": sess.title,
                "cwd": sess.cwd,
                "parent_id": sess.parent_id,
                "archived_at": sess.archived_at,
                "harness": "vioraharness"
            }))
            .into_response();
        }
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "session not found"})),
    )
        .into_response()
}

async fn list_sessions(
    _auth: Authed,
    State(_st): State<AppState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("db: {e}")})),
            )
                .into_response()
        }
    };
    let limit = q.limit.unwrap_or(50).min(100);
    let offset = q.offset.unwrap_or(0);
    let include_archived = q.include_archived.unwrap_or(false);
    match store.list_sessions_filtered(
        q.project.as_deref(),
        q.q.as_deref(),
        include_archived,
        limit,
        offset,
    ) {
        Ok(sessions) => {
            Json(serde_json::json!({"sessions": sessions, "total": sessions.len()})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn get_messages(_auth: Authed, Path(id): Path<String>) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.get_messages_detailed(&id) {
        Ok(msgs) => Json(serde_json::json!({"session_id": id, "messages": msgs})).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn export_session(_auth: Authed, Path(id): Path<String>) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.export_jsonl(&id) {
        Ok(jsonl) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
            jsonl,
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn fork_session(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkReq>,
) -> impl IntoResponse {
    let new_id = req.new_id.unwrap_or_else(|| {
        format!(
            "sess_{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    });
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.fork_session(&id, &new_id, req.at_seq) {
        Ok(_) => {
            if let Ok(Some(sess)) = store.get_session(&new_id) {
                let mut g = st.sessions.lock().unwrap_or_else(|e| e.into_inner());
                g.insert(
                    new_id.clone(),
                    SessionMeta {
                        id: new_id.clone(),
                        model: sess.model,
                        created_at: sess.created_at,
                        harness: "vioraharness".into(),
                    },
                );
            }
            let ev = BusEvent {
                session_id: new_id.clone(),
                event: "session.fork".into(),
                data: serde_json::json!({"parent_id": id, "new_id": new_id}),
            };
            let _ = st.bus.send(ev);
            Json(serde_json::json!({"ok": true, "id": new_id, "parent_id": id})).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn archive_session(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.archive_session(&id) {
        Ok(_) => {
            st.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            Json(serde_json::json!({"ok": true, "id": id, "archived": true})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn rename_session(
    _auth: Authed,
    Path(id): Path<String>,
    Json(req): Json<RenameReq>,
) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.rename_session(&id, &req.title) {
        Ok(_) => {
            Json(serde_json::json!({"ok": true, "id": id, "title": req.title})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn delete_session(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    };
    match store.delete_session(&id) {
        Ok(_) => {
            st.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            Json(serde_json::json!({"ok": true, "deleted": id})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

async fn run_turn_streaming(
    prompt: String,
    model: String,
    session_id: String,
    bus: broadcast::Sender<BusEvent>,
    mode: String,
) -> Result<String, String> {
    use vioraharness_core::provider::ProviderEvent as E;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<E>(256);
    let bus_ev = bus.clone();
    let sid_ev = session_id.clone();

    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let (event, data) = match &ev {
                E::TextDelta(t) => ("turn.text", serde_json::json!({"delta": t})),
                E::ReasoningDelta(r) => ("turn.thinking", serde_json::json!({"delta": r})),
                E::ToolCallDelta { id, name, args, .. } => (
                    "turn.tool_call",
                    serde_json::json!({"id": id, "tool": name, "args": args}),
                ),
                E::ToolResultDelta { id, content, ok } => (
                    "turn.tool_result",
                    serde_json::json!({"id": id, "ok": ok, "content": content}),
                ),
                E::Notice(msg) => ("turn.notice", serde_json::json!({"message": msg})),
                E::Done => ("turn.done", serde_json::json!({})),
            };
            let _ = bus_ev.send(BusEvent {
                session_id: sid_ev.clone(),
                event: event.into(),
                data,
            });
            if matches!(ev, E::Done) {
                break;
            }
        }
    });
    let loop_ = vioraharness_core::loop_mod::AgentLoop::with_mode(&mode);
    let out = loop_
        .run_streaming(&prompt, &model, Some(session_id), tx)
        .await
        .map_err(|e| format!("{e:#}"));

    let _ = pump.await;
    out
}

async fn prompt_async(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PromptReq>,
) -> impl IntoResponse {
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| {
            st.sessions
                .lock()
                .unwrap()
                .get(&id)
                .map(|m| m.model.clone())
                .unwrap_or_default()
        });
    let model = if model.trim().is_empty() {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        vioraharness_core::session::SessionStore::new(&db)
            .ok()
            .and_then(|s| s.get_session(&id).ok().flatten())
            .and_then(|sess| sess.model_last.or(Some(sess.model)))
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_default()
    } else {
        model
    };
    if model.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "no model set for this session; pass model: explicitly (see GET /v1/models)"})),
        )
            .into_response();
    }
    let prompt = req.prompt.clone();
    let session_id = id.clone();
    let bus = st.bus.clone();
    // Mode precedence: request > stored session mode > default chain.
    // Unknown request values fail closed (permissions fail-closed remotely).
    let mode = match req.mode.clone().filter(|m| !m.trim().is_empty()) {
        Some(m) => {
            if !vioraharness_core::mode::is_known_mode(&m) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("unknown mode: {m} (known: eda, web, android)")})),
                )
                    .into_response();
            }
            vioraharness_core::mode::normalize_mode_name(&m)
        }
        None => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            vioraharness_core::session::SessionStore::new(&db)
                .ok()
                .and_then(|s| s.get_session(&id).ok().flatten())
                .and_then(|sess| sess.mode)
                .filter(|mm| vioraharness_core::mode::is_known_mode(mm))
                .unwrap_or_else(|| vioraharness_core::mode::resolve_mode(None))
        }
    };

    if req.wait {
        let _ = bus.send(BusEvent {
            session_id: session_id.clone(),
            event: "prompt.start".into(),
            data: serde_json::json!({"prompt": prompt, "mode": mode}),
        });
        return match run_turn_streaming(prompt, model, session_id, bus.clone(), mode).await {
            Ok(text) => {
                let _ = bus.send(BusEvent {
                    session_id: id.clone(),
                    event: "prompt.done".into(),
                    data: serde_json::json!({"text": text}),
                });
                Json(serde_json::json!({"ok": true, "session_id": id, "text": text}))
                    .into_response()
            }
            Err(e) => {
                let _ = bus.send(BusEvent {
                    session_id: id.clone(),
                    event: "prompt.error".into(),
                    data: serde_json::json!({"error": e}),
                });
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"ok": false, "session_id": id, "error": e})),
                )
                    .into_response()
            }
        };
    }

    tokio::spawn(async move {
        let _ = bus.send(BusEvent {
            session_id: session_id.clone(),
            event: "prompt.start".into(),
            data: serde_json::json!({"prompt": prompt, "mode": mode}),
        });
        match run_turn_streaming(prompt, model, session_id.clone(), bus.clone(), mode).await {
            Ok(text) => {
                let _ = bus.send(BusEvent {
                    session_id: session_id.clone(),
                    event: "prompt.done".into(),
                    data: serde_json::json!({"text": text}),
                });
            }
            Err(e) => {
                let _ = bus.send(BusEvent {
                    session_id: session_id.clone(),
                    event: "prompt.error".into(),
                    data: serde_json::json!({"error": e}),
                });
            }
        }
    });

    Json(serde_json::json!({"ok": true, "session_id": id, "status": "queued"})).into_response()
}

async fn events(
    _auth: Authed,
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>> {
    let rx = st.bus.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(ev) => {
            let data = serde_json::to_string(&ev).unwrap_or_default();
            Some(Ok(axum::response::sse::Event::default()
                .event(ev.event)
                .data(data)))
        }
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn session_events(
    _auth: Authed,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>> {
    let rx = st.bus.subscribe();
    let sid = id.clone();
    let stream = BroadcastStream::new(rx).filter_map(move |res| {
        let sid = sid.clone();
        match res {
            Ok(ev) if ev.session_id == sid => {
                let data = serde_json::to_string(&ev).unwrap_or_default();
                Some(Ok(axum::response::sse::Event::default()
                    .event(ev.event)
                    .data(data)))
            }
            Ok(_) => None,
            Err(_) => None,
        }
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
// Env-mutating tests serialize on process-global locks held across awaits
// by design (env sets + awaits must not interleave); the deadlock risk the
// lint guards against does not apply to these test-only guards.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let (tx, _rx) = broadcast::channel::<BusEvent>(16);
        AppState {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            bus: tx,
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_db() -> (String, String) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vh_srv_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("t.db").to_string_lossy().to_string();
        (dir.to_string_lossy().to_string(), db)
    }

    #[test]
    fn prompt_req_compat() {
        let r: PromptReq =
            serde_json::from_str(r#"{"prompt":"hi","stream":true,"model":"m"}"#).unwrap();
        assert!(!r.wait);
        assert_eq!(r.model.as_deref(), Some("m"));
        let r: PromptReq = serde_json::from_str(r#"{"prompt":"hi","wait":true}"#).unwrap();
        assert!(r.wait);
        assert!(r.model.is_none());
        assert!(r.mode.is_none());
        let r: PromptReq = serde_json::from_str(r#"{"prompt":"hi","mode":"web"}"#).unwrap();
        assert_eq!(r.mode.as_deref(), Some("web"));
    }

    #[tokio::test]
    async fn prompt_unknown_mode_is_400() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, db) = temp_db();
        std::env::set_var("VIORAHARNESS_DB", &db);
        let st = test_state();
        let resp = prompt_async(
            Authed,
            State(st),
            Path("nope-missing".into()),
            Json(PromptReq {
                prompt: "hi".into(),
                model: Some("m".into()),
                mode: Some("nope".into()),
                wait: true,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        std::env::remove_var("VIORAHARNESS_DB");
    }

    #[tokio::test]
    async fn health_reports_version() {
        let resp = health().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_get_archive_cycle() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, db) = temp_db();
        std::env::set_var("VIORAHARNESS_DB", &db);
        let st = test_state();
        let resp = create_session(
            Authed,
            State(st.clone()),
            Json(CreateSessionReq {
                harness: "vioraharness".into(),
                model: "unit/test-model".into(),
                title: Some("t".into()),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            st.sessions.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        let id = st
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .next()
            .unwrap()
            .clone();

        let resp = archive_session(Authed, State(st.clone()), Path(id.clone()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(st
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());

        let resp = get_session(Authed, State(st.clone()), Path(id))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        std::env::remove_var("VIORAHARNESS_DB");
    }

    #[tokio::test]
    async fn prompt_without_model_is_400() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, db) = temp_db();
        std::env::set_var("VIORAHARNESS_DB", &db);
        let st = test_state();
        let resp = prompt_async(
            Authed,
            State(st),
            Path("nope-missing".into()),
            Json(PromptReq {
                prompt: "hi".into(),
                model: None,
                mode: None,
                wait: false,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        std::env::remove_var("VIORAHARNESS_DB");
    }

    #[tokio::test]
    async fn bearer_gate() {
        use axum::extract::FromRequestParts;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var("VIORAHARNESS_API_TOKEN");
        let (mut parts, _) = axum::http::Request::builder()
            .body(())
            .unwrap()
            .into_parts();
        assert!(Authed::from_request_parts(&mut parts, &()).await.is_ok());

        std::env::set_var("VIORAHARNESS_API_TOKEN", "s3cret");
        let (mut parts, _) = axum::http::Request::builder()
            .body(())
            .unwrap()
            .into_parts();
        let r = Authed::from_request_parts(&mut parts, &()).await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().0, StatusCode::UNAUTHORIZED);
        let (mut parts, _) = axum::http::Request::builder()
            .header("Authorization", "Bearer wrong")
            .body(())
            .unwrap()
            .into_parts();
        assert!(Authed::from_request_parts(&mut parts, &()).await.is_err());
        let (mut parts, _) = axum::http::Request::builder()
            .header("Authorization", "Bearer s3cret")
            .body(())
            .unwrap()
            .into_parts();
        assert!(Authed::from_request_parts(&mut parts, &()).await.is_ok());

        let (mut parts, _) = axum::http::Request::builder()
            .header("Authorization", "Bearer s3cre")
            .body(())
            .unwrap()
            .into_parts();
        assert!(Authed::from_request_parts(&mut parts, &()).await.is_err());
        std::env::remove_var("VIORAHARNESS_API_TOKEN");
    }

    #[tokio::test]
    async fn list_models_shape() {
        let resp = list_models(Authed).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
