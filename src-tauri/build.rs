fn main() {
    // Dichiarare i comandi genera i permessi "allow-<comando>":
    // lo studio (pagina remota) può chiamare SOLO i pub_*, la regia solo i suoi.
    let commands = &[
        "pub_ready", "pub_tracks", "pub_signal", "pub_thumbs", "pub_fullres", "pub_native_grow", "set_full_res",
        "get_state", "assign", "set_settings", "set_thumbs", "studio_action",
        "group_save", "group_delete", "group_remove", "set_obs", "group_end_fill",
        "output_add", "output_update", "output_move", "output_delete",
        "output_stats", "swap_outputs", "group_move", "set_lower_third", "apply_priority",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("tauri build");
}
