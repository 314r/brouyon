//! The floating Settings window. The settings themselves belong to the main view:
//! this window shows them and asks for changes.

use crate::{
    app::{Brouyon, Quit},
    config::{
        Config, DEFAULT_TEXT_PERCENT, MAX_TEXT_PERCENT, MIN_TEXT_PERCENT, TEXT_PERCENT_STEP,
        ThemeMode,
    },
};
use gpui_kit::{
    App, AppContext, Bounds, Context, Div, Entity, FocusHandle, FontWeight, IntoElement,
    KeyBinding, PathPromptOptions, Point, Render, ScrollHandle, Styled, Subscription, Window,
    WindowBounds, WindowDecorations, WindowHandle, WindowKind, WindowOptions,
    base::{ScrollbarAxis, actions::Cancel},
    div,
    prelude::*,
    px, rems, size,
};
use gpui_omarchy::{
    ActiveTheme, Button, ButtonVariant, ChoiceItem, ChoiceState, IconName, button, focus_scope,
    icon, scrollbar, select,
};
use std::time::Duration;

#[cfg(test)]
use gpui_kit::Focusable;

const CONTEXT: &str = "BrouyonSettings";
/// The delays on offer, in minutes. 0 means never.
const IDLE_MINUTES: [u64; 8] = [0, 1, 2, 5, 10, 15, 30, 60];

gpui_kit::actions!(brouyon, [CloseSettings]);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, context),
        KeyBinding::new("ctrl-w", CloseSettings, context),
        KeyBinding::new("ctrl-q", Quit, context),
    ]);
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Appearance,
    Notes,
    About,
}

impl Section {
    const ALL: [Self; 3] = [Self::Appearance, Self::Notes, Self::About];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Notes => "Notes",
            Self::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::Notes => IconName::FileText,
            Self::About => IconName::Info,
        }
    }
}

fn window_options(config: &Config, cx: &App) -> WindowOptions {
    WindowOptions {
        // Launching Brouyon focuses the first window of its app id, which has to
        // be the main one, so this window has an id of its own.
        app_id: Some("brouyon-settings".into()),
        // On Wayland this gives the window the focused one as its parent, and
        // Hyprland floats a window that has a parent.
        kind: WindowKind::Floating,
        titlebar: None,
        window_decorations: Some(WindowDecorations::Server),
        window_min_size: Some(size(px(600.), px(400.))),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            // Room for the sidebar and the widest button at any text size.
            size(px((30. * config.rem_px()).max(760.)), px(560.)),
            cx,
        ))),
        ..Default::default()
    }
}

/// Every delay on offer, shortest first, including one set by hand in the file.
fn idle_options(current: Option<Duration>) -> Vec<Option<Duration>> {
    let mut options: Vec<_> = IDLE_MINUTES
        .iter()
        .map(|&minutes| (minutes > 0).then(|| Duration::from_secs(minutes * 60)))
        .collect();
    if !options.contains(&current) {
        options.push(current);
        options.sort();
    }
    options
}

fn idle_label(idle: Option<Duration>) -> String {
    match idle.map(|idle| idle.as_secs()) {
        None => "Never".into(),
        Some(60) => "After 1 minute".into(),
        Some(3600) => "After 1 hour".into(),
        Some(seconds) if seconds % 60 == 0 => format!("After {} minutes", seconds / 60),
        Some(seconds) => format!("After {seconds} seconds"),
    }
}

pub struct SettingsWindow {
    app: Entity<Brouyon>,
    app_window: WindowHandle<Brouyon>,
    section: Section,
    idle_options: Vec<Option<Duration>>,
    idle_choice: Entity<ChoiceState>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    pub fn open(
        app: Entity<Brouyon>,
        app_window: WindowHandle<Brouyon>,
        cx: &mut App,
    ) -> gpui_kit::Result<WindowHandle<Self>> {
        let options = window_options(app.read(cx).config(), cx);
        cx.open_window(options, |window, cx| {
            window.set_window_title("Settings");
            cx.new(|cx| Self::new(app, app_window, window, cx))
        })
    }

    pub fn new(
        app: Entity<Brouyon>,
        app_window: WindowHandle<Brouyon>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let idle = app.read(cx).config().idle;
        let options = idle_options(idle);
        let items = options
            .iter()
            .enumerate()
            .map(|(index, &option)| ChoiceItem::new(index.to_string(), idle_label(option)))
            .collect();
        let current = options.iter().position(|&option| option == idle);
        let idle_choice = cx.new(|cx| {
            ChoiceState::new(items, window, cx)
                .label("Blank note")
                .default_selected(current.unwrap_or_default())
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            _subscriptions: vec![
                cx.observe(&app, |_, _, cx| cx.notify()),
                cx.observe(&idle_choice, Self::on_idle_choice),
            ],
            app,
            app_window,
            section: Section::Appearance,
            idle_options: options,
            idle_choice,
            focus,
            scroll: ScrollHandle::new(),
        }
    }

    fn on_idle_choice(&mut self, choice: Entity<ChoiceState>, cx: &mut Context<Self>) {
        let selected = choice
            .read(cx)
            .selected()
            .and_then(|item| item.value.parse::<usize>().ok());
        if let Some(index) = selected {
            let idle = self.idle_options[index];
            self.app_window
                .update(cx, |app, window, cx| app.set_idle(idle, window, cx))
                .ok();
        }
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        let app_window = self.app_window;
        cx.spawn(async move |_, cx| {
            let answer = answer.await;
            app_window
                .update(cx, |app, window, cx| match answer {
                    Ok(Ok(Some(paths))) => {
                        if let Some(dir) = paths.into_iter().next() {
                            app.set_notes_dir(Some(dir), window, cx);
                        }
                    }
                    Ok(Err(error)) => {
                        app.settings_problem =
                            Some(format!("Couldn't choose a folder: {error}").into());
                        cx.notify();
                    }
                    // Cancelled.
                    _ => {}
                })
                .ok();
        })
        .detach();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.omarchy();
        let (divider, fill) = (t.divider(), t.normal_fill());
        let rows = Section::ALL.map(|section| {
            button(
                ("settings-section", section as u64),
                "",
                ButtonVariant::Secondary,
                cx,
            )
            .accessibility_label(section.title())
            .debug_selector(move || format!("section-{}", section.title()))
            .selected(section == self.section)
            .w_full()
            .justify_start()
            .child(icon(section.icon()))
            .child(section.title())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = section;
                this.scroll.set_offset(Point::default());
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(rems(13.))
            .h_full()
            .bg(fill)
            .border_r_1()
            .border_color(divider)
            .child(
                heading("Settings", cx)
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h(rems(2.75))
                    .px(rems(0.75)),
            )
            .child(div().flex().flex_col().px(rems(0.125)).children(rows))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let divider = cx.omarchy().divider();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(rems(2.75))
            .pl(rems(1.25))
            .pr(rems(0.125))
            .border_b_1()
            .border_color(divider)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(self.section.title()),
            )
            .child(
                button("close-settings", "", ButtonVariant::Secondary, cx)
                    .accessibility_label("Close settings")
                    .debug_selector(|| "close-settings".into())
                    .child(icon(IconName::Close))
                    .on_click(|_, window, _| window.remove_window()),
            )
    }

    fn appearance_page(&self, config: &Config, cx: &mut Context<Self>) -> Div {
        let themes = [
            (ThemeMode::Omarchy, "Omarchy"),
            (ThemeMode::Light, "Light"),
            (ThemeMode::Dark, "Dark"),
        ]
        .map(|(mode, label)| {
            button(label, label, ButtonVariant::Outline, cx)
                .debug_selector(move || format!("theme-{label}"))
                .selected(config.theme == mode)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app.update(cx, |app, cx| app.set_theme(mode, cx));
                }))
        });
        let percent = config.text_percent;
        let text_size_button =
            |id: &'static str, label: String, to: u32, cx: &mut Context<Self>| {
                button(id, label, ButtonVariant::Outline, cx)
                    .debug_selector(move || id.into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.set_text_percent(to, cx));
                    }))
            };
        let smaller = text_size_button(
            "text-smaller",
            "−".into(),
            percent.saturating_sub(TEXT_PERCENT_STEP),
            cx,
        )
        .accessibility_label("Decrease text size")
        .disabled(percent <= MIN_TEXT_PERCENT);
        let reset = text_size_button(
            "text-reset",
            format!("{percent} %"),
            DEFAULT_TEXT_PERCENT,
            cx,
        )
        .accessibility_label("Reset text size to the desktop's");
        let larger = text_size_button("text-larger", "+".into(), percent + TEXT_PERCENT_STEP, cx)
            .accessibility_label("Increase text size")
            .disabled(percent >= MAX_TEXT_PERCENT);

        page()
            .child(
                section("Theme", cx)
                    .child(row().children(themes))
                    .child(description(
                        "Omarchy follows the current desktop theme and its changes.",
                        cx,
                    )),
            )
            .child(
                section("Text size", cx)
                    .child(row().child(smaller).child(reset).child(larger))
                    .child(description(
                        "Scales the desktop's text size throughout Brouyon. In the main window, \
                         Ctrl+Plus, Ctrl+Minus and Ctrl+0 do the same.",
                        cx,
                    )),
            )
    }

    fn notes_page(&self, config: &Config, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let folder = config.notes_dir.clone();
        page()
            .child(
                section("Folder", cx)
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(config.notes_dir.display().to_string()),
                    )
                    .child(
                        row()
                            .child(
                                labelled_button(
                                    "choose-folder",
                                    "Choose folder…",
                                    IconName::Folder,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
                            )
                            .child(
                                button(
                                    "default-folder",
                                    "Use the default folder",
                                    ButtonVariant::Outline,
                                    cx,
                                )
                                .debug_selector(|| "default-folder".into())
                                .disabled(config.notes_dir == config.default_notes_dir)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.app_window
                                            .update(cx, |app, window, cx| {
                                                app.set_notes_dir(None, window, cx);
                                            })
                                            .ok();
                                    },
                                )),
                            )
                            .child(
                                labelled_button(
                                    "open-folder",
                                    "Open folder",
                                    IconName::ExternalLink,
                                    cx,
                                )
                                .on_click(move |_, _, cx| cx.open_with_system(&folder)),
                            ),
                    )
                    .child(description(
                        "Each note is a Markdown file in this folder, named after the moment it \
                         was begun, and the trash is its .trash folder. Notes already written \
                         stay where they are when you choose another folder.",
                        cx,
                    )),
            )
            .child(
                section("Blank note", cx)
                    .child(div().w(rems(15.)).child(select(
                        "idle-choice",
                        &self.idle_choice,
                        window,
                        cx,
                    )))
                    .child(description(
                        "When nothing is typed or clicked for this long, Brouyon saves the note \
                         and puts a blank one in its place, as when it opens.",
                        cx,
                    )),
            )
    }

    fn about_page(&self, cx: &mut Context<Self>) -> Div {
        page()
            .child(
                section("Brouyon", cx)
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(concat!("Version ", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(description(
                        "Quick notes in Markdown, always on a blank page.",
                        cx,
                    )),
            )
            .child(section("Thanks to", cx).child(description(
                "GPUI, gpui-kit and gpui-omarchy for the interface, Omarchy for the look, \
                 pulldown-cmark for the Markdown, and Lucide for the icons.",
                cx,
            )))
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let brouyon = self.app.read(cx);
        let config = brouyon.config().clone();
        let problem = brouyon.settings_problem.clone();
        window.set_rem_size(px(config.rem_px()));
        let t = cx.omarchy().clone();

        let content = match self.section {
            Section::Appearance => self.appearance_page(&config, cx),
            Section::Notes => self.notes_page(&config, window, cx),
            Section::About => self.about_page(cx),
        };
        let sidebar = self.sidebar(cx);
        let header = self.header(cx);
        let scrollbar = scrollbar(
            "settings-scrollbar",
            ScrollbarAxis::Vertical,
            &self.scroll,
            window,
            cx,
        );

        focus_scope("settings")
            // focus_scope's own context, which connects Tab, and this window's.
            .key_context(format!("OmarchyFocusScope {CONTEXT}").as_str())
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.mono_font.clone())
            .text_size(rems(0.75))
            // Esc closes an open list first. A closed one would swallow the key,
            // so this has to come before it does.
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| {
                if !this.idle_choice.read(cx).is_open() {
                    cx.stop_propagation();
                    window.remove_window();
                }
            }))
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(header)
                    .when_some(problem, |column, problem| {
                        column.child(
                            div()
                                .flex_shrink_0()
                                .px(rems(1.25))
                                .py(rems(0.5))
                                .border_b_1()
                                .border_color(t.divider())
                                .text_color(t.danger)
                                .child(problem),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("settings-page")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll)
                                    .child(content),
                            )
                            .child(scrollbar),
                    ),
            )
    }
}

fn page() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(1.5))
        .p(rems(1.25))
        // Keeps the last line clear of the scrollbar's track.
        .pr(rems(1.75))
}

fn section(title: &'static str, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(0.5))
        .child(heading(title, cx))
}

fn heading(title: &'static str, cx: &App) -> Div {
    div()
        .text_size(rems(0.6875))
        .font_weight(FontWeight::BOLD)
        .text_color(cx.omarchy().secondary)
        .child(title.to_uppercase())
}

fn description(text: &'static str, cx: &App) -> Div {
    div().text_color(cx.omarchy().secondary).child(text)
}

fn row() -> Div {
    div().flex().flex_wrap().gap(rems(0.375))
}

fn labelled_button(id: &'static str, label: &'static str, name: IconName, cx: &App) -> Button {
    button(id, "", ButtonVariant::Outline, cx)
        .accessibility_label(label)
        .debug_selector(move || id.into())
        .child(icon(name).size(rems(0.875)))
        .child(label)
}

/// Reaches states of this window for the screenshot review.
#[cfg(test)]
impl SettingsWindow {
    pub fn show_section_for_test(&mut self, index: usize, cx: &mut Context<Self>) {
        self.section = Section::ALL[index];
        cx.notify();
    }

    /// Toggles the list of delays.
    pub fn open_idle_list_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.idle_choice.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        window.dispatch_action(
            Box::new(gpui_kit::base::actions::Confirm { secondary: false }),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{draw, keys, open, open_with, type_and_save};
    use crate::config::Config;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
    use gpui_omarchy::Theme;
    use std::{fs, path::Path};

    /// Opens Settings with its shortcut, and a context to act in that window.
    fn open_settings(
        view: &Entity<Brouyon>,
        cx: &mut VisualTestContext,
    ) -> (WindowHandle<SettingsWindow>, VisualTestContext) {
        keys(cx, "ctrl-,");
        let settings = cx.update(|_, cx| view.read(cx).settings_window().unwrap());
        let mut settings_cx = VisualTestContext::from_window(settings.into(), cx);
        draw(&mut settings_cx);
        (settings, settings_cx)
    }

    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} isn't drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
        draw(cx);
    }

    /// Acts in a window expected to close, so there is nothing to redraw after.
    fn closing_click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    fn closing_keys(keystrokes: &str, cx: &mut VisualTestContext) {
        cx.simulate_keystrokes(keystrokes);
        cx.run_until_parked();
    }

    fn config(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> Config {
        cx.update(|_, cx| view.read(cx).config().clone())
    }

    fn is_open(settings: WindowHandle<SettingsWindow>, cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| settings.update(cx, |_, _, _| ()).is_ok())
    }

    fn focus_idle_choice(settings: WindowHandle<SettingsWindow>, cx: &mut VisualTestContext) {
        click("section-Notes", cx);
        settings
            .update(cx, |this, window, cx| {
                let focus = this.idle_choice.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            })
            .unwrap();
        draw(cx);
    }

    #[gpui_kit::test]
    fn it_is_a_floating_window_with_its_own_app_id(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let options = cx.update(|_, cx| window_options(view.read(cx).config(), cx));
        assert_eq!(options.kind, WindowKind::Floating);
        assert_eq!(options.app_id.as_deref(), Some("brouyon-settings"));
    }

    #[gpui_kit::test]
    fn the_shortcut_and_the_button_open_a_single_window(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (first, _settings_cx) = open_settings(&view, cx);
        keys(cx, "ctrl-,");
        click("settings", cx);
        let again = cx.update(|_, cx| view.read(cx).settings_window().unwrap());
        assert_eq!(first, again);
    }

    #[gpui_kit::test]
    fn it_opens_again_after_being_closed(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (first, mut settings_cx) = open_settings(&view, cx);
        closing_click("close-settings", &mut settings_cx);
        assert!(!is_open(first, cx));

        let (second, _settings_cx) = open_settings(&view, cx);
        assert_ne!(first, second);
        assert!(is_open(second, cx));
    }

    #[gpui_kit::test]
    fn closing_the_main_window_closes_settings_too(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (settings, _settings_cx) = open_settings(&view, cx);
        assert!(cx.simulate_close());
        cx.run_until_parked();
        assert!(!is_open(settings, cx));
    }

    #[gpui_kit::test]
    fn escape_and_ctrl_w_close_the_window(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (settings, mut settings_cx) = open_settings(&view, cx);
        closing_keys("escape", &mut settings_cx);
        assert!(!is_open(settings, cx));

        let (settings, mut settings_cx) = open_settings(&view, cx);
        closing_keys("ctrl-w", &mut settings_cx);
        assert!(!is_open(settings, cx));
    }

    #[gpui_kit::test]
    fn escape_closes_an_open_list_before_the_window(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (settings, mut settings_cx) = open_settings(&view, cx);
        focus_idle_choice(settings, &mut settings_cx);
        keys(&mut settings_cx, "enter");
        let list_is_open = |cx: &mut VisualTestContext| {
            settings
                .update(cx, |this, _, cx| this.idle_choice.read(cx).is_open())
                .unwrap()
        };
        assert!(list_is_open(&mut settings_cx));

        keys(&mut settings_cx, "escape");
        assert!(is_open(settings, cx), "only the list closed");
        assert!(!list_is_open(&mut settings_cx));

        closing_keys("escape", &mut settings_cx);
        assert!(!is_open(settings, cx), "then the window");
    }

    #[gpui_kit::test]
    fn the_tab_key_walks_the_controls_and_escape_still_closes(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (settings, mut settings_cx) = open_settings(&view, cx);
        let focused = |cx: &mut VisualTestContext| cx.update(|window, cx| window.focused(cx));

        let before = focused(&mut settings_cx);
        keys(&mut settings_cx, "tab");
        let first = focused(&mut settings_cx);
        assert_ne!(before, first, "focus moved to a control");
        keys(&mut settings_cx, "tab");
        assert_ne!(first, focused(&mut settings_cx), "and on to the next");

        closing_keys("escape", &mut settings_cx);
        assert!(!is_open(settings, cx));
    }

    #[gpui_kit::test]
    fn the_sections_each_show_their_own_page(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (_settings, mut settings_cx) = open_settings(&view, cx);
        assert!(settings_cx.debug_bounds("theme-Light").is_some());
        assert!(settings_cx.debug_bounds("open-folder").is_none());

        click("section-Notes", &mut settings_cx);
        assert!(settings_cx.debug_bounds("open-folder").is_some());
        assert!(settings_cx.debug_bounds("theme-Light").is_none());

        click("section-About", &mut settings_cx);
        assert!(settings_cx.debug_bounds("open-folder").is_none());
        assert!(settings_cx.debug_bounds("theme-Light").is_none());
    }

    #[gpui_kit::test]
    fn choosing_a_theme_applies_and_saves_it(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (_settings, mut settings_cx) = open_settings(&view, cx);

        click("theme-Light", &mut settings_cx);
        cx.update(|_, cx| assert_eq!(cx.omarchy().name, Theme::flexoki_light().name));
        assert!(config(&view, cx).saved_text().contains("theme = \"light\""));

        click("theme-Dark", &mut settings_cx);
        cx.update(|_, cx| assert_eq!(cx.omarchy().name, Theme::tokyo_night().name));
        assert!(config(&view, cx).saved_text().contains("theme = \"dark\""));

        click("theme-Omarchy", &mut settings_cx);
        cx.update(|_, cx| assert_eq!(cx.omarchy().name, Theme::system_or_default().name));
        assert!(
            config(&view, cx)
                .saved_text()
                .contains("theme = \"omarchy\"")
        );
    }

    #[gpui_kit::test]
    fn every_theme_keeps_the_mono_font_on_widgets(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Omarchy] {
            view.update(cx, |this, cx| this.set_theme(mode, cx));
            cx.run_until_parked();
            cx.update(|_, cx| assert_eq!(cx.omarchy().font, cx.omarchy().mono_font));
        }
    }

    #[gpui_kit::test]
    fn text_size_scales_both_windows_within_its_limits(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let (settings, mut settings_cx) = open_settings(&view, cx);
        let base = cx.update(|window, _| window.rem_size());

        click("text-larger", &mut settings_cx);
        draw(cx);
        assert_eq!(config(&view, cx).text_percent, 110);
        cx.update(|window, _| assert_eq!(window.rem_size(), base * 1.1));
        settings
            .update(&mut settings_cx, |_, window, _| {
                assert_eq!(window.rem_size(), base * 1.1);
            })
            .unwrap();
        assert!(
            config(&view, cx)
                .saved_text()
                .contains("text_percent = 110")
        );

        click("text-reset", &mut settings_cx);
        assert_eq!(config(&view, cx).text_percent, 100);

        for _ in 0..4 {
            click("text-smaller", &mut settings_cx);
        }
        assert_eq!(config(&view, cx).text_percent, 80, "stops at the minimum");
    }

    #[gpui_kit::test]
    fn the_keyboard_changes_text_size_while_typing(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("kept");
        keys(cx, "ctrl-=");
        assert_eq!(config(&view, cx).text_percent, 110);
        keys(cx, "ctrl-+");
        assert_eq!(config(&view, cx).text_percent, 120);
        keys(cx, "ctrl--");
        assert_eq!(config(&view, cx).text_percent, 110);
        keys(cx, "ctrl-0");
        assert_eq!(config(&view, cx).text_percent, 100);
        cx.update(|_, cx| assert_eq!(view.read(cx).editor_text(cx), "kept"));
    }

    #[gpui_kit::test]
    fn text_size_keys_stay_within_the_limits(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        for _ in 0..14 {
            keys(cx, "ctrl-=");
        }
        assert_eq!(config(&view, cx).text_percent, MAX_TEXT_PERCENT);
        for _ in 0..14 {
            keys(cx, "ctrl--");
        }
        assert_eq!(config(&view, cx).text_percent, MIN_TEXT_PERCENT);
    }

    #[gpui_kit::test]
    fn choosing_a_delay_saves_it_and_it_applies_at_once(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        type_and_save(cx, "a note");
        let (settings, mut settings_cx) = open_settings(&view, cx);
        focus_idle_choice(settings, &mut settings_cx);

        // The list opens on the current delay, 5 minutes: two up is 1 minute.
        keys(&mut settings_cx, "enter up up enter");
        assert_eq!(config(&view, cx).idle, Some(Duration::from_secs(60)));
        assert!(config(&view, cx).saved_text().contains("idle_minutes = 1"));

        crate::app::tests::wait(cx, Duration::from_secs(60));
        cx.update(|_, cx| assert!(view.read(cx).is_blank()));
    }

    #[gpui_kit::test]
    fn never_stops_the_idle_move_already_counting(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        type_and_save(cx, "stays");
        view.update_in(cx, |this, window, cx| this.set_idle(None, window, cx));
        crate::app::tests::wait(cx, Duration::from_secs(3600));
        cx.update(|_, cx| assert!(!view.read(cx).is_blank()));
    }

    #[test]
    fn a_delay_set_by_hand_is_offered_too() {
        let options = idle_options(Some(Duration::from_secs(6)));
        assert_eq!(options.len(), IDLE_MINUTES.len() + 1);
        assert_eq!(options[0], None);
        assert_eq!(options[1], Some(Duration::from_secs(6)));
        assert_eq!(idle_label(options[1]), "After 6 seconds");
        assert_eq!(
            idle_options(Some(Duration::from_secs(300))).len(),
            IDLE_MINUTES.len(),
            "an offered delay isn't listed twice"
        );
    }

    #[test]
    fn delays_read_naturally() {
        assert_eq!(idle_label(None), "Never");
        assert_eq!(idle_label(Some(Duration::from_secs(60))), "After 1 minute");
        assert_eq!(
            idle_label(Some(Duration::from_secs(120))),
            "After 2 minutes"
        );
        assert_eq!(idle_label(Some(Duration::from_secs(3600))), "After 1 hour");
    }

    fn texts(dir: &Path) -> Vec<String> {
        crate::app::tests::note_texts(dir)
    }

    #[gpui_kit::test]
    fn changing_folder_saves_the_open_note_where_it_began(cx: &mut TestAppContext) {
        let (old, view, cx) = open(cx);
        let new = tempfile::tempdir().unwrap();
        fs::write(new.path().join("there.md"), "already there").unwrap();

        cx.simulate_input("typed just now");
        view.update_in(cx, |this, window, cx| {
            this.set_notes_dir(Some(new.path().to_owned()), window, cx);
        });
        cx.run_until_parked();

        assert_eq!(texts(old.path()), ["typed just now"]);
        assert_eq!(texts(new.path()), ["already there"]);
        cx.update(|_, cx| {
            let this = view.read(cx);
            assert!(this.is_blank());
            assert_eq!(this.note_titles(), ["already there"]);
        });
        assert!(
            config(&view, cx)
                .saved_text()
                .contains(new.path().to_str().unwrap())
        );

        // What is typed next goes to the new folder.
        draw(cx);
        type_and_save(cx, "written here");
        assert_eq!(texts(new.path()), ["already there", "written here"]);
        assert_eq!(texts(old.path()), ["typed just now"]);
    }

    #[gpui_kit::test]
    fn the_default_folder_button_goes_back_and_clears_the_setting(cx: &mut TestAppContext) {
        let (default, view, cx) = open(cx);
        let other = tempfile::tempdir().unwrap();
        view.update_in(cx, |this, window, cx| {
            this.set_notes_dir(Some(other.path().to_owned()), window, cx);
        });
        let (_settings, mut settings_cx) = open_settings(&view, cx);
        click("section-Notes", &mut settings_cx);
        click("default-folder", &mut settings_cx);

        assert_eq!(config(&view, cx).notes_dir, default.path());
        assert!(!config(&view, cx).saved_text().contains("notes_dir"));
    }

    #[gpui_kit::test]
    fn a_folder_that_cannot_be_used_changes_nothing_and_says_so(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "safe");
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "").unwrap();

        view.update_in(cx, |this, window, cx| {
            this.set_notes_dir(Some(blocked.join("notes")), window, cx);
        });
        cx.run_until_parked();

        assert_eq!(config(&view, cx).notes_dir, dir.path());
        assert!(!config(&view, cx).saved_text().contains("notes_dir"));
        cx.update(|_, cx| {
            let this = view.read(cx);
            assert!(!this.is_blank(), "stays on the open note");
            assert!(this.settings_problem.is_some());
        });
    }

    #[gpui_kit::test]
    fn a_pending_question_cannot_act_on_the_new_folder(cx: &mut TestAppContext) {
        let (old, view, cx) = open(cx);
        // The same file name in both trashes, which a question can't tell apart.
        let new = tempfile::tempdir().unwrap();
        fs::create_dir_all(old.path().join(".trash")).unwrap();
        fs::create_dir_all(new.path().join(".trash")).unwrap();
        fs::write(old.path().join(".trash/same.md"), "old").unwrap();
        fs::write(new.path().join(".trash/same.md"), "new").unwrap();
        view.update_in(cx, |this, window, cx| {
            this.ask_to_delete_forever("same.md".into(), window, cx);
        });

        view.update_in(cx, |this, window, cx| {
            this.set_notes_dir(Some(new.path().to_owned()), window, cx);
        });
        cx.update(|_, cx| assert!(!view.read(cx).has_question()));
        assert!(new.path().join(".trash/same.md").exists());
        assert!(old.path().join(".trash/same.md").exists());
    }

    #[gpui_kit::test]
    fn a_folder_picked_in_the_dialog_is_used(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let picked = tempfile::tempdir().unwrap();
        let (_settings, mut settings_cx) = open_settings(&view, cx);
        click("section-Notes", &mut settings_cx);
        click("choose-folder", &mut settings_cx);

        assert!(cx.did_prompt_for_paths());
        let path = picked.path().to_owned();
        cx.simulate_path_prompt_response(move |options| {
            assert!(options.directories && !options.files && !options.multiple);
            Some(vec![path])
        });
        cx.run_until_parked();
        assert_eq!(config(&view, cx).notes_dir, picked.path());
    }

    #[gpui_kit::test]
    fn cancelling_the_dialog_changes_nothing(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let (_settings, mut settings_cx) = open_settings(&view, cx);
        click("section-Notes", &mut settings_cx);
        click("choose-folder", &mut settings_cx);
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        assert_eq!(config(&view, cx).notes_dir, dir.path());
        cx.update(|_, cx| assert!(view.read(cx).settings_problem.is_none()));
    }

    #[gpui_kit::test]
    fn a_settings_file_it_cannot_read_is_reported_and_left_alone(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let settings_file = dir.path().join(".settings/config.toml");
        fs::create_dir_all(settings_file.parent().unwrap()).unwrap();
        fs::write(&settings_file, "not toml =").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);

        view.update(cx, |this, cx| this.set_theme(ThemeMode::Dark, cx));
        cx.update(|_, cx| {
            let problem = view.read(cx).settings_problem.clone().unwrap();
            assert!(problem.contains("Couldn't save settings"), "{problem}");
        });
        assert_eq!(fs::read_to_string(&settings_file).unwrap(), "not toml =");
        cx.update(|_, cx| assert_eq!(cx.omarchy().name, Theme::tokyo_night().name));
    }
}
