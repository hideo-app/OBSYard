//! Installa il plugin OBS di OBSYard (compilato con l'app, vedi build.rs) per l'OBS di questo PC:
//! %ProgramData%\obs-studio\plugins\obsyard-obs\bin\64bit\obsyard-obs.dll (niente diritti di
//! amministratore, la stessa cartella che usa CueRider).

use serde::Serialize;
use std::path::PathBuf;

static PLUGIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/obsyard-obs.dll"));

fn target() -> PathBuf {
    let data = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| r"C:\ProgramData".into());
    data.join("obs-studio").join("plugins").join("obsyard-obs").join("bin").join("64bit").join("obsyard-obs.dll")
}

fn obs_installed() -> bool {
    let programs = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into());
    programs.join("obs-studio").join("bin").join("64bit").join("obs64.exe").exists()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    /// l'app porta il plugin
    pub available: bool,
    pub obs_found: bool,
    pub installed: bool,
    /// installato e uguale a quello dell'app
    pub current: bool,
    pub path: String,
}

#[tauri::command]
pub fn obs_plugin_status() -> PluginStatus {
    let path = target();
    let on_disk = std::fs::read(&path).ok();
    PluginStatus {
        available: !PLUGIN.is_empty(),
        obs_found: obs_installed(),
        installed: on_disk.is_some(),
        current: !PLUGIN.is_empty() && on_disk.as_deref() == Some(PLUGIN),
        path: path.display().to_string(),
    }
}

/// copia il plugin nella cartella dei plugin di OBS (OBS va riavviato per caricarlo)
#[tauri::command]
pub fn obs_plugin_install() -> Result<PluginStatus, String> {
    if PLUGIN.is_empty() {
        return Err("questa versione di OBSYard non include il plugin OBS".into());
    }
    let path = target();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("cartella dei plugin di OBS: {e}"))?;
    std::fs::write(&path, PLUGIN).map_err(|e| {
        if e.raw_os_error() == Some(32) {
            "il plugin è in uso: chiudi OBS e riprova".to_string()
        } else {
            format!("installazione del plugin: {e}")
        }
    })?;
    Ok(obs_plugin_status())
}
