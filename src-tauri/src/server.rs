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
        // solo per la pagina di test: WebSocket che rimanda indietro i messaggi, così la pagina
        // può simulare il protocollo di StreamYard (lo script dello studio li vede come "in arrivo")
        .route("/test/echo", get(|ws: WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket: WebSocket| async move {
                while let Some(Ok(Message::Text(t))) = socket.recv().await {
                    if socket.send(Message::Text(t)).await.is_err() {
                        break;
                    }
                }
            })
        }))
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
        .map(|o| {
            let by_name = url_escape(&o.name);
            format!(
                "<li><a href=\"/slot/{0}\">{1}</a> <code>/slot/{0}</code> · <a href=\"/slot/{2}\"><code>/slot/{3}</code></a></li>",
                o.id,
                html_escape(&o.name),
                by_name,
                html_escape(&by_name)
            )
        })
        .collect();
    Html(format!(
        "<!doctype html><meta charset=utf-8><title>OBSYard</title>\
         <body style=\"font:14px system-ui;background:#111;color:#ddd\">\
         <h3>OBSYard – fonti per OBS</h3><ul>{links}</ul>\
         <p>Parametri: <code>?audio=0</code> (solo video), <code>?fit=cover</code>, <code>?bg=000000</code></p>"
    ))
}

/// Codifica un nome per il percorso dell'URL (spazi e caratteri non sicuri in %XX).
pub fn url_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// /slot/3 (id) oppure /slot/Cam%201 (nome dell'uscita). Il nome si risolve all'apertura della
/// pagina: rinominata l'uscita, la fonte continua a funzionare finché OBS non la ricarica.
async fn slot_page(Path(key): Path<String>, State(hub): State<SharedHub>) -> Response {
    let Some(n) = hub.lock().unwrap().output_by_key(&key) else {
        return (StatusCode::NOT_FOUND, "uscita inesistente").into_response();
    };
    Html(SLOT_HTML.replace("__SLOT__", &n.to_string())).into_response()
}

/// Pagina dell'uscita. Le regole del sottopancia hanno specificità zero (:where), così un
/// selettore semplice nel "CSS personalizzato" della Fonte browser le batte; classi e variabili
/// sono le stesse di guestroom.live (.lower-third, --lt-*).
const SLOT_HTML: &str = r#"<!doctype html><html><head><meta charset=utf-8><title>OBSYard slot __SLOT__</title>
<style>html,body{margin:0;height:100%;overflow:hidden;background:transparent}
video{position:fixed;inset:0;width:100%;height:100%;object-fit:contain;opacity:0}
video.on{opacity:1}
:where(.lower-third){position:fixed;left:var(--lt-offset-x,4vw);bottom:var(--lt-offset-y,7vh);z-index:2;display:flex;flex-direction:column;gap:.4vh;max-width:80vw;padding:1.2vh 2.4vh 1.3vh 2vh;background:var(--lt-bg,rgb(12 18 32/.88));color:var(--lt-fg,#fff);border-left:.8vh solid var(--lt-accent,#ff5c48);border-radius:var(--lt-radius,.8vh);font-family:var(--lt-font,'Segoe UI',system-ui,sans-serif);transition:opacity .35s ease,transform .35s ease}
:where(.lower-third[data-state=out]){opacity:0;transform:translateX(-2vw)}
:where(.lower-third-name){font-size:var(--lt-name-size,4.4vh);font-weight:700;line-height:1.15}
:where(.lower-third-title){font-size:var(--lt-title-size,2.6vh);line-height:1.2;opacity:.85}
@media(prefers-reduced-motion:reduce){:where(.lower-third){transition:none}}</style></head>
<body><video id=v autoplay playsinline disablepictureinpicture></video>
<script>window.__SLOT=__SLOT__</script><script src="/viewer.js"></script></body></html>"#;

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
    /// id dell'uscita o suo nome ("Cam 1")
    slot: String,
    /// id traccia o parte del nome dell'ospite; "none" per svuotare
    video: Option<String>,
    /// id, nome, "auto" o "none" (default: auto)
    audio: Option<String>,
}

/// Per Stream Deck / script: GET /api/assign?slot=1&video=Mario
async fn api_assign(Query(q): Query<AssignQuery>, State(hub): State<SharedHub>) -> Response {
    let mut h = hub.lock().unwrap();
    let Some(slot) = h.output_by_key(&q.slot) else {
        return (StatusCode::NOT_FOUND, "uscita inesistente").into_response();
    };
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
    h.assign(slot, crate::hub::SlotAssign { video, audio }, "queue"); // API: nessuna domanda
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
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(t.as_str()) {
                    let mut h = hub.lock().unwrap();
                    match v["type"].as_str() {
                        // tally e statistiche restano nell'hub: allo studio va solo il signaling
                        Some("tally") => h.viewer_tally(id, v["active"] == true, v["visible"] == true),
                        Some("stats") => h.viewer_stats(id, v["stats"].clone()),
                        _ => h.viewer_signal(id, v),
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    hub.lock().unwrap().viewer_leave(id);
}
