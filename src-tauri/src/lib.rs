mod groups;
mod hub;
mod native;
mod obs_plugin;
mod server;

use groups::GroupConfig;
use hub::{Hub, ObsConfig, SharedHub, SlotAssign, Settings};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

pub const PORT: u16 = 7878;
const STUDIO_JS: &str = include_str!("../gen/studio.js");

/// Stessi argomenti per TUTTE le webview (WebView2 condivide l'environment:
/// argomenti diversi farebbero fallire la creazione della seconda finestra).
/// - niente throttling quando la finestra è in background/occlusa/minimizzata
/// - candidati ICE con IP reali (no mDNS) per il loopback verso OBS
/// - autoplay senza gesto utente
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion,WebRtcHideLocalIpsWithMdns,IntensiveWakeUpThrottling --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows --autoplay-policy=no-user-gesture-required";

fn browser_args() -> String {
    // OBSYARD_CDP_PORT=9222 -> remote debugging (Chrome DevTools Protocol) per diagnosi
    match std::env::var("OBSYARD_CDP_PORT") {
        Ok(p) => format!("{BROWSER_ARGS} --remote-debugging-port={p}"),
        Err(_) => BROWSER_ARGS.to_string(),
    }
}

/// Popup aperti da StreamYard con window.open (es. "Accedi con Google").
/// Senza questo handler Tauri li blocca e il click non fa nulla.
/// La finestra condivide l'environment WebView2 dello studio, quindi
/// cookie e window.opener funzionano e il login torna allo studio.
fn open_popup(
    app: tauri::AppHandle,
) -> impl Fn(url::Url, tauri::webview::NewWindowFeatures) -> tauri::webview::NewWindowResponse<tauri::Wry> + Send + 'static {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    move |url, features| {
        let label = format!("popup-{}", N.fetch_add(1, Ordering::Relaxed));
        let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External("about:blank".parse().unwrap()))
            .window_features(features)
            .title(url.host_str().unwrap_or("OBSYard"))
            .on_document_title_changed(|w, title| {
                let _ = w.set_title(&title);
            })
            .build();
        match built {
            Ok(window) => tauri::webview::NewWindowResponse::Create { window },
            Err(e) => {
                eprintln!("[obsyard] popup non creato: {e}");
                tauri::webview::NewWindowResponse::Allow
            }
        }
    }
}

fn studio_url(platform: &str) -> String {
    std::env::var("OBSYARD_URL").unwrap_or_else(|_| hub::platform_url(platform).into())
}

// ---------- comandi chiamati dalla pagina StreamYard (publisher) ----------

#[tauri::command]
fn pub_ready(hub: State<SharedHub>) {
    hub.lock().unwrap().publisher_ready();
}

#[tauri::command]
fn pub_tracks(hub: State<SharedHub>, tracks: Value) {
    hub.lock().unwrap().set_tracks(tracks);
}

#[tauri::command]
fn pub_signal(hub: State<SharedHub>, viewer_id: u64, msg: Value) {
    hub.lock().unwrap().to_viewer(viewer_id, &msg);
}

#[tauri::command]
fn pub_fullres(hub: State<SharedHub>, status: String) {
    hub.lock().unwrap().full_res_report(status);
}

/// lo studio ha un fotogramma che non entra nel buffer verso il plugin OBS
#[tauri::command]
fn pub_native_grow(app: tauri::AppHandle, slot: u32, bytes: usize) {
    native::grow(&app, slot, bytes);
}

#[tauri::command]
fn set_full_res(hub: State<SharedHub>, on: bool) {
    hub.lock().unwrap().set_full_res(on);
}

#[tauri::command]
fn pub_thumbs(hub: State<SharedHub>, thumbs: Value) {
    hub.lock().unwrap().emit_thumbs(thumbs);
}

// ---------- comandi chiamati dalla UI di controllo ----------

#[tauri::command]
fn get_state(hub: State<SharedHub>) -> Value {
    hub.lock().unwrap().snapshot()
}

#[tauri::command]
fn assign(hub: State<SharedHub>, slot: u32, assign: SlotAssign, fill: Option<bool>, dup: Option<bool>, queue: Option<bool>) -> Value {
    // dalla Regia: chiede la prova di riempimento; fill = true quando confermata, dup = true "Duplica"
    let mode = if dup == Some(true) { "dup" } else if fill == Some(true) { "fill" } else if queue == Some(true) { "queue" } else { "ask" };
    hub.lock().unwrap().assign(slot, assign, mode)
}

#[tauri::command]
fn group_end_fill(hub: State<SharedHub>, id: u32) {
    hub.lock().unwrap().group_end_fill(id);
}

#[tauri::command]
fn set_settings(hub: State<SharedHub>, settings: Settings) {
    hub.lock().unwrap().set_settings(settings);
}

#[tauri::command]
fn group_save(hub: State<SharedHub>, group: GroupConfig) -> Result<u32, String> {
    hub.lock().unwrap().group_save(group)
}

#[tauri::command]
fn group_delete(hub: State<SharedHub>, id: u32) {
    hub.lock().unwrap().group_delete(id);
}

#[tauri::command]
fn group_remove(hub: State<SharedHub>, id: u32, track: String) {
    hub.lock().unwrap().group_remove(id, &track);
}

#[tauri::command]
fn output_add(hub: State<SharedHub>, name: Option<String>) -> u32 {
    hub.lock().unwrap().output_add(name)
}

#[tauri::command]
fn output_update(hub: State<SharedHub>, id: u32, name: Option<String>, bg: Option<String>, lt: Option<bool>) -> Result<(), String> {
    hub.lock().unwrap().output_update(id, name, bg, lt)
}

#[tauri::command]
fn output_stats(hub: State<SharedHub>, slot: u32) -> Value {
    hub.lock().unwrap().output_stats(slot)
}

#[tauri::command]
fn swap_outputs(hub: State<SharedHub>, a: u32, b: u32) {
    hub.lock().unwrap().swap_outputs(a, b);
}

#[tauri::command]
fn group_move(hub: State<SharedHub>, id: u32, track: String, delta: i32) {
    hub.lock().unwrap().group_move(id, &track, delta);
}

#[tauri::command]
fn apply_priority(hub: State<SharedHub>) {
    hub.lock().unwrap().apply_priority();
}

#[tauri::command]
fn set_lower_third(hub: State<SharedHub>, label: String, name: Option<String>, title: String) {
    hub.lock().unwrap().set_lower_third(&label, name, title);
}

#[tauri::command]
fn output_move(hub: State<SharedHub>, id: u32, delta: i32) {
    hub.lock().unwrap().output_move(id, delta);
}

#[tauri::command]
fn output_delete(hub: State<SharedHub>, id: u32) {
    hub.lock().unwrap().output_delete(id);
}

#[tauri::command]
fn set_obs(hub: State<SharedHub>, obs: ObsConfig) {
    hub.lock().unwrap().set_obs(obs);
}

#[tauri::command]
fn set_thumbs(hub: State<SharedHub>, on: bool) {
    hub.lock().unwrap().set_thumbs(on);
}

#[tauri::command]
fn studio_action(app: tauri::AppHandle, action: String, url: Option<String>) -> Result<(), String> {
    let w = app.get_webview_window("studio").ok_or("studio non trovato")?;
    match action.as_str() {
        "show" => {
            let _ = w.unminimize();
            w.show().map_err(|e| e.to_string())?;
            w.set_focus().map_err(|e| e.to_string())?;
        }
        "reload" => w.eval("location.reload()").map_err(|e| e.to_string())?,
        "devtools" => {
            #[cfg(debug_assertions)]
            w.open_devtools();
        }
        "navigate" => {
            let u = url.ok_or("url mancante")?;
            w.navigate(u.parse().map_err(|e: url::ParseError| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        _ => return Err("azione sconosciuta".into()),
    }
    Ok(())
}

pub fn run() {
    let hub: SharedHub = Arc::new(Mutex::new(Hub::new(PORT)));

    tauri::Builder::default()
        .manage(hub.clone())
        .invoke_handler(tauri::generate_handler![
            pub_ready, pub_tracks, pub_signal, pub_thumbs, pub_fullres, pub_native_grow, set_full_res,
            get_state, assign, set_settings, set_thumbs, studio_action,
            group_save, group_delete, group_remove, set_obs, group_end_fill,
            output_add, output_update, output_move, output_delete,
            output_stats, swap_outputs, group_move, set_lower_third, apply_priority,
            obs_plugin::obs_plugin_status, obs_plugin::obs_plugin_install
        ])
        .setup(move |app| {
            {
                let mut h = hub.lock().unwrap();
                h.app = Some(app.handle().clone());
                // OBSYARD_CONFIG_DIR: cartella di configurazione separata (test e sviluppo),
                // così non si tocca quella dell'app installata
                let dir = match std::env::var("OBSYARD_CONFIG_DIR") {
                    Ok(d) if !d.is_empty() => std::path::PathBuf::from(d),
                    _ => app.path().app_config_dir()?,
                };
                h.load(dir.join("config.json"));
            }
            let platform = hub.lock().unwrap().settings.platform.clone();
            native::start(app.handle().clone());
            let listener = server::bind(PORT)
                .map_err(|e| format!("porta 127.0.0.1:{PORT} occupata ({e}): OBSYard è già aperto?"))?;
            tauri::async_runtime::spawn(server::serve(hub.clone(), listener));

            let args = browser_args();
            WebviewWindowBuilder::new(app, "control", WebviewUrl::App("index.html".into()))
                .title("OBSYard – Regia")
                .inner_size(1100.0, 760.0)
                .min_inner_size(760.0, 480.0)
                // senza il gestore di Tauri per i file trascinati, altrimenti su Windows il
                // drag and drop HTML5 della Regia (sorgente → uscita) non funziona
                .disable_drag_drop_handler()
                .additional_browser_args(&args)
                .build()?;

            WebviewWindowBuilder::new(app, "studio", WebviewUrl::External(studio_url(&platform).parse()?))
                .title(hub::platform_title(&platform))
                .inner_size(1440.0, 900.0)
                .initialization_script(STUDIO_JS)
                .additional_browser_args(&args)
                .on_new_window(open_popup(app.handle().clone()))
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // chiudere la regia chiude l'app; chiudere lo studio lo nasconde soltanto;
            // i popup (login Google ecc.) si chiudono normalmente
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "studio" => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    "control" => window.app_handle().exit(0),
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("errore avvio OBSYard");
}
