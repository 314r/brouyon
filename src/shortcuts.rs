//! The keys of Brouyon's shortcuts on each system, and how they are written there:
//! with Ctrl on Linux, and with ⌘ on macOS, where the editor already takes it for
//! undo, copy and paste.

/// A binding on the key that holds shortcuts on this system: `primary!("n")` is
/// `ctrl-n` on Linux and `cmd-n` on macOS. A macro, because `KeyBinding::new` wants
/// a literal, and refuses GPUI's own `secondary-`.
#[cfg(not(target_os = "macos"))]
macro_rules! primary {
    ($keys:literal) => {
        concat!("ctrl-", $keys)
    };
}

#[cfg(target_os = "macos")]
macro_rules! primary {
    ($keys:literal) => {
        concat!("cmd-", $keys)
    };
}

pub(crate) use primary;

#[cfg(not(target_os = "macos"))]
mod written {
    pub const NEW_NOTE: &str = "Ctrl+N";
    pub const FIND_NOTE: &str = "Ctrl+F";
    pub const PREVIOUS_NEXT: &str = "Alt+↑ / Alt+↓";
    pub const TOGGLE_TASK: &str = "Ctrl+Enter";
    pub const COPY_NOTE: &str = "Ctrl+Shift+C";
    pub const MOVE_TO_TRASH: &str = "Ctrl+Shift+Backspace";
    pub const TOGGLE_TRASH: &str = "Ctrl+Shift+T";
    pub const UNDO_REDO: &str = "Ctrl+Z / Ctrl+Shift+Z";
    pub const SETTINGS: &str = "Ctrl+,";
    pub const TEXT_SIZE: &str = "Ctrl+Plus, Ctrl+Minus, Ctrl+0";
    pub const SHORTCUTS: &str = "F1";
    pub const QUIT: &str = "Ctrl+Q";
}

/// As macOS's own menus write them, with the modifiers in their usual order.
#[cfg(target_os = "macos")]
mod written {
    pub const NEW_NOTE: &str = "⌘N";
    pub const FIND_NOTE: &str = "⌘F";
    pub const PREVIOUS_NEXT: &str = "⌥↑ / ⌥↓";
    pub const TOGGLE_TASK: &str = "⌘↩";
    pub const COPY_NOTE: &str = "⇧⌘C";
    pub const MOVE_TO_TRASH: &str = "⇧⌘⌫";
    pub const TOGGLE_TRASH: &str = "⇧⌘T";
    pub const UNDO_REDO: &str = "⌘Z / ⇧⌘Z";
    pub const SETTINGS: &str = "⌘,";
    pub const TEXT_SIZE: &str = "⌘+, ⌘−, ⌘0";
    // A Mac's F1 needs Fn, unless the keyboard is set otherwise.
    pub const SHORTCUTS: &str = "⌘/ or F1";
    pub const QUIT: &str = "⌘Q";
}

pub use written::*;

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
