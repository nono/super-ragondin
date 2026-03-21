#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::SyncGuard;
use commands::TRAY_ID;
use commands::TRAY_IDLE_BYTES;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

fn main() {
    let builder = commands::make_builder();

    tauri::Builder::default()
        .manage(SyncGuard::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            // Build tray menu
            let show_item = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let idle_icon = Image::from_bytes(TRAY_IDLE_BYTES)?;

            let window = app
                .get_webview_window("main")
                .expect("main window not found");

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(idle_icon)
                .menu(&menu)
                .on_tray_icon_event({
                    let tray_window = window.clone();
                    move |_tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            tray_window.show().ok();
                            tray_window.set_focus().ok();
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            w.show().ok();
                            w.set_focus().ok();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Hide to tray instead of quitting when the window is closed
            let close_window = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    close_window.hide().ok();
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
