use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{
    Emitter, Manager, TitleBarStyle, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    menu::{Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder},
};

mod native_transport;

// One-line addition per command. The `id` strings here are the
// contract with the JS side — see `src/lib/commands.ts` where the
// same ids appear, each paired with the same `event` name. The
// hand-coded submenus in `build_menu` decide *where* in the menu
// each id appears (so a new command is also one new `.item(...)`
// line in the appropriate submenu).
struct CommandDef {
    id: &'static str,
    label: &'static str,
    accelerator: &'static str,
    event: &'static str,
}

const COMMAND_LIST: &[CommandDef] = &[
    CommandDef {
        id: "new-chat",
        label: "New Chat",
        accelerator: "CmdOrCtrl+N",
        event: "room://new-chat",
    },
    CommandDef {
        id: "new-window",
        label: "New Window",
        accelerator: "CmdOrCtrl+Shift+N",
        event: "room://new-window",
    },
    CommandDef {
        id: "toggle-palette",
        label: "Command Palette",
        accelerator: "CmdOrCtrl+K",
        event: "room://toggle-palette",
    },
    CommandDef {
        id: "toggle-sidebar",
        label: "Toggle Sidebar",
        accelerator: "CmdOrCtrl+1",
        event: "room://toggle-sidebar",
    },
    CommandDef {
        id: "open-settings",
        label: "Settings…",
        accelerator: "CmdOrCtrl+,",
        event: "room://open-settings",
    },
    CommandDef {
        id: "next-thread",
        label: "Next Chat",
        accelerator: "CmdOrCtrl+BracketRight",
        event: "room://next-thread",
    },
    CommandDef {
        id: "previous-thread",
        label: "Previous Chat",
        accelerator: "CmdOrCtrl+BracketLeft",
        event: "room://previous-thread",
    },
];

#[cfg(target_os = "macos")]
fn apply_traffic_lights_visible(window: &tauri::WebviewWindow, visible: bool) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let raw = match window.ns_window() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("room: ns_window failed: {err}");
            return;
        }
    };
    let ns_window_ptr = raw as *mut NSWindow;
    if ns_window_ptr.is_null() {
        return;
    }
    unsafe {
        let ns_window = &*ns_window_ptr;
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(btn) = ns_window.standardWindowButton(kind) {
                btn.setHidden(!visible);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_traffic_lights_visible(_window: &tauri::WebviewWindow, _visible: bool) {}

#[tauri::command]
fn set_traffic_lights(window: tauri::WebviewWindow, visible: bool) -> tauri::Result<()> {
    let w = window.clone();
    window.run_on_main_thread(move || apply_traffic_lights_visible(&w, visible))
}

// Match `main`'s look: blurred sidebar vibrancy + hidden traffic
// lights (revealed by the JS reveal-zone hover). Called both during
// setup for the configured `main` window and on every spawn_window
// call so every Room window looks identical.
fn decorate_room_window(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy};
        let _ = apply_vibrancy(
            window,
            NSVisualEffectMaterial::Sidebar,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            None,
        );
    }
    apply_traffic_lights_visible(window, false);
}

// Per-call counter feeds both the unique window label and the
// ephemeral identity suffix below. Starts at 2 because `main` is 1.
static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(2);

const EPHEMERAL_ANIMALS: &[&str] = &[
    "otter", "fox", "heron", "lynx", "panda", "whale", "koala", "crane", "badger", "puffin",
    "wolf", "seal", "finch", "ibex", "tapir", "gecko",
];

fn ephemeral_name(n: u64) -> String {
    let animal = EPHEMERAL_ANIMALS[(n as usize) % EPHEMERAL_ANIMALS.len()];
    let suffix = 100 + (n % 900);
    format!("{animal}-{suffix}")
}

// Open a second Room window. The `?as=<name>` query is read by the
// JS identity layer (lib/identity.ts) so the new window shows up as a
// distinct user in the presence stack — useful for testing the
// multiplayer flow without juggling browsers.
#[tauri::command]
fn spawn_window(app: tauri::AppHandle) -> tauri::Result<()> {
    let n = WINDOW_COUNTER.fetch_add(1, Ordering::SeqCst);
    let label = format!("room-{n}");
    let identity = ephemeral_name(n);
    let url_path = format!("index.html?as={identity}");

    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url_path.into()))
        .title("Room")
        .inner_size(1200.0, 800.0)
        .min_inner_size(720.0, 480.0)
        .resizable(true)
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true)
        .transparent(true)
        .build()?;

    decorate_room_window(&win);
    Ok(())
}

#[cfg(target_os = "macos")]
fn perform_haptic_impl(pattern: isize) {
    use objc2_app_kit::{
        NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
        NSHapticFeedbackPerformer,
    };

    let pat = match pattern {
        2 => NSHapticFeedbackPattern::LevelChange,
        0 => NSHapticFeedbackPattern::Generic,
        _ => NSHapticFeedbackPattern::Alignment,
    };
    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(pat, NSHapticFeedbackPerformanceTime::Default);
}

#[cfg(not(target_os = "macos"))]
fn perform_haptic_impl(_pattern: isize) {}

#[tauri::command]
fn haptic_feedback(window: tauri::WebviewWindow, kind: Option<String>) -> tauri::Result<()> {
    let pattern: isize = match kind.as_deref() {
        Some("level") => 2,
        Some("generic") => 0,
        _ => 1,
    };
    window.run_on_main_thread(move || perform_haptic_impl(pattern))
}

fn build_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // Build one MenuItem per command up front, keyed by id, then pull
    // them into their submenus.
    let mut by_id: HashMap<&str, MenuItem<tauri::Wry>> = HashMap::new();
    for cmd in COMMAND_LIST {
        let item = MenuItemBuilder::with_id(cmd.id, cmd.label)
            .accelerator(cmd.accelerator)
            .build(app)?;
        by_id.insert(cmd.id, item);
    }
    let get = |id: &str| {
        by_id
            .get(id)
            .unwrap_or_else(|| panic!("missing command item: {id}"))
    };

    let app_name = app.package_info().name.clone();
    let app_submenu = SubmenuBuilder::new(app, &app_name)
        .about(None)
        .separator()
        .item(get("open-settings"))
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file_submenu = SubmenuBuilder::new(app, "File")
        .item(get("new-chat"))
        .item(get("new-window"))
        .separator()
        .close_window()
        .build()?;

    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, "View")
        .item(get("toggle-palette"))
        .item(get("toggle-sidebar"))
        .separator()
        .item(get("previous-thread"))
        .item(get("next-thread"))
        .separator()
        .fullscreen()
        .build()?;

    let window_submenu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;

    MenuBuilder::new(app)
        .item(&app_submenu)
        .item(&file_submenu)
        .item(&edit_submenu)
        .item(&view_submenu)
        .item(&window_submenu)
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Disables WKWebView's hard 60fps cap on macOS 13-15 by
        // toggling WebKit's PreferPageRenderingUpdatesNear60FPSEnabled
        // private preference. No-op everywhere else.
        .plugin(tauri_plugin_macos_fps::init())
        // Registers the `room://` URL scheme so right-click → Share /
        // Copy Link on a thread row hands out a deep link that opens
        // the app and routes to the same thread on the recipient.
        .plugin(tauri_plugin_deep_link::init())
        .manage(native_transport::NativeTransportManager::default())
        .invoke_handler(tauri::generate_handler![
            set_traffic_lights,
            haptic_feedback,
            spawn_window,
            native_transport::native_transport_connect,
            native_transport::native_transport_send_loro,
            native_transport::native_transport_send_datagram,
            native_transport::native_transport_close
        ])
        .setup(|app| {
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;

            app.on_menu_event(|app, event| {
                let id = event.id().as_ref();
                let Some(cmd) = COMMAND_LIST.iter().find(|c| c.id == id) else {
                    return;
                };
                // Menu accelerators are app-global, but the UI state
                // they toggle (sidebar collapse, palette, etc.) is
                // per-window. Find the focused window and emit only
                // to it; broadcasting via `app.emit` makes every
                // window toggle in lockstep, which is wrong. Fall back
                // to a broadcast if nothing is focused so the menu
                // still works when the click came from outside any
                // window.
                let focused = app
                    .webview_windows()
                    .into_iter()
                    .find(|(_, w)| w.is_focused().unwrap_or(false))
                    .map(|(label, _)| label);

                let result = match focused {
                    Some(label) => {
                        app.emit_to(tauri::EventTarget::webview_window(&label), cmd.event, ())
                    }
                    None => app.emit(cmd.event, ()),
                };
                if let Err(err) = result {
                    eprintln!("room: failed to emit {}: {err}", cmd.event);
                }
            });

            if let Some(main) = app.get_webview_window("main") {
                decorate_room_window(&main);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
