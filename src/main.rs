#![cfg_attr(
    test,
    allow(clippy::disallowed_methods, reason = "#[gpui_kit::test] quits")
)]

mod app;
mod assets;
mod checked;
mod config;
mod desktop;
mod kept;
mod lists;
mod look;
mod macos;
mod markdown;
mod settings;
mod shortcuts;
mod store;
mod title_bar;
mod watch;

use app::Brouyon;
use config::Config;
use gpui_kit::{App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions, px, size};
use look::MAC;
use std::io;
use store::Store;

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

fn startup_problem(problems: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let problems: Vec<_> = problems.into_iter().flatten().collect();
    (!problems.is_empty()).then(|| problems.join(" "))
}

fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        app_id: Some("brouyon".into()),
        titlebar: title_bar::options(),
        app_owns_titlebar_drag: title_bar::DRAWN,
        window_decorations: Some(WindowDecorations::Server),
        window_background: desktop::surface(cx),
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
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            app::init(cx);
            if MAC {
                macos::init(cx);
            } else {
                desktop::follow(cx);
            }
            app::apply_theme(config.theme, cx);
            let options = window_options(cx);
            cx.open_window(options, |window, cx| {
                window.set_window_title("Brouyon");
                let brouyon = cx.new(|cx| {
                    let mut brouyon = Brouyon::new(store, config, window, cx);
                    brouyon.watch_notes(window, cx);
                    brouyon.settings_problem =
                        startup_problem([config_problem, folder_problem]).map(Into::into);
                    brouyon
                });
                if MAC {
                    macos::follow_appearance(&brouyon, window, cx);
                }
                brouyon
            })
            .expect("open window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use checked::NotesDir;
    use desktop::{Desktop, Transparency};
    use gpui_kit::{TestAppContext, WindowBackgroundAppearance};
    use std::fs;

    #[gpui_kit::test]
    fn the_window_opens_with_the_surface_of_the_mode(cx: &mut TestAppContext) {
        use WindowBackgroundAppearance::{Blurred, Opaque, Transparent};
        cx.update(|cx| {
            desktop::init(cx);
            assert_eq!(
                window_options(cx).window_background,
                if MAC { Blurred } else { Opaque }
            );
            cx.global_mut::<Desktop>().transparency = Transparency::Glassy;
            assert_eq!(
                window_options(cx).window_background,
                if MAC { Blurred } else { Transparent }
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
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        config.notes_dir = NotesDir::new(blocked.join("notes"));

        let (_store, problem) = open_store(&mut config).unwrap();
        assert_eq!(&*config.notes_dir, dir.path().join("default"));
        assert!(problem.unwrap().contains("Using the default folder"));
        assert!(config.saved_text().is_empty(), "the setting is left alone");
    }

    #[test]
    fn a_usable_folder_has_nothing_to_report() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(dir.path());
        let (_store, problem) = open_store(&mut config).unwrap();
        assert_eq!(problem, None);
        assert_eq!(&*config.notes_dir, dir.path());
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
