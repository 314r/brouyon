//! What a Mac expects of an app besides its window: a menu bar, the keys that hide
//! and minimise, a theme that follows the system's light or dark appearance, and
//! quitting once the window is closed.

use crate::{
    app::{
        self, Brouyon, CloseWindow, CopyNote, DecreaseTextSize, FindNote, IncreaseTextSize,
        MoveToTrash, NewNote, NextNote, OpenSettings, PreviousNote, Quit, ResetTextSize,
        ToggleShortcuts, ToggleTrash,
    },
    config::ThemeMode,
};
use gpui_kit::{
    App, Entity, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, Window,
    base::input as editor,
};

gpui_kit::actions!(brouyon, [Hide, HideOthers, ShowAll, Minimize, Zoom]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| in_active_window(cx, Window::minimize_window));
    cx.on_action(|_: &Zoom, cx| in_active_window(cx, Window::zoom_window));
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        // Settings has its own, in its own context.
        KeyBinding::new("cmd-w", CloseWindow, Some(app::KEY_CONTEXT)),
    ]);
    // Each item shows the key bound to its action, so it's set once the keys are.
    cx.set_menus(menus());

    // A Mac app keeps running when its windows are closed, but Brouyon is its window:
    // without one, clicking its icon in the Dock would do nothing.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// The theme that follows the system, which on a Mac is its light or dark appearance,
/// changes with it.
pub fn follow_appearance(brouyon: &Entity<Brouyon>, window: &mut Window) {
    let brouyon = brouyon.downgrade();
    window
        .observe_window_appearance(move |_, cx| {
            let follows = brouyon
                .upgrade()
                .is_some_and(|brouyon| brouyon.read(cx).config().theme == ThemeMode::Omarchy);
            if follows {
                app::apply_theme(ThemeMode::Omarchy, cx);
            }
        })
        .detach();
}

fn in_active_window(cx: &mut App, act: fn(&Window)) {
    if let Some(window) = cx.active_window() {
        window.update(cx, |_, window, _| act(window)).ok();
    }
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Brouyon").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Brouyon", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Brouyon", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Note", NewNote),
            MenuItem::separator(),
            MenuItem::action("Copy Note", CopyNote),
            MenuItem::action("Move to Trash", MoveToTrash),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        // Named so, macOS adds its own items to it, such as dictation and emoji.
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", editor::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", editor::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", editor::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", editor::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", editor::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", editor::SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find a Note", FindNote),
        ]),
        Menu::new("View").items([
            MenuItem::action("Previous Note", PreviousNote),
            MenuItem::action("Next Note", NextNote),
            MenuItem::action("Notes or Trash", ToggleTrash),
            MenuItem::separator(),
            MenuItem::action("Bigger Text", IncreaseTextSize),
            MenuItem::action("Smaller Text", DecreaseTextSize),
            MenuItem::action("Actual Size", ResetTextSize),
        ]),
        // Named so, macOS lists the open windows in it.
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        Menu::new("Help").items([MenuItem::action("Keyboard Shortcuts", ToggleShortcuts)]),
    ]
}
