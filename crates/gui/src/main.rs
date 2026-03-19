#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::SyncGuard;

fn main() {
    tauri::Builder::default()
        .manage(SyncGuard::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::init_config,
            commands::start_auth,
            commands::start_sync,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
