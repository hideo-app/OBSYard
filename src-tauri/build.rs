use std::path::PathBuf;
use std::process::Command;

fn main() {
    // il plugin OBS (../obs-plugin) viaggia dentro l'app, che lo installa dalla Regia
    build_plugin("obs-plugin", "obsyard_obs.dll", "obsyard-obs.dll");
    // Dichiarare i comandi genera i permessi "allow-<comando>":
    // lo studio (pagina remota) può chiamare SOLO i pub_*, la regia solo i suoi.
    let commands = &[
        "pub_ready", "pub_tracks", "pub_signal", "pub_thumbs", "pub_fullres", "pub_native_grow", "set_full_res",
        "get_state", "assign", "set_settings", "set_thumbs", "studio_action",
        "group_save", "group_delete", "group_remove", "set_obs", "group_end_fill",
        "output_add", "output_update", "output_move", "output_delete",
        "output_stats", "swap_outputs", "group_move", "set_lower_third", "apply_priority",
        "obs_plugin_status", "obs_plugin_install",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("tauri build");
}

/// Compila il plugin (come CueRider) e lo copia in OUT_DIR, dove l'app lo include. Senza
/// (non Windows, compilazione fallita) l'app si compila lo stesso e dice che il plugin manca.
fn build_plugin(dir: &str, built_name: &str, out_name: &str) {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(out_name);
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let plugin = manifest.join("..").join(dir);
    println!("cargo:rerun-if-changed={}", plugin.join("src").display());
    println!("cargo:rerun-if-changed={}", plugin.join("Cargo.toml").display());
    // i layout che plugin e app condividono
    println!("cargo:rerun-if-changed={}", manifest.join("..").join("shm-proto").join("src").display());
    let empty = || {
        let _ = std::fs::write(&out, []);
    };
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || !plugin.join("Cargo.toml").exists() {
        return empty();
    }
    let target_dir = manifest.join("target").join(dir);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--manifest-path"])
        .arg(plugin.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        // le impostazioni della build esterna non sono del plugin
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_FEATURES")
        .status();
    let built = target_dir.join("release").join(built_name);
    match status {
        Ok(s) if s.success() && built.exists() => {
            if std::fs::copy(&built, &out).is_err() {
                empty();
            }
        }
        _ => {
            println!("cargo:warning={dir} non compilato: l'app non lo includerà");
            empty();
        }
    }
}
