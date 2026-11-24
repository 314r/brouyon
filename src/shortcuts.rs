//! The keys of Brouyon's shortcuts on each system, and how they are written there:
//! with Ctrl on Linux, and with ⌘ on macOS, where the editor already takes it for
//! undo, copy and paste.

use crate::look::MAC;

/// A binding on the key that holds shortcuts on this system: `primary!("n")` is
/// `ctrl-n` on Linux and `cmd-n` on macOS. A macro, because `KeyBinding::new` wants
/// a literal, and refuses GPUI's own `secondary-`.
macro_rules! primary {
    ($keys:literal) => {
        if crate::look::MAC {
            concat!("cmd-", $keys)
        } else {
            concat!("ctrl-", $keys)
        }
    };
}

pub(crate) use primary;

/// On a Mac, as macOS's own menus write them, with the modifiers in their usual order.
const fn written(on_linux: &'static str, on_mac: &'static str) -> &'static str {
    if MAC { on_mac } else { on_linux }
}

pub const NEW_NOTE: &str = written("Ctrl+N", "⌘N");
pub const FIND_NOTE: &str = written("Ctrl+F", "⌘F");
pub const PREVIOUS_NEXT: &str = written("Alt+↑ / Alt+↓", "⌥↑ / ⌥↓");
pub const TOGGLE_TASK: &str = written("Ctrl+Enter", "⌘↩");
pub const COPY_NOTE: &str = written("Ctrl+Shift+C", "⇧⌘C");
pub const MOVE_TO_TRASH: &str = written("Ctrl+Shift+Backspace", "⇧⌘⌫");
pub const TOGGLE_TRASH: &str = written("Ctrl+Shift+T", "⇧⌘T");
pub const UNDO_REDO: &str = written("Ctrl+Z / Ctrl+Shift+Z", "⌘Z / ⇧⌘Z");
pub const SETTINGS: &str = written("Ctrl+,", "⌘,");
pub const TEXT_SIZE: &str = written("Ctrl+Plus, Ctrl+Minus, Ctrl+0", "⌘+, ⌘−, ⌘0");
// A Mac's F1 needs Fn, unless the keyboard is set otherwise.
pub const SHORTCUTS: &str = written("F1", "⌘/ or F1");
pub const QUIT: &str = written("Ctrl+Q", "⌘Q");

/// What the shortcuts overlay lists, in the order of the README's table.
pub const ALL: [(&str, &str); 13] = [
    (NEW_NOTE, "New note"),
    (FIND_NOTE, "Find a note"),
    (PREVIOUS_NEXT, "Previous / next note"),
    (TOGGLE_TASK, "Tick a task"),
    (COPY_NOTE, "Copy the note"),
    (MOVE_TO_TRASH, "Move to Trash"),
    (TOGGLE_TRASH, "Switch between Notes and Trash"),
    ("Esc", "Stop finding, or leave the Trash"),
    (UNDO_REDO, "Undo / redo"),
    (SETTINGS, "Settings"),
    (TEXT_SIZE, "Text size"),
    (SHORTCUTS, "These shortcuts"),
    (QUIT, "Quit"),
];
