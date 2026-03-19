#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::get_app_state,])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
