//! Background mode: a tray icon, and closing the window hides it instead of quitting.
//!
//! Opt-in (Settings). While it is on, Sentinel keeps running in the notification area so that
//! low-space alerts work with the window closed. The tray menu has two items, Open Sentinel and
//! Quit Sentinel; a left click on the icon also opens the window. Starting Sentinel a second time
//! shows the running window (see `single_instance`).

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::startup::BACKGROUND_ARG;

const TRAY_ID: &str = "sentinel-tray";

/// Bring the main window to the front, whether it was hidden, minimized or behind others.
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Whether this start should leave the window hidden: only when started with `--background`
/// (by the sign-in entry) *and* background mode is on, so Sentinel is never invisible without a
/// tray icon to open it from.
pub(crate) fn wants_hidden_start(args: &[String], run_in_background: bool) -> bool {
    run_in_background && args.iter().any(|a| a == BACKGROUND_ARG)
}

/// Create or remove the tray icon to match `enabled`.
pub(crate) fn apply(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if !enabled {
        let _ = app.remove_tray_by_id(TRAY_ID);
        return Ok(());
    }
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    let build = || -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", "Open Sentinel", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit Sentinel", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        let mut builder = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip("Sentinel")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "open" => show_main_window(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        builder.build(app)?;
        Ok(())
    };
    build().map_err(|e| format!("Windows would not create the notification-area icon: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_hidden_start_needs_both_the_flag_and_background_mode() {
        assert!(wants_hidden_start(
            &args(&["sentinel.exe", "--background"]),
            true
        ));
        // Started at sign-in, but the person has since turned background mode off: show the window.
        assert!(!wants_hidden_start(
            &args(&["sentinel.exe", "--background"]),
            false
        ));
        // A normal start always shows the window.
        assert!(!wants_hidden_start(&args(&["sentinel.exe"]), true));
        assert!(!wants_hidden_start(&[], true));
    }
}
