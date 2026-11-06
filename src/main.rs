// GPUI macros expand through the canonical `gpui` crate path.
extern crate gpui_kit as gpui;

mod app;
mod config;
mod desktop;
mod markdown;
mod settings;
mod store;

use app::Brouyon;
use config::Config;
use desktop::Desktop;
use gpui_kit::{App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions, px, size};
use std::io;
use store::Store;

/// Opens the notes folder. A chosen folder can go away, such as on an unplugged
/// drive, and the app still has to start so it can be changed: the default folder
/// stands in for this session, the setting stays, and the message says so.
fn open_store(config: &mut Config) -> io::Result<(Store, Option<String>)> {
    match Store::open(&config.notes_dir) {
        Ok(store) => Ok((store, None)),
        Err(error) if config.notes_dir == config.default_notes_dir => Err(error),
        Err(error) => {
            let message = format!(
                "Can't use {}: {error}. Using the default folder for now.",
                config.notes_dir.display()
            );
            config.notes_dir = config.default_notes_dir.clone();
            Store::open(&config.notes_dir).map(|store| (store, Some(message)))
        }
    }
}

/// Everything wrong at launch, as one message for the window to show.
fn startup_problem(problems: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let problems: Vec<_> = problems.into_iter().flatten().collect();
    (!problems.is_empty()).then(|| problems.join(" "))
}

fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        app_id: Some("brouyon".into()),
        // Hyprland draws the border and handles tiling, so there is no
        // title bar of our own.
        titlebar: None,
        window_decorations: Some(WindowDecorations::Server),
        // Set from the start, so the first frame isn't a solid one.
        window_background: cx.global::<Desktop>().transparency.surface(),
        window_min_size: Some(size(px(480.), px(320.))),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1060.), px(760.)),
            cx,
        ))),
        ..Default::default()
    }
}

fn main() {
    let (mut config, config_problem) = Config::load();
    let (store, folder_problem) = match open_store(&mut config) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("brouyon: can't use {}: {error}", config.notes_dir.display());
            std::process::exit(1);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            app::init(cx);
            desktop::follow(cx);
            app::apply_theme(config.theme, cx);
            let options = window_options(cx);
            cx.open_window(options, |window, cx| {
                window.set_window_title("Brouyon");
                cx.new(|cx| {
                    let mut brouyon = Brouyon::new(store, config, window, cx);
                    brouyon.settings_problem =
                        startup_problem([config_problem, folder_problem]).map(Into::into);
                    brouyon
                })
            })
            .expect("open window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop::Transparency;
    use gpui_kit::{TestAppContext, WindowBackgroundAppearance};
    use std::fs;

    #[gpui_kit::test]
    fn the_window_opens_with_the_surface_of_the_mode(cx: &mut TestAppContext) {
        cx.update(|cx| {
            desktop::init(cx);
            assert_eq!(
                window_options(cx).window_background,
                WindowBackgroundAppearance::Opaque
            );
            cx.global_mut::<Desktop>().transparency = Transparency::Glassy;
            assert_eq!(
                window_options(cx).window_background,
                WindowBackgroundAppearance::Transparent
            );
        });
    }

    #[test]
    fn every_startup_problem_is_reported() {
        assert_eq!(startup_problem([None, None]), None);
        assert_eq!(
            startup_problem([Some("Settings.".into()), None]),
            Some("Settings.".into())
        );
        assert_eq!(
            startup_problem([Some("Settings.".into()), Some("Folder.".into())]),
            Some("Settings. Folder.".into())
        );
    }

    #[test]
    fn a_missing_folder_falls_back_to_the_default_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(&dir.path().join("default"));
        // A file where the chosen folder should be makes it impossible to open.
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        config.notes_dir = blocked.join("notes");

        let (_store, problem) = open_store(&mut config).unwrap();
        assert_eq!(config.notes_dir, dir.path().join("default"));
        assert!(problem.unwrap().contains("Using the default folder"));
        assert!(config.saved_text().is_empty(), "the setting is left alone");
    }

    #[test]
    fn a_usable_folder_has_nothing_to_report() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(dir.path());
        let (_store, problem) = open_store(&mut config).unwrap();
        assert_eq!(problem, None);
        assert_eq!(config.notes_dir, dir.path());
    }

    #[test]
    fn an_unusable_default_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        let mut config = Config::for_test(&blocked.join("notes"));
        assert!(open_store(&mut config).is_err());
    }
}
