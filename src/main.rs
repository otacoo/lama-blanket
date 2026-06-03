#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod config;
mod gguf;
mod hwdetect;
mod persistence;
mod server;
mod tray;

use std::sync::{mpsc, Arc, Mutex};

fn install_tray_handlers(
    event_tx: mpsc::Sender<app::AppEvent>,
    ctx: egui::Context,
    window_handle: Arc<Mutex<Option<isize>>>,
) {
    let tray_tx = event_tx.clone();
    let tray_ctx = ctx.clone();
    let tray_window_handle = window_handle.clone();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
        let should_show = matches!(
            event,
            tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } | tray_icon::TrayIconEvent::DoubleClick {
                button: tray_icon::MouseButton::Left,
                ..
            }
        );

        if should_show {
            app::restore_native_window(&tray_window_handle);
            let _ = tray_tx.send(app::AppEvent::ShowWindow);
            tray_ctx.request_repaint();
        }
    }));

    let menu_window_handle = window_handle;
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        let app_event = match event.id.0.as_str() {
            "show" => {
                app::restore_native_window(&menu_window_handle);
                Some(app::AppEvent::ShowWindow)
            }
            "quit" => Some(app::AppEvent::Quit),
            _ => None,
        };

        if let Some(app_event) = app_event {
            let _ = event_tx.send(app_event);
            ctx.request_repaint();
        }
    }));
}

fn main() -> Result<(), eframe::Error> {
    let icon_data = tray::load_icon();
    let (event_tx, event_rx) = mpsc::channel();
    let window_handle = Arc::new(Mutex::new(None));

    let tray_icon = tray_icon::Icon::from_rgba(
        icon_data.rgba.clone(),
        icon_data.width,
        icon_data.height,
    )
    .expect("failed to create tray icon");

    let menu = tray_icon::menu::Menu::new();
    let show_item = tray_icon::menu::MenuItem::with_id("show", "Show", true, None);
    let quit_item = tray_icon::menu::MenuItem::with_id("quit", "Quit", true, None);
    let _ = menu.append_items(&[&show_item, &quit_item]);

    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("Lama Blanket")
        .with_icon(tray_icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .expect("failed to build tray icon");

    let window_icon = egui::IconData {
        rgba: icon_data.rgba,
        width: icon_data.width,
        height: icon_data.height,
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 700.0])
            .with_title("Lama Blanket")
            .with_icon(std::sync::Arc::new(window_icon)),
        ..Default::default()
    };

    eframe::run_native(
        "Lama Blanket",
        native_options,
        Box::new(move |cc| {
            install_tray_handlers(
                event_tx.clone(),
                cc.egui_ctx.clone(),
                window_handle.clone(),
            );

            let mut app = app::LamaBlanketApp::new(event_rx, window_handle.clone());
            app.apply_theme(&cc.egui_ctx);
            app.tray = Some(tray);
            Ok(Box::new(app))
        }),
    )
}
