#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::SyncGuard;

fn main() {
    let builder = commands::make_builder();

    tauri::Builder::default()
        .manage(SyncGuard::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
