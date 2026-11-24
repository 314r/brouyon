use crate::{
    app::{
        self, Brouyon, CloseWindow, CopyNote, DecreaseTextSize, FindNote, IncreaseTextSize,
        MoveToTrash, NewNote, NextNote, OpenSettings, PreviousNote, Quit, ResetTextSize,
        ToggleShortcuts, ToggleTrash,
    },
    config::ThemeMode,
    kept,
};
use gpui_kit::base::ThemeAppearance;
use gpui_kit::{
    App, Entity, Hsla, KeyBinding, Menu, MenuItem, OsAction, Pixels, Point, Rgba, SystemMenuType,
    Window, base::input as editor, px,
};
use gpui_omarchy::Theme;

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
        KeyBinding::new("cmd-w", CloseWindow, Some(app::KEY_CONTEXT)),
    ]);
    // Each item shows the key bound to its action, so it's set once the keys are.
    cx.set_menus(menus());

    // A Mac app keeps running when its windows are closed, and its icon in the Dock would
    // then do nothing.
    cx.on_window_closed(|cx, _| kept::quit_without_windows(cx))
        .detach();
}

pub fn follow_appearance(brouyon: &Entity<Brouyon>, window: &mut Window, cx: &mut App) {
    let weak = brouyon.downgrade();
    window
        .observe_window_appearance(move |_, cx| {
            let follows = weak
                .upgrade()
                .is_some_and(|brouyon| brouyon.read(cx).config().theme == ThemeMode::Omarchy);
            if follows {
                app::apply_theme(ThemeMode::Omarchy, cx);
            }
        })
        .detach();
    brouyon.update(cx, |_, cx| {
        cx.observe_window_activation(window, |this, window, cx| {
            let stale = window.is_window_active()
                && system_accent().is_some_and(|accent| accent != cx.global::<Theme>().accent);
            if stale {
                app::apply_theme(this.config().theme, cx);
            }
        })
        .detach();
    });
}

/// What macOS 26 rounds: small marks such as a checkbox, then controls and the surfaces
/// drawn with them. gpui-omarchy only follows Hyprland's rounding.
pub const CORNERS: (f32, f32) = (4., 8.);

/// GPUI takes out the tint macOS gives its own blur, so what's behind the window
/// shows through as it is.
pub fn sidebar_opacity(theme: &Theme) -> f32 {
    match theme.appearance {
        ThemeAppearance::Light => 0.85,
        ThemeAppearance::Dark => 0.9,
    }
}

#[cfg(target_os = "macos")]
pub fn system_accent() -> Option<Hsla> {
    use objc2_app_kit::{NSColor, NSColorSpace};
    let accent =
        NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(
        Rgba {
            r: accent.redComponent() as f32,
            g: accent.greenComponent() as f32,
            b: accent.blueComponent() as f32,
            a: 1.,
        }
        .into(),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn system_accent() -> Option<Hsla> {
    let blue = Rgba {
        r: 0.,
        g: 0.478,
        b: 1.,
        a: 1.,
    };
    Some(blue.into())
}

pub fn use_system_accent(cx: &mut App) {
    let Some(accent) = system_accent() else {
        return;
    };
    let mut theme = cx.global::<Theme>().clone();
    if theme.accent == accent {
        return;
    }
    theme.accent = accent;
    theme.on_accent = gpui_kit::white();
    theme.chart[0] = accent;
    theme.apply(cx);
}

/// GPUI keeps its own copy, updated a moment after each of AppKit's notices, and
/// leaving full screen can leave that copy saying the window is in the background
/// while it's in front.
#[cfg(target_os = "macos")]
pub fn is_key_window(window: &Window) -> bool {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return window.is_window_active();
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return window.is_window_active();
    };
    // SAFETY: GPUI's Mac window hands out its live NSView, and this runs on the main
    // thread, where GPUI draws.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window().is_some_and(|window| window.isKeyWindow())
}

#[cfg(not(target_os = "macos"))]
pub fn is_key_window(window: &Window) -> bool {
    window.is_window_active()
}

pub fn place_buttons(at: Point<Pixels>, window: &Window) {
    #[cfg(target_os = "macos")]
    window.set_traffic_light_position(at);
    #[cfg(not(target_os = "macos"))]
    let _ = (at, window);
}

/// Of the keys that gpui-base's editor only has on a Mac, those that tests press.
#[cfg(test)]
pub fn bind_editor_keys(cx: &mut App) {
    if crate::look::MAC && cfg!(not(target_os = "macos")) {
        cx.bind_keys([
            KeyBinding::new("cmd-a", editor::SelectAll, Some("Input")),
            KeyBinding::new("cmd-z", editor::Undo, Some("Input")),
        ]);
    }
}

/// How round macOS 26 makes a window with no toolbar, such as Brouyon's.
pub const WINDOW_CORNER: f32 = 16.;

pub fn round_corners(cx: &mut App) {
    let (small, control) = CORNERS;
    let radius = &mut gpui_kit::base::Theme::global_mut(cx).tokens.radius;
    radius.none = px(0.);
    radius.sm = px(small);
    radius.md = px(control);
    radius.lg = px(control);
    radius.xl = px(control);
    radius.full = px(9999.);
    cx.refresh_windows();
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{black, white};

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let luminance = |colour: Hsla| {
            let Rgba { r, g, b, .. } = colour.into();
            let linear = |c: f32| {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn over(tint: Hsla, alpha: f32, behind: Hsla) -> Hsla {
        let (tint, behind): (Rgba, Rgba) = (tint.into(), behind.into());
        let mix = |t: f32, b: f32| alpha * t + (1. - alpha) * b;
        Rgba {
            r: mix(tint.r, behind.r),
            g: mix(tint.g, behind.g),
            b: mix(tint.b, behind.b),
            a: 1.,
        }
        .into()
    }

    #[test]
    fn the_sidebar_text_stays_readable_whatever_is_behind_the_window() {
        for theme in [Theme::tokyo_night(), Theme::flexoki_light()] {
            let alpha = sidebar_opacity(&theme);
            for behind in [white(), black()] {
                let sidebar = over(theme.background, alpha, behind);
                for text in [theme.foreground, theme.secondary] {
                    let ratio = contrast(text, sidebar);
                    assert!(ratio >= 4.5, "{}: {ratio:.1}:1", theme.name);
                }
            }
        }
    }
}
