fn main() {
    // Dichiarare i comandi genera i permessi "allow-<comando>":
    // lo studio (pagina remota) può chiamare SOLO i pub_*, la regia solo i suoi.
    let commands = &[
        "pub_ready", "pub_tracks", "pub_signal", "pub_thumbs", "pub_fullres", "set_full_res",
        "get_state", "assign", "set_settings", "set_thumbs", "studio_action",
        "group_save", "group_delete", "group_remove", "set_obs",
        "output_add", "output_update", "output_move", "output_delete",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("tauri build");
}
