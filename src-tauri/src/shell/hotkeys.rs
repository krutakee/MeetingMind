//! Global keyboard shortcuts: start/stop and pause/resume, working while any program has the focus.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::storage::settings::Hotkeys;

/// What a shortcut does when pressed.
type Action = fn(&AppHandle);

/// Words for the user when a shortcut cannot be used. `what` names the action ("start/stop").
fn problem_for(keys: &str, what: &str, why: &str) -> String {
    format!("The {what} shortcut \"{keys}\" cannot be used: {why}")
}

/// Registers both shortcuts, replacing whatever was registered before. All or nothing: if either cannot be
/// registered, none stays registered and the message says which one and why (usually another program has it).
pub fn apply(app: &AppHandle, hotkeys: &Hotkeys) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();

    let wanted: [(&str, &str, Action); 2] = [
        (&hotkeys.start_stop, "start/stop", super::toggle_start_stop),
        (&hotkeys.pause_resume, "pause/resume", super::toggle_pause),
    ];
    for (keys, what, action) in wanted {
        let parsed: Shortcut = match keys.parse() {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = shortcuts.unregister_all();
                return Err(problem_for(
                    keys,
                    what,
                    &format!("it is not a valid key combination ({error}). Example: Ctrl+Shift+S."),
                ));
            }
        };
        let registered = shortcuts.on_shortcut(parsed, move |app, _shortcut, event| {
            // Act once per key press, not again when the key is released.
            if event.state == ShortcutState::Pressed {
                action(app);
            }
        });
        if let Err(error) = registered {
            let _ = shortcuts.unregister_all();
            return Err(problem_for(
                keys,
                what,
                &format!(
                    "another program may already be using it ({error}). Choose a different combination in Settings."
                ),
            ));
        }
    }
    Ok(())
}
