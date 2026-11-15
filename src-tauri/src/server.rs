//! Server locale per OBS: pagine "fonte browser" + WebSocket di signaling.

use crate::hub::SharedHub;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

const VIEWER_JS: &str = include_str!("../gen/viewer.js");
const TEST_HTML: &str = include_str!("../assets/test-publisher.html");

/// Bind sincrono: la porta è già in ascolto prima che si apra la finestra studio.
pub fn bind(port: u16) -> std::io::Result<std::net::TcpListener> {
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

pub async fn serve(hub: SharedHub, std_listener: std::net::TcpListener) {
    let app = Router::new()
        .route("/", get(index))
        .route("/slot/{n}", get(slot_page))
        .route("/viewer.js", get(viewer_js))
        .route("/ws", get(ws))
        .route("/test", get(|| async { Html(TEST_HTML) }))
        .route("/api/state", get(api_state))
        .route("/api/assign", get(api_assign))
        .route("/api/log", axum::routing::post(|body: String| async move { eprintln!("[studio] {body}"); }))
        .with_state(hub);
    let listener = match tokio::net::TcpListener::from_std(std_listener) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[obsyard] listener: {e}");
            return;
        }
    };
    let _ = axum::serve(listener, app).await;
}

async fn index(State(hub): State<SharedHub>) -> Html<String> {
    let outputs = hub.lock().unwrap().outputs.clone();
    let links: String = outputs
        .iter()
        .map(|o| format!("<li><a href=\"/slot/{0}\">{1}</a> <code>/slot/{0}</code></li>", o.id, html_escape(&o.name)))
        .collect();
    Html(format!(
        "<!doctype html><meta charset=utf-8><title>OBSYard</title>\
         <body style=\"font:14px system-ui;background:#111;color:#ddd\">\
         <h3>OBSYard – fonti per OBS</h3><ul>{links}</ul>\
         <p>Parametri: <code>?audio=0</code> (solo video), <code>?fit=cover</code>, <code>?bg=000000</code></p>"
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn slot_page(Path(n): Path<u32>, State(hub): State<SharedHub>) -> Response {
    if !hub.lock().unwrap().has_output(n) {
        return (StatusCode::NOT_FOUND, "slot inesistente").into_response();
    }
    Html(format!(
        "<!doctype html><html><head><meta charset=utf-8><title>OBSYard slot {n}</title>\
         <style>html,body{{margin:0;height:100%;overflow:hidden;background:transparent}}\
         video{{position:fixed;inset:0;width:100%;height:100%;object-fit:contain;opacity:0}}\
         video.on{{opacity:1}}</style></head>\
         <body><video id=v autoplay playsinline disablepictureinpicture></video>\
         <script>window.__SLOT={n}</script><script src=\"/viewer.js\"></script></body></html>"
    ))
    .into_response()
}

async fn viewer_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        VIEWER_JS,
    )
}

async fn api_state(State(hub): State<SharedHub>) -> Response {
    axum::Json(hub.lock().unwrap().snapshot()).into_response()
}

#[derive(Deserialize)]
struct AssignQuery {
    slot: u32,
    /// id traccia o parte del nome dell'ospite; "none" per svuotare
    video: Option<String>,
    /// id, nome, "auto" o "none" (default: auto)
    audio: Option<String>,
}

/// Per Stream Deck / script: GET /api/assign?slot=1&video=Mario
async fn api_assign(Query(q): Query<AssignQuery>, State(hub): State<SharedHub>) -> Response {
    let mut h = hub.lock().unwrap();
    if !h.slots.contains_key(&q.slot) {
        return (StatusCode::NOT_FOUND, "slot inesistente").into_response();
    }
    let video = match q.video.as_deref() {
        None | Some("none") | Some("") => None,
        Some(s) => match h.find_track("video", s) {
            Some(id) => Some(id),
            None => return (StatusCode::NOT_FOUND, "traccia video non trovata").into_response(),
        },
    };
    let audio = match q.audio.as_deref() {
        None | Some("auto") => Some("auto".to_string()),
        Some("none") | Some("") => None,
        Some(s) => match h.find_track("audio", s) {
            Some(id) => Some(id),
            None => return (StatusCode::NOT_FOUND, "traccia audio non trovata").into_response(),
        },
    };
    h.assign(q.slot, crate::hub::SlotAssign { video, audio });
    axum::Json(h.snapshot()).into_response()
}

#[derive(Deserialize)]
struct WsQuery {
    slot: u32,
}

async fn ws(ws: WebSocketUpgrade, Query(q): Query<WsQuery>, State(hub): State<SharedHub>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, q.slot, hub))
}

async fn handle_ws(socket: WebSocket, slot: u32, hub: SharedHub) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let id = hub.lock().unwrap().viewer_join(slot, tx);

    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => {
                if let Ok(v) = serde_json::from_str(t.as_str()) {
                    hub.lock().unwrap().viewer_signal(id, v);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    hub.lock().unwrap().viewer_leave(id);
}
