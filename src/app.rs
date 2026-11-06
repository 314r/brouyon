//! The Brouyon window: which note is open, when it is saved, when the app moves
//! on to a blank note, and how all of that is drawn.

use crate::{
    config::{Config, DEFAULT_TEXT_PERCENT, TEXT_PERCENT_STEP, ThemeMode},
    desktop::{self, Desktop},
    markdown,
    settings::{self, SettingsWindow},
    store::{Note, NoteId, Store},
};
use chrono::Datelike;
use gpui_kit::base::{
    Tab, Tabs, VirtualListScrollHandle,
    input::{Editor, EditorState, InputEvent, Redo},
};
use gpui_kit::{
    App, Context, DispatchPhase, Entity, FontWeight, IntoElement, MouseButton, Render, ScrollEvent,
    ScrollStrategy, SharedString, Styled, Subscription, Task, Window, WindowHandle, canvas, div,
    prelude::*, px, relative, rems, size, transparent_black,
};
use gpui_omarchy::{
    ActiveTheme, ButtonVariant, IconName, Theme, alert_dialog, button, dialog, dialog_button,
    dialog_description, dialog_popup, dialog_title, icon, virtual_list, with_tooltip,
};
use std::{
    io,
    path::PathBuf,
    rc::Rc,
    time::{Duration, SystemTime},
};

/// Saving waits for this pause in typing, so a burst of keys is one write.
const SAVE_DELAY: Duration = Duration::from_millis(400);
const KEY_CONTEXT: &str = "Brouyon";
/// What the shortcuts overlay lists, in the order of the README's table.
const SHORTCUTS: [(&str, &str); 10] = [
    ("Ctrl+N", "New note"),
    ("Alt+↑ / Alt+↓", "Previous / next note"),
    ("Ctrl+Shift+Backspace", "Move to Trash"),
    ("Ctrl+Shift+T", "Switch between Notes and Trash"),
    ("Esc", "Leave the Trash, or close Settings"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
    ("Ctrl+,", "Settings"),
    ("Ctrl+Plus, Ctrl+Minus, Ctrl+0", "Text size"),
    ("F1", "These shortcuts"),
    ("Ctrl+Q", "Quit"),
];

gpui_kit::actions!(
    brouyon,
    [
        NewNote,
        PreviousNote,
        NextNote,
        MoveToTrash,
        ToggleTrash,
        LeaveTrash,
        OpenSettings,
        IncreaseTextSize,
        DecreaseTextSize,
        ResetTextSize,
        ToggleShortcuts,
        Quit
    ]
);

pub fn init(cx: &mut App) {
    use gpui_kit::KeyBinding;
    desktop::init(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // Scoped to the app's context: a binding with no context would outrank the
    // editor's and the dialog's, so Escape would skip past an open question.
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewNote, context),
        KeyBinding::new("alt-up", PreviousNote, context),
        KeyBinding::new("alt-down", NextNote, context),
        KeyBinding::new("ctrl-shift-backspace", MoveToTrash, context),
        KeyBinding::new("ctrl-shift-t", ToggleTrash, context),
        KeyBinding::new("escape", LeaveTrash, context),
        KeyBinding::new("ctrl-,", OpenSettings, context),
        KeyBinding::new("ctrl-=", IncreaseTextSize, context),
        KeyBinding::new("ctrl-+", IncreaseTextSize, context),
        KeyBinding::new("ctrl--", DecreaseTextSize, context),
        KeyBinding::new("ctrl-0", ResetTextSize, context),
        // Typing `?` has to reach the note. F1 is here too because `/` takes Shift on
        // some layouts, such as AZERTY.
        KeyBinding::new("f1", ToggleShortcuts, context),
        KeyBinding::new("ctrl-/", ToggleShortcuts, context),
        KeyBinding::new("ctrl-q", Quit, context),
        // gpui-base only binds Ctrl+Y on Linux. `Input` is its editor's key context.
        KeyBinding::new("ctrl-shift-z", Redo, Some("Input")),
    ]);
    settings::bind_keys(cx);

    // Omarchy's shell uses one face throughout, while gpui-omarchy's widgets use
    // the theme's UI font. Every new theme, including one the desktop switches
    // to, gets the system monospace in its place.
    use_mono_font(cx);
    cx.observe_global::<Theme>(use_mono_font).detach();
    cx.observe_global::<Desktop>(use_mono_font).detach();
}

/// gpui-omarchy looks the monospace font up only once, so a font changed since then
/// comes from [`Desktop`]. A new theme arrives with the old one, and this runs before
/// it's drawn. Writing notifies observers of the theme, this among them, and the second
/// time through there's nothing left to change.
fn use_mono_font(cx: &mut App) {
    let theme = cx.omarchy();
    let wanted = cx
        .global::<Desktop>()
        .font
        .clone()
        .unwrap_or_else(|| theme.mono_font.clone());
    if theme.font != wanted || theme.mono_font != wanted {
        let theme = cx.global_mut::<Theme>();
        theme.font = wanted.clone();
        theme.mono_font = wanted;
        cx.refresh_windows();
    }
}

pub fn apply_theme(mode: ThemeMode, cx: &mut App) {
    match mode {
        ThemeMode::Omarchy => follow_desktop_theme(cx),
        ThemeMode::Light => Theme::flexoki_light().apply(cx),
        ThemeMode::Dark => Theme::tokyo_night().apply(cx),
    }
    // Not left to the observer: at startup it isn't active yet, and it never hears of
    // this theme, because GPUI drops a notification when one is already queued.
    use_mono_font(cx);
}

#[cfg(not(test))]
fn follow_desktop_theme(cx: &mut App) {
    Theme::follow_system(cx);
}

/// The same palette, without watching the real home folder. Any file changing there
/// would wake the test scheduler from another thread, which aborts the whole run.
#[cfg(test)]
fn follow_desktop_theme(cx: &mut App) {
    Theme::system_or_default().apply(cx);
}

/// What the editor pane is showing. The list being shown follows from it.
#[derive(Clone, Debug, PartialEq)]
enum Selection {
    /// A new note with no file yet. The first save creates one.
    Blank,
    Note(NoteId),
    Trashed(NoteId),
}

impl Selection {
    fn in_trash(&self) -> bool {
        matches!(self, Selection::Trashed(_))
    }
}

/// A destructive action waiting for an answer in the confirmation dialog.
#[derive(Clone)]
enum Confirm {
    DeleteForever(NoteId),
    EmptyTrash,
}

pub struct Brouyon {
    store: Store,
    config: Config,
    notes: Vec<Note>,
    trash: Vec<Note>,
    selection: Selection,
    /// The note the Notes tab goes back to from the Trash. `None` means a blank one.
    before_trash: Option<NoteId>,
    /// Rebuilt for each note, so undo history never crosses between notes.
    editor: Entity<EditorState>,
    _editor_events: Subscription,
    /// The editor holds text that isn't on disk yet.
    dirty: bool,
    /// The open file's modified time as last read or written, to notice changes
    /// made outside the app.
    loaded_modified: Option<SystemTime>,
    problem: Option<SharedString>,
    /// What went wrong with the last setting, shown until one succeeds.
    pub settings_problem: Option<SharedString>,
    settings_window: Option<WindowHandle<SettingsWindow>>,
    save_task: Option<Task<()>>,
    idle_task: Option<Task<()>>,
    confirm: Option<Confirm>,
    shortcuts_open: bool,
    /// Held by whichever dialog is open.
    dialog_focus: gpui_kit::FocusHandle,
    list_scroll: VirtualListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Brouyon {
    pub fn new(store: Store, config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = build_editor("", false, window, cx);
        let editor_events = cx.subscribe_in(&editor, window, Self::on_editor_event);
        let subscriptions = vec![
            cx.observe_window_activation(window, Self::on_activation),
            cx.on_app_quit(|this, cx| {
                this.flush(cx);
                async {}
            }),
            // Keys with a binding, like the arrows, never reach key listeners.
            cx.observe_keystrokes(|this, _, window, cx| this.restart_idle_timer(window, cx)),
        ];
        let view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            view.update(cx, |this, cx| this.on_close(cx))
                .unwrap_or(true)
        });

        let mut this = Self {
            store,
            config,
            notes: Vec::new(),
            trash: Vec::new(),
            selection: Selection::Blank,
            before_trash: None,
            editor,
            _editor_events: editor_events,
            dirty: false,
            loaded_modified: None,
            problem: None,
            settings_problem: None,
            settings_window: None,
            save_task: None,
            idle_task: None,
            confirm: None,
            shortcuts_open: false,
            dialog_focus: cx.focus_handle(),
            list_scroll: VirtualListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.reload_lists();
        this.focus_content(window, cx);
        this
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn focus_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| state.focus(window, cx));
    }

    fn replace_editor(
        &mut self,
        text: &str,
        readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor = build_editor(text, readonly, window, cx);
        self._editor_events = cx.subscribe_in(&self.editor, window, Self::on_editor_event);
        self.focus_content(window, cx);
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.dirty = true;
            self.restart_idle_timer(window, cx);
            self.save_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SAVE_DELAY).await;
                this.update(cx, |this, cx| {
                    this.flush(cx);
                })
                .ok();
            }));
        }
    }

    /// Writes any pending text. Returns false if it could not be saved, in which
    /// case the text stays in the editor and callers must not navigate away.
    fn flush(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_task = None;
        if !self.dirty {
            return true;
        }
        let text = self.editor.read(cx).value();
        let saved = match (&self.selection, text.trim().is_empty()) {
            (Selection::Note(id), true) => match self.store.remove(id) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(None),
            },
            (Selection::Note(id), false) => self.store.write(id, &text).map(|()| Some(id.clone())),
            (Selection::Blank, false) => self.store.create(&text).map(Some),
            _ => Ok(None),
        };
        match saved {
            Ok(saved) => {
                self.dirty = false;
                self.problem = None;
                self.record_save(saved, &text);
                cx.notify();
                true
            }
            Err(error) => {
                self.problem = Some(format!("Couldn't save: {error}").into());
                cx.notify();
                false
            }
        }
    }

    /// Moves a saved note to the top of the list, without rescanning the folder
    /// on every pause in typing. `None` means the note was emptied and is gone.
    fn record_save(&mut self, saved: Option<NoteId>, text: &str) {
        let previous_row = match &self.selection {
            Selection::Note(id) => {
                let row = self.notes.iter().position(|note| note.id == *id);
                self.notes.retain(|note| note.id != *id);
                row
            }
            _ => None,
        };
        let Some(id) = saved else {
            self.selection = Selection::Blank;
            self.loaded_modified = None;
            return;
        };
        let modified = self
            .store
            .modified(&id)
            .unwrap_or_else(|_| SystemTime::now());
        self.notes.insert(0, Note::new(id.clone(), text, modified));
        self.selection = Selection::Note(id);
        self.loaded_modified = Some(modified);
        if previous_row != Some(0) {
            self.reveal_selection();
        }
    }

    fn reload_lists(&mut self) {
        match self
            .store
            .list()
            .and_then(|notes| Ok((notes, self.store.list_trash()?)))
        {
            Ok((notes, trash)) => {
                self.notes = notes;
                self.trash = trash;
            }
            Err(error) => self.problem = Some(format!("Couldn't read notes: {error}").into()),
        }
    }

    /// Opens `selection`, saving the current note first. Stays put if either
    /// step fails, so no unsaved text is ever left behind.
    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flush(cx) {
            return;
        }
        // Reselecting the open note keeps its cursor and undo history. A blank
        // note is always rebuilt, which drops the history of an emptied note.
        if selection == self.selection && selection != Selection::Blank {
            self.focus_content(window, cx);
            return;
        }
        self.open(selection, window, cx);
    }

    /// Shows `selection` in a fresh editor. Assumes pending text is already saved.
    fn open(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        let (text, modified) = match &selection {
            Selection::Blank => (Ok(String::new()), None),
            Selection::Note(id) => (self.store.read(id), self.store.modified(id).ok()),
            Selection::Trashed(id) => (self.store.read_trashed(id), None),
        };
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                self.problem = Some(format!("Couldn't open note: {error}").into());
                self.reload_lists();
                cx.notify();
                return;
            }
        };
        self.problem = None;
        let readonly = selection.in_trash();
        if !readonly {
            self.before_trash = None;
        }
        self.selection = selection;
        self.loaded_modified = modified;
        self.replace_editor(&text, readonly, window, cx);
        self.reveal_selection();
        self.restart_idle_timer(window, cx);
        cx.notify();
    }

    fn restart_idle_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(delay) = self.config.idle else {
            return;
        };
        self.idle_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| this.on_idle(window, cx))
                .ok();
        }));
    }

    fn on_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A dialog on screen waits to be closed.
        if self.dialog_is_open() {
            self.restart_idle_timer(window, cx);
            return;
        }
        if self.selection == Selection::Blank && !self.dirty {
            return;
        }
        self.select(Selection::Blank, window, cx);
        // Still dirty means the save failed: stay on the note and try again later.
        if self.dirty {
            self.restart_idle_timer(window, cx);
        }
    }

    fn on_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            self.sync_with_disk(window, cx);
        } else {
            self.flush(cx);
        }
    }

    /// Picks up changes made outside the app, such as a note edited in another
    /// program. Pending edits win: they are written on the next save.
    fn sync_with_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_lists();
        match self.selection.clone() {
            Selection::Note(id) if !self.dirty => match self.store.modified(&id) {
                Ok(modified) if Some(modified) == self.loaded_modified => {}
                Ok(_) => self.open(Selection::Note(id), window, cx),
                Err(_) => self.open(Selection::Blank, window, cx),
            },
            Selection::Trashed(id) if !self.trash.iter().any(|note| note.id == id) => {
                self.show_next_trashed(window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The first close attempt after a failed save keeps the window open so the
    /// error can be seen. A second attempt closes anyway.
    fn on_close(&mut self, cx: &mut Context<Self>) -> bool {
        let already_warned = self.problem.is_some();
        let closing = self.flush(cx) || already_warned;
        if closing && let Some(settings) = self.settings_window {
            // The app runs until its last window closes, and Settings can't
            // outlive the window it belongs to.
            settings
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
        closing
    }

    fn row_count(&self) -> usize {
        if self.selection.in_trash() {
            self.trash.len()
        } else {
            self.notes.len() + usize::from(self.selection == Selection::Blank)
        }
    }

    fn row_selection(&self, row: usize) -> Selection {
        if self.selection.in_trash() {
            Selection::Trashed(self.trash[row].id.clone())
        } else if self.selection == Selection::Blank {
            match row {
                0 => Selection::Blank,
                row => Selection::Note(self.notes[row - 1].id.clone()),
            }
        } else {
            Selection::Note(self.notes[row].id.clone())
        }
    }

    fn selected_row(&self) -> Option<usize> {
        match &self.selection {
            Selection::Blank => Some(0),
            Selection::Note(id) => self.notes.iter().position(|note| note.id == *id),
            Selection::Trashed(id) => self.trash.iter().position(|note| note.id == *id),
        }
    }

    fn reveal_selection(&self) {
        if let Some(row) = self.selected_row() {
            self.list_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    /// Moves through the list. Going up from the first note reaches a blank one.
    fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flush(cx) {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        let target = if down {
            row + 1
        } else if row > 0 {
            row - 1
        } else {
            if !self.selection.in_trash() {
                self.select(Selection::Blank, window, cx);
            }
            return;
        };
        if target < self.row_count() {
            let selection = self.row_selection(target);
            self.select(selection, window, cx);
        }
    }

    fn move_to_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Text typed moments ago may not have a file yet.
        if !self.flush(cx) {
            return;
        }
        let Selection::Note(id) = self.selection.clone() else {
            return;
        };
        match self.store.trash(&id) {
            Ok(_) => {
                self.reload_lists();
                self.open(Selection::Blank, window, cx);
            }
            Err(error) => {
                self.problem = Some(format!("Couldn't move to Trash: {error}").into());
                cx.notify();
            }
        }
    }

    /// The Notes tab: back to the note shown before the Trash, if it's still there.
    fn show_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.in_trash() {
            self.focus_content(window, cx);
            return;
        }
        let before = self
            .before_trash
            .take()
            .filter(|id| self.notes.iter().any(|note| note.id == *id));
        self.select(before.map_or(Selection::Blank, Selection::Note), window, cx);
    }

    /// The Trash tab, which shows its first note. An empty trash has nothing to show.
    fn show_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.in_trash() || !self.flush(cx) {
            return;
        }
        self.reload_lists();
        let Some(first) = self.trash.first().map(|note| note.id.clone()) else {
            cx.notify();
            return;
        };
        self.before_trash = match &self.selection {
            Selection::Note(id) => Some(id.clone()),
            _ => None,
        };
        self.open(Selection::Trashed(first), window, cx);
    }

    fn toggle_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.in_trash() {
            self.show_notes(window, cx);
        } else {
            self.show_trash(window, cx);
        }
    }

    /// Once a note has left the trash: the first one still there, or the notes.
    fn show_next_trashed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.trash.first().map(|note| note.id.clone()) {
            Some(id) => self.open(Selection::Trashed(id), window, cx),
            None => self.show_notes(window, cx),
        }
    }

    fn restore(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        match self.store.restore(&id) {
            Ok(restored) => {
                self.reload_lists();
                self.open(Selection::Note(restored), window, cx);
            }
            Err(error) => {
                self.problem = Some(format!("Couldn't restore: {error}").into());
                cx.notify();
            }
        }
    }

    fn ask(&mut self, confirm: Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = Some(confirm);
        self.dialog_focus.focus(window, cx);
        cx.notify();
    }

    fn dialog_is_open(&self) -> bool {
        self.confirm.is_some() || self.shortcuts_open
    }

    fn toggle_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcuts_open = !self.shortcuts_open;
        if self.shortcuts_open {
            self.dialog_focus.focus(window, cx);
        } else {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    fn answer(&mut self, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        if confirmed {
            let result = match confirm {
                Confirm::DeleteForever(id) => self.store.purge(&id),
                Confirm::EmptyTrash => self.store.empty_trash(),
            };
            self.reload_lists();
            match result {
                Ok(()) => self.show_next_trashed(window, cx),
                Err(error) => {
                    self.problem = Some(format!("Couldn't delete: {error}").into());
                    self.focus_content(window, cx);
                }
            }
        } else {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings_window
            && settings
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let Some(app_window) = window.window_handle().downcast::<Self>() else {
            return;
        };
        let app = cx.entity();
        // Deferred until this view is free, because Settings reads it as it opens.
        cx.defer(move |cx| {
            let opened = SettingsWindow::open(app.clone(), app_window, cx);
            app.update(cx, |this, cx| {
                match opened {
                    Ok(settings) => this.settings_window = Some(settings),
                    Err(error) => {
                        this.settings_problem =
                            Some(format!("Couldn't open Settings: {error}").into());
                    }
                }
                cx.notify();
            });
        });
    }

    fn report_setting(&mut self, result: io::Result<()>, cx: &mut Context<Self>) {
        self.settings_problem = result
            .err()
            .map(|error| format!("Couldn't save settings: {error}").into());
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: ThemeMode, cx: &mut Context<Self>) {
        if theme != self.config.theme {
            let saved = self.config.set_theme(theme);
            apply_theme(theme, cx);
            self.report_setting(saved, cx);
        }
    }

    pub fn set_text_percent(&mut self, percent: u32, cx: &mut Context<Self>) {
        let saved = self.config.set_text_percent(percent);
        self.report_setting(saved, cx);
    }

    pub fn set_idle(
        &mut self,
        idle: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if idle != self.config.idle {
            let saved = self.config.set_idle(idle);
            self.idle_task = None;
            self.restart_idle_timer(window, cx);
            self.report_setting(saved, cx);
        }
    }

    /// Moves to another folder, or back to the default one with `None`. The open
    /// note is saved where it was begun, and existing notes don't move.
    pub fn set_notes_dir(
        &mut self,
        dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = dir
            .clone()
            .unwrap_or_else(|| self.config.default_notes_dir.clone());
        if target == self.config.notes_dir {
            return;
        }
        if !self.flush(cx) {
            self.settings_problem =
                Some("The open note couldn't be saved, so the folder didn't change.".into());
            cx.notify();
            return;
        }
        let store = match Store::open(&target) {
            Ok(store) => store,
            Err(error) => {
                self.settings_problem =
                    Some(format!("Couldn't use {}: {error}", target.display()).into());
                cx.notify();
                return;
            }
        };
        self.store = store;
        // A question about a note in the old folder must not act on the new one,
        // where a note can have the same name.
        self.confirm = None;
        let saved = self.config.set_notes_dir(dir);
        self.reload_lists();
        self.open(Selection::Blank, window, cx);
        self.report_setting(saved, cx);
    }

    fn sidebar(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let in_trash = self.selection.in_trash();
        // Wide enough for a title and its date on one line, unless the editor needs it.
        let width = if window.viewport_size().width < rems(45.).to_pixels(window.rem_size()) {
            11.25
        } else {
            20.
        };
        let tabs = Tabs::new("note-tabs")
            .flex()
            .h_full()
            .gap(rems(1.))
            .child(
                panel_tab("notes-tab", "Notes", self.notes.len(), !in_trash, true, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.show_notes(window, cx))),
            )
            .child(
                panel_tab(
                    "trash-tab",
                    "Trash",
                    self.trash.len(),
                    in_trash,
                    in_trash || !self.trash.is_empty(),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.show_trash(window, cx))),
            );
        let new_note = with_tooltip(
            button("new-note", "", ButtonVariant::Secondary, cx)
                .accessibility_label("New note")
                .debug_selector(|| "new-note".into())
                .child(icon(IconName::Plus))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.select(Selection::Blank, window, cx);
                })),
            "New note (Ctrl+N)",
        );
        let header = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(rems(2.75))
            .pl(rems(0.75))
            .pr(rems(0.125))
            .border_b_1()
            .border_color(t.divider())
            .child(tabs)
            .child(new_note);

        let row_height = rems(3.25).to_pixels(window.rem_size());
        let sizes = Rc::new(vec![
            size(gpui_kit::Pixels::ZERO, row_height);
            self.row_count()
        ]);
        let list = virtual_list(
            cx.entity(),
            "note-list",
            sizes,
            move |this, range, _, cx| {
                range
                    .map(|row| this.render_row(row, row_height, cx))
                    .collect::<Vec<_>>()
            },
            cx,
        )
        .track_scroll(&self.list_scroll)
        .size_full()
        // gpui-omarchy gives lists the theme's solid background, which would hide
        // the window's own where it's see-through.
        .bg(transparent_black());

        let footer = in_trash.then(|| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_end()
                .h(rems(2.75))
                .px(rems(0.5))
                .border_t_1()
                .border_color(t.divider())
                .child(
                    button("empty-trash", "Empty Trash", ButtonVariant::Outline, cx)
                        .debug_selector(|| "empty-trash".into())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.ask(Confirm::EmptyTrash, window, cx);
                        })),
                )
        });

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(rems(width))
            .h_full()
            .border_r_1()
            .border_color(t.divider())
            .child(header)
            .child(div().flex_1().min_h_0().child(list))
            .children(footer)
    }

    fn render_row(
        &self,
        row: usize,
        height: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let selection = self.row_selection(row);
        let selected = selection == self.selection;
        let note = match &selection {
            Selection::Note(id) => self.notes.iter().find(|note| note.id == *id),
            Selection::Trashed(id) => self.trash.iter().find(|note| note.id == *id),
            _ => None,
        };
        let (title, date, preview): (SharedString, SharedString, SharedString) = match note {
            Some(note) if note.title.is_empty() => (
                "Untitled".into(),
                format_date(note.modified, chrono::Local::now()).into(),
                "".into(),
            ),
            Some(note) => (
                note.title.clone().into(),
                format_date(note.modified, chrono::Local::now()).into(),
                note.preview.clone().into(),
            ),
            None => ("New note".into(), "".into(), "".into()),
        };
        div()
            .id(("note-row", row))
            .flex()
            .flex_col()
            .justify_center()
            .flex_shrink_0()
            .w_full()
            .h(height)
            .px(rems(0.75))
            .overflow_hidden()
            .when(selected, |row| row.bg(t.hover_fill()))
            .hover(|row| row.bg(t.hover_fill()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(selection.clone(), window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(rems(0.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if selected { t.accent } else { t.foreground })
                            .when(selected, |title| title.font_weight(FontWeight::BOLD))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.6875))
                            .text_color(t.secondary)
                            .child(date),
                    ),
            )
            // Empty, it keeps its line, so a title sits at the same height in every row.
            .child(
                div()
                    .truncate()
                    .text_size(rems(0.6875))
                    .text_color(t.secondary)
                    .child(preview),
            )
    }

    fn editor_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = self.editor.clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(rems(1.5))
                    .pr(rems(1.))
                    .pt(rems(1.25))
                    .text_size(rems(0.875))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        editor.update(cx, |state, cx| state.focus(window, cx));
                    })
                    .child(div().size_full().child(Editor::new(&self.editor))),
            )
            .child(self.status_strip(cx))
    }

    fn status_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let problem = self.problem.as_ref().or(self.settings_problem.as_ref());
        let status = match (problem, &self.selection, self.loaded_modified) {
            (Some(problem), _, _) => div().text_color(t.danger).child(problem.clone()),
            (None, Selection::Note(_), Some(modified)) => div().child(format!(
                "Saved · {}",
                format_date(modified, chrono::Local::now())
            )),
            (None, Selection::Trashed(_), _) => div().child("In the Trash"),
            _ => div(),
        };
        let actions = div().flex().flex_shrink_0().gap(rems(0.25));
        let actions = match self.selection.clone() {
            Selection::Note(_) => actions.child(
                button(
                    "move-to-trash",
                    "Move to Trash",
                    ButtonVariant::Secondary,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.move_to_trash(window, cx))),
            ),
            Selection::Trashed(id) => {
                let restore = id.clone();
                actions
                    .child(
                        button("restore", "Restore", ButtonVariant::Secondary, cx).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.restore(restore.clone(), window, cx);
                            }),
                        ),
                    )
                    .child(
                        button(
                            "delete-forever",
                            "Delete forever",
                            ButtonVariant::Danger,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.ask(Confirm::DeleteForever(id.clone()), window, cx);
                            },
                        )),
                    )
            }
            _ => actions,
        };
        let settings = with_tooltip(
            button("settings", "", ButtonVariant::Secondary, cx)
                .accessibility_label("Settings")
                .debug_selector(|| "settings".into())
                .child(icon(IconName::Settings))
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            "Settings (Ctrl+,)",
        );
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap(rems(0.75))
            .h(rems(2.75))
            .pl(rems(1.5))
            .pr(rems(0.125))
            .border_t_1()
            .border_color(t.divider())
            .text_size(rems(0.6875))
            .text_color(t.secondary)
            .child(div().truncate().child(status))
            .child(actions.child(settings))
    }

    fn confirm_dialog(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let confirm = self.confirm.as_ref()?;
        let (title, description, label) = match confirm {
            Confirm::DeleteForever(_) => (
                "Delete this note forever?",
                "This removes its file from the Trash and can't be undone.",
                "Delete forever",
            ),
            Confirm::EmptyTrash => (
                "Empty the Trash?",
                "This removes every file in the Trash and can't be undone.",
                "Empty Trash",
            ),
        };
        let close = cx.listener(|this, confirmed: &bool, window, cx| {
            this.answer(*confirmed, window, cx);
        });
        let popup = div()
            .id("confirm-surface")
            .occlude()
            .max_w(relative(0.9))
            .child(
                dialog_popup(cx)
                    .child(dialog_title(title, cx))
                    .child(dialog_description(description, cx))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(rems(0.5))
                            .child(
                                dialog_button("cancel", "Cancel", ButtonVariant::Secondary, cx)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(gpui_kit::base::actions::Cancel),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                dialog_button("confirm", label, ButtonVariant::Danger, cx)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(gpui_kit::base::actions::Confirm {
                                                secondary: false,
                                            }),
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            );
        Some(
            alert_dialog(&self.dialog_focus, cx)
                .popup(popup)
                .request_close(move |confirmed, window, cx| close(&confirmed, window, cx)),
        )
    }

    fn shortcuts_dialog(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.shortcuts_open {
            return None;
        }
        let t = cx.omarchy().clone();
        let close = cx.listener(|this, _: &bool, window, cx| this.toggle_shortcuts(window, cx));
        let rows = SHORTCUTS.map(|(keys, action)| {
            div()
                .flex()
                .gap(rems(1.))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(rems(15.))
                        .text_color(t.bright)
                        .child(keys),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(t.secondary)
                        .child(action),
                )
        });
        let popup = div()
            .id("shortcuts-surface")
            .occlude()
            .max_w(relative(0.9))
            .child(
                dialog_popup(cx)
                    .w(rems(36.))
                    .child(dialog_title("Keyboard shortcuts", cx))
                    .child(div().flex().flex_col().gap(rems(0.375)).children(rows)),
            );
        Some(
            dialog(&self.dialog_focus, cx)
                .popup(popup)
                .request_close(move |closed, window, cx| close(&closed, window, cx)),
        )
    }
}

impl Render for Brouyon {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(self.config.rem_px(cx.global::<Desktop>().text_scale)));
        let t = cx.omarchy().clone();
        let style = markdown::editor_style(&t);
        self.editor
            .update(cx, |state, _| state.set_editor_style(style));
        let sidebar = self.sidebar(window, cx);
        let editor_pane = self.editor_pane(cx);
        let dialog = self.confirm_dialog(cx);
        let shortcuts = self.shortcuts_dialog(cx);
        let view = cx.entity().downgrade();
        let answering = self.dialog_is_open();
        let questioning = self.confirm.is_some();
        div()
            .key_context(KEY_CONTEXT)
            .relative()
            .flex()
            .size_full()
            .bg(desktop::root_background(window, cx))
            .text_color(t.foreground)
            .font_family(t.mono_font.clone())
            .text_size(rems(0.75))
            .capture_key_down(cx.listener(|this, _, window, cx| {
                this.restart_idle_timer(window, cx);
            }))
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.restart_idle_timer(window, cx);
            }))
            // The key that opens the shortcuts closes them too, but a question comes first.
            .when(!questioning, |root| {
                root.on_action(cx.listener(|this, _: &ToggleShortcuts, window, cx| {
                    this.toggle_shortcuts(window, cx);
                }))
            })
            // An open dialog keeps the keyboard to itself.
            .when(!answering, |root| {
                root.on_action(cx.listener(|this, _: &NewNote, window, cx| {
                    this.select(Selection::Blank, window, cx);
                }))
                .on_action(cx.listener(|this, _: &PreviousNote, window, cx| {
                    this.step(false, window, cx);
                }))
                .on_action(cx.listener(|this, _: &NextNote, window, cx| {
                    this.step(true, window, cx);
                }))
                .on_action(cx.listener(|this, _: &MoveToTrash, window, cx| {
                    this.move_to_trash(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleTrash, window, cx| {
                    this.toggle_trash(window, cx);
                }))
                .on_action(cx.listener(|this, _: &LeaveTrash, window, cx| {
                    if this.selection.in_trash() {
                        this.show_notes(window, cx);
                    } else {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                    this.open_settings(window, cx);
                }))
                .on_action(cx.listener(|this, _: &IncreaseTextSize, _, cx| {
                    this.set_text_percent(this.config.text_percent + TEXT_PERCENT_STEP, cx);
                }))
                .on_action(cx.listener(|this, _: &DecreaseTextSize, _, cx| {
                    this.set_text_percent(this.config.text_percent - TEXT_PERCENT_STEP, cx);
                }))
                .on_action(cx.listener(|this, _: &ResetTextSize, _, cx| {
                    this.set_text_percent(DEFAULT_TEXT_PERCENT, cx);
                }))
            })
            .child(
                // Scrolling inside the editor or list stops propagating before
                // it would reach an ordinary listener on this root element.
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.on_mouse_event(move |_: &ScrollEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                view.update(cx, |this, cx| this.restart_idle_timer(window, cx))
                                    .ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_0(),
            )
            .child(sidebar)
            .child(editor_pane)
            .children(dialog)
            .children(shortcuts)
    }
}

/// A tab above the list: a small capital label with its count after it, and a
/// bar in the accent under the current one.
fn panel_tab(
    id: &'static str,
    label: &'static str,
    count: usize,
    selected: bool,
    enabled: bool,
    cx: &App,
) -> Tab {
    let t = cx.omarchy();
    let (hover, bright, accent) = (t.foreground, t.bright, t.accent);
    Tab::new(id)
        .selected(selected)
        .disabled(!enabled)
        .accessibility_label(format!("{label}, {count}"))
        .debug_selector(|| id.into())
        .h_full()
        .gap(rems(0.375))
        .border_b_2()
        .border_color(t.foreground.opacity(0.))
        .text_size(rems(0.6875))
        .font_weight(FontWeight::BOLD)
        .text_color(t.secondary)
        .when(enabled && !selected, |tab| {
            tab.hover(move |tab| tab.text_color(hover))
        })
        .styles(|styles| {
            styles
                .selected(|tab| tab.text_color(bright).border_color(accent))
                .disabled(|tab| tab.opacity(0.45))
        })
        .child(label.to_uppercase())
        .when(count > 0, |tab| {
            tab.child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.secondary)
                    .child(count.to_string()),
            )
        })
}

fn build_editor(
    text: &str,
    readonly: bool,
    window: &mut Window,
    cx: &mut Context<Brouyon>,
) -> Entity<EditorState> {
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(markdown::LANGUAGE)
            .soft_wrap(true)
            .line_number(false)
            .folding(false)
            .auto_close(false)
            .indent_guides(false)
            .searchable(false)
            .placeholder("Start typing…")
            .default_value(text.to_string());
        state.set_highlighter_factory(markdown::highlighter_factory(), cx);
        state.set_readonly(readonly, cx);
        state
    })
}

fn format_date(time: SystemTime, now: chrono::DateTime<chrono::Local>) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    let days_ago = (now.date_naive() - time.date_naive()).num_days();
    if days_ago == 0 {
        time.format("%H:%M").to_string()
    } else if days_ago == 1 {
        "Yesterday".to_string()
    } else if time.year() == now.year() {
        time.format("%-d %b").to_string()
    } else {
        time.format("%-d %b %Y").to_string()
    }
}

/// What tests in other modules need to see of the main view.
#[cfg(test)]
impl Brouyon {
    pub fn settings_window(&self) -> Option<WindowHandle<SettingsWindow>> {
        self.settings_window
    }

    pub fn editor_text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn is_blank(&self) -> bool {
        self.selection == Selection::Blank
    }

    pub fn note_titles(&self) -> Vec<String> {
        self.notes.iter().map(|note| note.title.clone()).collect()
    }

    pub fn ask_to_delete_forever(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask(Confirm::DeleteForever(id), window, cx);
    }

    pub fn has_question(&self) -> bool {
        self.confirm.is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::desktop::Transparency;
    use chrono::TimeZone;
    use gpui_kit::{
        Focusable, Modifiers, TestAppContext, VisualTestContext, base::actions, point, px,
    };
    use std::{
        fs::{self, File},
        path::Path,
    };

    const IDLE: Duration = Duration::from_secs(300);

    pub(crate) fn open(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Brouyon>, &mut VisualTestContext) {
        open_with(cx, tempfile::tempdir().unwrap())
    }

    pub(crate) fn open_with(
        cx: &mut TestAppContext,
        dir: tempfile::TempDir,
    ) -> (tempfile::TempDir, Entity<Brouyon>, &mut VisualTestContext) {
        // As `main` starts. This also ends `gpui_omarchy::init`'s watch on the real home.
        cx.update(|cx| {
            gpui_omarchy::init(cx);
            init(cx);
            apply_theme(ThemeMode::Omarchy, cx);
        });
        let store = Store::open(dir.path()).unwrap();
        let config = Config::for_test(dir.path());
        let (view, cx) =
            cx.add_window_view(move |window, cx| Brouyon::new(store, config, window, cx));
        draw(cx);
        (dir, view, cx)
    }

    pub(crate) fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Runs everything due within `duration`, then redraws so the current editor
    /// is registered as the window's text input.
    pub(crate) fn wait(cx: &mut VisualTestContext, duration: Duration) {
        cx.executor().advance_clock(duration);
        cx.run_until_parked();
        draw(cx);
    }

    pub(crate) fn type_and_save(cx: &mut VisualTestContext, text: &str) {
        cx.simulate_input(text);
        wait(cx, SAVE_DELAY);
    }

    pub(crate) fn keys(cx: &mut VisualTestContext, keystrokes: &str) {
        cx.simulate_keystrokes(keystrokes);
        cx.run_until_parked();
        draw(cx);
    }

    /// The notes on disk, ignoring the order they were created in.
    pub(crate) fn note_texts(dir: &Path) -> Vec<String> {
        texts_in(dir)
    }

    fn trashed_texts(dir: &Path) -> Vec<String> {
        texts_in(&dir.join(".trash"))
    }

    fn texts_in(dir: &Path) -> Vec<String> {
        let mut texts: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .map(|path| fs::read_to_string(path).unwrap())
            .collect();
        texts.sort();
        texts
    }

    fn selection(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> Selection {
        cx.update(|_, cx| view.read(cx).selection.clone())
    }

    fn editor_text(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| view.read(cx).editor.read(cx).value().to_string())
    }

    fn titles(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|_, cx| {
            view.read(cx)
                .notes
                .iter()
                .map(|note| note.title.clone())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn launching_opens_a_focused_blank_note_and_writes_nothing(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(editor_text(&view, cx), "");
        assert!(note_texts(dir.path()).is_empty());
        cx.update(|window, cx| {
            let focus = view.read(cx).editor.read(cx).focus_handle(cx);
            assert!(focus.is_focused(window));
        });
        // Nothing appears on disk just from time passing.
        wait(cx, IDLE * 2);
        assert!(note_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn existing_notes_are_listed_but_the_app_still_opens_blank(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "# Old note\nbody").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(titles(&view, cx), ["Old note"]);
    }

    #[gpui_kit::test]
    fn typing_saves_only_after_a_pause(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        cx.simulate_input("hel");
        wait(cx, SAVE_DELAY / 2);
        cx.simulate_input("lo");
        wait(cx, SAVE_DELAY / 2);
        assert!(note_texts(dir.path()).is_empty(), "still within the pause");

        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["hello"]);
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert_eq!(titles(&view, cx), ["hello"]);
        assert_eq!(
            editor_text(&view, cx),
            "hello",
            "keeps editing the same text"
        );

        type_and_save(cx, " world");
        assert_eq!(note_texts(dir.path()), ["hello world"], "no second file");
    }

    /// In the order `main` does it. A theme applied here replaces the one `init` has
    /// just corrected, and it isn't observed yet.
    #[gpui_kit::test]
    fn widgets_have_the_mono_font_from_the_start(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_omarchy::init(cx);
            init(cx);
            apply_theme(ThemeMode::Omarchy, cx);
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.omarchy().font, cx.omarchy().mono_font));
    }

    #[gpui_kit::test]
    fn the_preview_follows_the_note_as_it_is_typed(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        let first_preview =
            |cx: &mut VisualTestContext| cx.update(|_, cx| view.read(cx).notes[0].preview.clone());

        type_and_save(cx, "A title");
        assert_eq!(first_preview(cx), "");

        type_and_save(cx, "\nthe line under it");
        assert_eq!(first_preview(cx), "the line under it");
    }

    #[gpui_kit::test]
    fn emptying_a_note_removes_its_file_and_undo_brings_it_back(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "short lived");
        assert_eq!(note_texts(dir.path()).len(), 1);

        keys(cx, "ctrl-a backspace");
        wait(cx, SAVE_DELAY);
        assert!(note_texts(dir.path()).is_empty());
        assert!(titles(&view, cx).is_empty());
        assert_eq!(selection(&view, cx), Selection::Blank);

        keys(cx, "ctrl-z");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["short lived"]);
    }

    #[gpui_kit::test]
    fn whitespace_alone_never_creates_a_file(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        type_and_save(cx, "   \n  ");
        assert!(note_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn the_idle_delay_saves_and_moves_to_a_blank_note(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        cx.simulate_input("unfinished");
        // Reaches the idle delay with no save in between: the pause is handled
        // by the same clock, so check both happened.
        wait(cx, IDLE);
        assert_eq!(note_texts(dir.path()), ["unfinished"]);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(editor_text(&view, cx), "");
        assert_eq!(titles(&view, cx), ["unfinished"]);
        cx.update(|window, cx| {
            let focus = view.read(cx).editor.read(cx).focus_handle(cx);
            assert!(focus.is_focused(window), "ready to type again");
        });

        type_and_save(cx, "next thought");
        assert_eq!(note_texts(dir.path()), ["next thought", "unfinished"]);
    }

    #[gpui_kit::test]
    fn keys_clicks_and_scrolling_all_postpone_the_idle_move(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        type_and_save(cx, "still here");
        let almost = IDLE - Duration::from_secs(10);
        let is_blank = |cx: &mut VisualTestContext| selection(&view, cx) == Selection::Blank;

        wait(cx, almost);
        keys(cx, "left");
        wait(cx, almost);
        assert!(!is_blank(cx), "a key restarted the timer");

        cx.simulate_mouse_down(
            point(px(600.), px(100.)),
            gpui_kit::MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(600.), px(100.)),
            gpui_kit::MouseButton::Left,
            Modifiers::default(),
        );
        wait(cx, almost);
        assert!(!is_blank(cx), "a click restarted the timer");

        cx.simulate_event(ScrollEvent {
            position: point(px(600.), px(100.)),
            ..Default::default()
        });
        wait(cx, almost);
        assert!(!is_blank(cx), "scrolling restarted the timer");

        wait(cx, Duration::from_secs(20));
        assert!(is_blank(cx), "with no more activity the timer fires");
    }

    #[gpui_kit::test]
    fn reading_an_old_note_also_returns_to_blank_when_idle(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.md"), "old").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        assert_eq!(selection(&view, cx), Selection::Note("old.md".into()));
        assert_eq!(editor_text(&view, cx), "old");
        wait(cx, IDLE);
        assert_eq!(selection(&view, cx), Selection::Blank);
    }

    #[gpui_kit::test]
    fn undo_in_a_new_blank_note_cannot_bring_back_the_previous_note(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "private");
        keys(cx, "ctrl-n");
        assert_eq!(selection(&view, cx), Selection::Blank);
        keys(cx, "ctrl-z");
        assert_eq!(editor_text(&view, cx), "");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["private"]);
    }

    #[gpui_kit::test]
    fn undo_cannot_cross_between_existing_notes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "aaa").unwrap();
        let (dir, view, cx) = open_with(cx, dir);
        type_and_save(cx, "bbb");
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "aaa");
        keys(cx, "ctrl-z");
        assert_eq!(editor_text(&view, cx), "aaa");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["aaa", "bbb"]);
    }

    #[gpui_kit::test]
    fn switching_notes_saves_immediately_and_loads_without_marking_dirty(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.md");
        fs::write(&old, "old").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let (dir, view, cx) = open_with(cx, dir);

        cx.simulate_input("typed just now");
        keys(cx, "alt-down");
        assert_eq!(note_texts(dir.path()), ["old", "typed just now"]);
        assert_eq!(editor_text(&view, cx), "old", "moved to the note below");

        // Merely opening a note must not rewrite it or change its date.
        wait(cx, SAVE_DELAY);
        assert_eq!(
            fs::metadata(&old).unwrap().modified().unwrap(),
            SystemTime::UNIX_EPOCH
        );
    }

    #[gpui_kit::test]
    fn arrow_shortcuts_walk_the_list_and_up_from_the_top_reaches_blank(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        for text in ["first", "second"] {
            type_and_save(cx, text);
            keys(cx, "ctrl-n");
        }
        assert_eq!(titles(&view, cx), ["second", "first"]);
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "second");
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "first");
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "first", "stays on the last note");
        keys(cx, "alt-up alt-up");
        assert_eq!(selection(&view, cx), Selection::Blank);
        keys(cx, "alt-up");
        assert_eq!(selection(&view, cx), Selection::Blank, "stays on blank");
    }

    #[gpui_kit::test]
    fn the_new_note_shortcut_works_while_typing_in_the_editor(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        cx.simulate_input("just typed");
        keys(cx, "ctrl-n");
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(editor_text(&view, cx), "");
        assert_eq!(note_texts(dir.path()), ["just typed"]);
    }

    #[gpui_kit::test]
    fn trash_and_restore_round_trip(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "keep me");

        keys(cx, "ctrl-shift-backspace");
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert!(note_texts(dir.path()).is_empty());
        assert_eq!(trashed_texts(dir.path()), ["keep me"]);

        keys(cx, "ctrl-shift-t");
        let trashed = selection(&view, cx);
        assert!(matches!(trashed, Selection::Trashed(_)));
        assert_eq!(editor_text(&view, cx), "keep me");
        cx.update(|_, cx| assert!(!view.read(cx).editor.read(cx).is_editable()));

        let Selection::Trashed(id) = trashed else {
            unreachable!()
        };
        view.update_in(cx, |this, window, cx| this.restore(id, window, cx));
        cx.run_until_parked();
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert_eq!(note_texts(dir.path()), ["keep me"]);
        assert!(trashed_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn trashing_text_typed_moments_ago_loses_nothing(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        cx.simulate_input("not saved yet");
        keys(cx, "ctrl-shift-backspace");
        assert_eq!(trashed_texts(dir.path()), ["not saved yet"]);
    }

    #[gpui_kit::test]
    fn trashing_a_blank_note_does_nothing(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        keys(cx, "ctrl-shift-backspace");
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert!(trashed_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn the_trash_is_read_only_and_escape_leaves_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "gone");
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        cx.simulate_input("edit attempt");
        wait(cx, SAVE_DELAY);
        assert_eq!(editor_text(&view, cx), "gone");
        assert_eq!(trashed_texts(dir.path()), ["gone"]);

        keys(cx, "escape");
        assert_eq!(selection(&view, cx), Selection::Blank);
    }

    #[gpui_kit::test]
    fn an_empty_trash_has_nothing_to_open(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("still typing");
        keys(cx, "ctrl-shift-t");
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert_eq!(editor_text(&view, cx), "still typing");

        let bounds = cx.debug_bounds("trash-tab").unwrap();
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(!selection(&view, cx).in_trash(), "the tab is disabled");

        draw(cx);
        cx.simulate_input(", and more");
        assert_eq!(
            editor_text(&view, cx),
            "still typing, and more",
            "the click didn't take the keyboard away"
        );
    }

    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} isn't drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn the_tabs_switch_between_the_notes_and_the_trash(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        type_and_save(cx, "binned");
        keys(cx, "ctrl-shift-backspace");

        click("trash-tab", cx);
        assert!(selection(&view, cx).in_trash());
        assert_eq!(editor_text(&view, cx), "binned");
        assert!(cx.debug_bounds("empty-trash").is_some());

        click("notes-tab", cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert!(cx.debug_bounds("empty-trash").is_none());
    }

    #[gpui_kit::test]
    fn the_notes_tab_goes_back_to_the_note_shown_before_the_trash(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "was reading this").unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        fs::write(dir.path().join(".trash/b.md"), "binned").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));

        click("trash-tab", cx);
        assert_eq!(selection(&view, cx), Selection::Trashed("b.md".into()));
        click("notes-tab", cx);
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));

        // The same with the keyboard.
        keys(cx, "ctrl-shift-t");
        assert!(selection(&view, cx).in_trash());
        keys(cx, "escape");
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));
    }

    #[gpui_kit::test]
    fn typed_text_is_saved_on_the_way_to_the_trash_and_found_on_the_way_back(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        fs::write(dir.path().join(".trash/b.md"), "binned").unwrap();
        let (dir, view, cx) = open_with(cx, dir);
        cx.simulate_input("not saved yet");

        click("trash-tab", cx);
        assert_eq!(note_texts(dir.path()), ["not saved yet"]);
        click("notes-tab", cx);
        assert_eq!(editor_text(&view, cx), "not saved yet");
    }

    #[gpui_kit::test]
    fn a_note_removed_while_in_the_trash_means_a_blank_note_on_the_way_back(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "soon gone").unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        fs::write(dir.path().join(".trash/b.md"), "binned").unwrap();
        let (dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        click("trash-tab", cx);

        fs::remove_file(dir.path().join("a.md")).unwrap();
        refocus(cx);
        click("notes-tab", cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
    }

    #[gpui_kit::test]
    fn the_new_note_button_also_works_from_the_trash(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        type_and_save(cx, "binned");
        keys(cx, "ctrl-shift-backspace");
        click("trash-tab", cx);
        click("new-note", cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
    }

    #[gpui_kit::test]
    fn deleting_the_last_trashed_note_returns_to_the_note_before(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "was reading this").unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        fs::write(dir.path().join(".trash/b.md"), "binned").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        click("trash-tab", cx);
        click("empty-trash", cx);
        cx.dispatch_action(actions::Confirm { secondary: false });
        cx.run_until_parked();
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));
    }

    #[gpui_kit::test]
    fn a_trash_emptied_elsewhere_returns_to_the_notes(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        fs::remove_file(dir.path().join(".trash").join(id)).unwrap();
        refocus(cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
    }

    fn with_one_trashed_note(
        cx: &mut TestAppContext,
    ) -> (
        tempfile::TempDir,
        Entity<Brouyon>,
        &mut VisualTestContext,
        NoteId,
    ) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "trashed");
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        let Selection::Trashed(id) = selection(&view, cx) else {
            panic!("expected a trashed note");
        };
        (dir, view, cx, id)
    }

    #[gpui_kit::test]
    fn deleting_forever_asks_first_and_cancel_keeps_the_note(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::DeleteForever(id), window, cx);
        });
        draw(cx);

        // Escape answers the dialog rather than also leaving the trash.
        keys(cx, "escape");
        cx.update(|_, cx| assert!(view.read(cx).confirm.is_none()));
        assert!(selection(&view, cx).in_trash());
        assert_eq!(trashed_texts(dir.path()), ["trashed"]);
    }

    #[gpui_kit::test]
    fn app_shortcuts_are_ignored_while_a_question_is_open(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::DeleteForever(id), window, cx);
        });
        draw(cx);
        keys(cx, "ctrl-n");
        keys(cx, "ctrl-shift-t");
        assert!(selection(&view, cx).in_trash());
        cx.update(|_, cx| assert!(view.read(cx).confirm.is_some()));
        assert_eq!(trashed_texts(dir.path()), ["trashed"]);
    }

    #[gpui_kit::test]
    fn confirming_delete_forever_removes_the_file(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::DeleteForever(id), window, cx);
        });
        draw(cx);
        cx.dispatch_action(actions::Confirm { secondary: false });
        cx.run_until_parked();
        assert!(trashed_texts(dir.path()).is_empty());
        assert_eq!(selection(&view, cx), Selection::Blank, "back on the notes");
    }

    #[gpui_kit::test]
    fn empty_trash_removes_only_trashed_notes(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "kept");
        keys(cx, "ctrl-n");
        type_and_save(cx, "binned");
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::EmptyTrash, window, cx);
        });
        draw(cx);
        cx.dispatch_action(actions::Confirm { secondary: false });
        cx.run_until_parked();
        assert!(trashed_texts(dir.path()).is_empty());
        assert_eq!(note_texts(dir.path()), ["kept"]);
    }

    #[gpui_kit::test]
    fn the_idle_move_waits_for_an_open_question(cx: &mut TestAppContext) {
        let (_dir, view, cx, id) = with_one_trashed_note(cx);
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::DeleteForever(id), window, cx);
        });
        draw(cx);
        wait(cx, IDLE * 2);
        assert!(selection(&view, cx).in_trash());
        cx.update(|_, cx| assert!(view.read(cx).confirm.is_some()));
    }

    fn shortcuts_open(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| view.read(cx).shortcuts_open)
    }

    #[gpui_kit::test]
    fn both_keys_open_the_shortcuts_and_they_close_again(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        for key in ["f1", "ctrl-/"] {
            keys(cx, key);
            assert!(shortcuts_open(&view, cx), "{key} opens them");
            keys(cx, key);
            assert!(!shortcuts_open(&view, cx), "{key} closes them");
        }

        keys(cx, "f1");
        keys(cx, "escape");
        assert!(!shortcuts_open(&view, cx), "so does Esc");
        cx.update(|window, cx| {
            let focus = view.read(cx).editor.read(cx).focus_handle(cx);
            assert!(focus.is_focused(window), "ready to type again");
        });
    }

    #[gpui_kit::test]
    fn a_question_mark_goes_in_the_note(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("?");
        assert_eq!(editor_text(&view, cx), "?");
        assert!(!shortcuts_open(&view, cx));
    }

    #[gpui_kit::test]
    fn the_shortcuts_hold_the_keyboard_and_the_idle_move(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "open note");
        keys(cx, "f1");

        keys(cx, "ctrl-n");
        keys(cx, "ctrl-shift-backspace");
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert_eq!(note_texts(dir.path()), ["open note"]);

        wait(cx, IDLE * 2);
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert!(shortcuts_open(&view, cx));

        keys(cx, "escape");
        wait(cx, IDLE);
        assert_eq!(selection(&view, cx), Selection::Blank, "the timer goes on");
    }

    #[gpui_kit::test]
    fn the_shortcuts_key_leaves_an_open_question_alone(cx: &mut TestAppContext) {
        let (_dir, view, cx, id) = with_one_trashed_note(cx);
        view.update_in(cx, |this, window, cx| {
            this.ask(Confirm::DeleteForever(id), window, cx);
        });
        draw(cx);
        keys(cx, "f1");
        assert!(!shortcuts_open(&view, cx));
        cx.update(|_, cx| assert!(view.read(cx).confirm.is_some()));
    }

    #[gpui_kit::test]
    fn both_redo_keys_bring_back_what_undo_took(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("words");
        for redo in ["ctrl-shift-z", "ctrl-y"] {
            keys(cx, "ctrl-z");
            assert_eq!(editor_text(&view, cx), "", "before {redo}");
            keys(cx, redo);
            assert_eq!(editor_text(&view, cx), "words", "{redo}");
        }
    }

    fn set_desktop(cx: &mut VisualTestContext, change: impl FnOnce(&mut Desktop)) {
        cx.update(|_, cx| change(cx.global_mut::<Desktop>()));
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn only_the_root_background_follows_the_transparency_mode(cx: &mut TestAppContext) {
        let (_dir, _view, cx) = open(cx);
        let alpha = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| desktop::root_background(window, cx).a)
        };
        assert_eq!(alpha(cx), 1., "solid until told otherwise");

        set_desktop(cx, |desktop| {
            desktop.transparency = desktop::Transparency::Glassy;
        });
        let active = cx.update(|window, _| window.is_window_active());
        assert_eq!(alpha(cx), desktop::Transparency::Glassy.alpha(active));
        cx.update(|_, cx| {
            assert_eq!(cx.omarchy().background.a, 1., "dialogs paint this one");
        });
    }

    #[gpui_kit::test]
    fn a_fixed_theme_stays_when_the_desktop_changes(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        view.update(cx, |this, cx| this.set_theme(ThemeMode::Light, cx));
        set_desktop(cx, |desktop| {
            desktop.transparency = desktop::Transparency::Glassy;
            desktop.font = Some("Another Mono".into());
        });
        cx.update(|_, cx| {
            let theme = cx.omarchy();
            assert_eq!(theme.name, Theme::flexoki_light().name);
            assert_eq!(theme.mono_font, "Another Mono", "the font still follows");
        });
    }

    #[gpui_kit::test]
    fn a_changed_font_survives_every_theme(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        set_desktop(cx, |desktop| desktop.font = Some("Another Mono".into()));
        for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Omarchy] {
            view.update(cx, |this, cx| this.set_theme(mode, cx));
            // Parking at all shows that observing the theme doesn't feed itself.
            cx.run_until_parked();
            cx.update(|_, cx| {
                assert_eq!(cx.omarchy().font, "Another Mono", "{mode:?}");
                assert_eq!(cx.omarchy().mono_font, "Another Mono", "{mode:?}");
            });
        }
    }

    #[gpui_kit::test]
    fn a_failed_save_keeps_the_text_and_stays_on_the_note(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        fs::remove_dir_all(dir.path()).unwrap();

        cx.simulate_input("precious");
        wait(cx, SAVE_DELAY);
        cx.update(|_, cx| assert!(view.read(cx).problem.is_some()));

        keys(cx, "ctrl-n");
        assert_eq!(editor_text(&view, cx), "precious", "didn't navigate away");
        wait(cx, IDLE);
        assert_eq!(editor_text(&view, cx), "precious", "idle didn't either");

        // Once saving works again, the next attempt succeeds and clears the error.
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        wait(cx, IDLE);
        assert_eq!(note_texts(dir.path()), ["precious"]);
        cx.update(|_, cx| assert!(view.read(cx).problem.is_none()));
    }

    #[gpui_kit::test]
    fn closing_the_window_saves_pending_text(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        cx.simulate_input("last words");
        assert!(note_texts(dir.path()).is_empty());
        assert!(cx.simulate_close());
        assert_eq!(note_texts(dir.path()), ["last words"]);
    }

    #[gpui_kit::test]
    fn losing_focus_saves_pending_text(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.simulate_input("switching away");
        cx.deactivate_window();
        assert_eq!(note_texts(dir.path()), ["switching away"]);
    }

    /// Refocuses the window, which is what triggers looking at the folder again.
    fn refocus(cx: &mut VisualTestContext) {
        cx.deactivate_window();
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn a_note_edited_outside_the_app_reloads_on_focus(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        fs::write(&path, "original").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "original");

        fs::write(&path, "changed elsewhere").unwrap();
        refocus(cx);
        assert_eq!(editor_text(&view, cx), "changed elsewhere");
        assert_eq!(titles(&view, cx), ["changed elsewhere"]);
    }

    #[gpui_kit::test]
    fn a_new_file_appears_and_a_deleted_open_note_goes_blank_on_focus(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "will vanish");
        let Selection::Note(id) = selection(&view, cx) else {
            panic!("expected a saved note");
        };
        fs::remove_file(dir.path().join(id)).unwrap();
        fs::write(dir.path().join("arrived.md"), "arrived").unwrap();
        refocus(cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(titles(&view, cx), ["arrived"]);
    }

    /// Renders every screen to PNGs for a person to review, with no window on the
    /// desktop and the theme only changed inside this process:
    /// `BROUYON_SCREENSHOTS=some/folder cargo test screenshots -- --ignored`
    #[test]
    #[ignore = "writes images for visual review"]
    fn screenshots() {
        use gpui_kit::{AnyWindowHandle, AppContext as _, HeadlessAppContext};
        use std::sync::Arc;

        const SAMPLE: &str = "# Courses du samedi é\n\n\
            Du **gras**, de l'*italique*, du `code`, un [lien](https://omarchy.org) et ~~barré~~.\n\n\
            ## Un sous-titre avec **du gras**\n\n\
            - pain\n- [x] café\n- [ ] 日本語\n\n1. premier\n2. second\n\n\
            > une citation sur une ligne\n\n---\n\n\
            ```\ndu code\n```\n\n\
            Une très longue ligne pour vérifier que le texte passe bien à la ligne tout seul \
            dans la fenêtre sans jamais avoir besoin de faire défiler horizontalement, parce \
            que personne ne veut ça dans un bloc-notes, même sur un écran très large.\n";

        let out = std::path::PathBuf::from(
            std::env::var_os("BROUYON_SCREENSHOTS").expect("set BROUYON_SCREENSHOTS to a folder"),
        );
        fs::create_dir_all(&out).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for (name, text) in [
            ("2026-11-06 09-40-00.md", SAMPLE),
            ("2026-11-05 18-00-00.md", "Idée pour plus tard\nsuite"),
            (
                "2026-11-04 12-00-00.md",
                "Un titre vraiment très long qui ne tient pas dans la largeur de la colonne",
            ),
            (
                ".trash/2026-11-01 08-00-00.md",
                "# Note jetée\n\nDans la corbeille.",
            ),
            (".trash/2026-10-30 08-00-00.md", "Une autre note jetée"),
        ] {
            fs::write(dir.path().join(name), text).unwrap();
        }

        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(gpui_kit::assets::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        // As `main` starts, so that these pictures show what a real window does.
        cx.update(|cx| {
            gpui_omarchy::init(cx);
            init(cx);
            apply_theme(ThemeMode::Omarchy, cx);
        });

        fn capture(
            cx: &mut HeadlessAppContext,
            window: AnyWindowHandle,
            path: &Path,
        ) -> image::RgbaImage {
            let draw = |cx: &mut HeadlessAppContext| {
                cx.run_until_parked();
                cx.update_window(window, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                })
                .unwrap();
            };
            // Button fills ease to a new state on this clock, which only moves
            // when told to: draw once to start them, then again at their end.
            draw(cx);
            cx.advance_clock(Duration::from_millis(200));
            draw(cx);
            let image = cx.capture_screenshot(window).unwrap();
            assert!(
                image
                    .pixels()
                    .any(|pixel| pixel.0[..3] != image.get_pixel(0, 0).0[..3])
            );
            image.save(path).unwrap();
            image
        }

        let mut config = Config::for_test(dir.path());
        config.idle = None;
        let narrow_config = config.clone();
        let window = cx
            .open_window(size(px(1060.), px(760.)), |window, cx| {
                cx.new(|cx| Brouyon::new(store, config, window, cx))
            })
            .unwrap();
        let any_window: AnyWindowHandle = window.into();

        capture(&mut cx, any_window, &out.join("01-blank.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.select(Selection::Note("2026-11-06 09-40-00.md".into()), window, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("02-note.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.problem = Some("Couldn't save: disque plein (os error 28)".into());
                cx.notify();
                let _ = window;
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("03-save-error.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.problem = None;
                this.toggle_trash(window, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("04-trash.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.ask(Confirm::EmptyTrash, window, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("05-confirm.png"));

        window
            .update(&mut cx, |this, window, cx| this.answer(false, window, cx))
            .unwrap();

        // Each page of the floating window, in its usual size.
        let app = window.update(&mut cx, |_, _, cx| cx.entity()).unwrap();
        let settings = cx
            .open_window(size(px(760.), px(560.)), |window_, cx| {
                cx.new(|cx| SettingsWindow::new(app.clone(), window, window_, cx))
            })
            .unwrap();
        let any_settings: AnyWindowHandle = settings.into();
        capture(
            &mut cx,
            any_settings,
            &out.join("06-settings-appearance.png"),
        );
        settings
            .update(&mut cx, |this, _, cx| this.show_section_for_test(1, cx))
            .unwrap();
        capture(&mut cx, any_settings, &out.join("07-settings-notes.png"));
        settings
            .update(&mut cx, |this, window, cx| {
                this.open_idle_list_for_test(window, cx);
            })
            .unwrap();
        capture(
            &mut cx,
            any_settings,
            &out.join("08-settings-idle-list.png"),
        );
        settings
            .update(&mut cx, |this, window, cx| {
                this.open_idle_list_for_test(window, cx);
                this.show_section_for_test(2, cx);
            })
            .unwrap();
        capture(&mut cx, any_settings, &out.join("09-settings-about.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.show_notes(window, cx);
                this.select(Selection::Note("2026-11-06 09-40-00.md".into()), window, cx);
                this.set_theme(ThemeMode::Light, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("10-light-theme.png"));
        settings
            .update(&mut cx, |this, _, cx| this.show_section_for_test(0, cx))
            .unwrap();
        capture(&mut cx, any_settings, &out.join("11-settings-light.png"));

        window
            .update(&mut cx, |this, _, cx| {
                this.set_theme(ThemeMode::Dark, cx);
                this.set_text_percent(200, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("12-large-text.png"));
        settings
            .update(&mut cx, |this, _, cx| this.show_section_for_test(1, cx))
            .unwrap();
        capture(
            &mut cx,
            any_settings,
            &out.join("13-settings-large-text.png"),
        );

        let narrow_window = cx
            .open_window(size(px(500.), px(420.)), |window, cx| {
                let store = Store::open(dir.path()).unwrap();
                cx.new(|cx| Brouyon::new(store, narrow_config, window, cx))
            })
            .unwrap();
        let narrow: AnyWindowHandle = narrow_window.into();
        window
            .update(&mut cx, |this, _, cx| this.set_text_percent(100, cx))
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow.png"));
        narrow_window
            .update(&mut cx, |this, window, cx| {
                this.toggle_shortcuts(window, cx);
            })
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow-shortcuts.png"));

        window
            .update(&mut cx, |this, window, cx| {
                this.toggle_shortcuts(window, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("15-shortcuts.png"));
        window
            .update(&mut cx, |this, window, cx| {
                this.toggle_shortcuts(window, cx);
                this.select(Selection::Blank, window, cx);
                this.set_theme(ThemeMode::Light, cx);
            })
            .unwrap();

        // The same screen solid and then as glass. Headless draws over black, so
        // whatever is see-through comes out darker by its alpha. A light theme makes
        // that easy to measure, and any solid layer left behind shows up as a failure.
        let solid = capture(&mut cx, any_window, &out.join("16-solid.png"));
        cx.update(|cx| cx.global_mut::<Desktop>().transparency = Transparency::Glassy);
        let glassy = capture(&mut cx, any_window, &out.join("17-glassy.png"));
        for (place, x, y) in [
            ("note list", 120, 700),
            ("editor", 700, 500),
            ("status strip", 600, 742),
        ] {
            let before = solid.get_pixel(x, y).0;
            let after = glassy.get_pixel(x, y).0;
            for channel in 0..3 {
                let ratio = f32::from(after[channel]) / f32::from(before[channel]);
                assert!(
                    (0.58..0.70).contains(&ratio),
                    "{place}: {before:?} became {after:?}"
                );
            }
        }
        capture(&mut cx, any_settings, &out.join("18-glassy-settings.png"));

        // A dialog is the one place the solid background can still appear.
        let background = solid.get_pixel(120, 700).0;
        let has_solid_background =
            |image: &image::RgbaImage| image.pixels().any(|pixel| pixel.0 == background);
        assert!(!has_solid_background(&glassy));
        window
            .update(&mut cx, |this, window, cx| {
                this.ask(Confirm::EmptyTrash, window, cx);
            })
            .unwrap();
        let confirm = capture(&mut cx, any_window, &out.join("19-glassy-confirm.png"));
        assert!(has_solid_background(&confirm));
        window
            .update(&mut cx, |this, window, cx| this.answer(false, window, cx))
            .unwrap();

        // The picture at the top of the README: made-up notes and the default palette,
        // so it doesn't depend on who runs this or on the current theme.
        let readme_dir = tempfile::tempdir().unwrap();
        for (days_ago, name, text) in [
            (
                0,
                "2026-11-06 09-40-00.md",
                "# Ideas for the weekend\n\n\
                 Walk to the old mill if it stays dry.\n\n\
                 Otherwise the **market**, and the *bakery* on the way back.\n\n\
                 ## To bring\n\n- [x] Thermos\n- [ ] Rain jacket\n- [ ] Map of the valley\n\n\
                 > Leave before the bakery sells out.\n\n\
                 Ask about the `opening hours` on Sunday.\n",
            ),
            (
                1,
                "2026-11-05 18-00-00.md",
                "Book recommendations\nfrom the club",
            ),
            (
                2,
                "2026-11-04 12-00-00.md",
                "Fix the shelf in the hallway\nTwo screws are missing",
            ),
            (
                4,
                "2026-11-02 08-30-00.md",
                "Questions for the plumber\nWhy does the tap drip at night?",
            ),
        ] {
            let path = readme_dir.path().join(name);
            fs::write(&path, text).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(days_ago * 86_400))
                .unwrap();
        }
        cx.update(|cx| {
            cx.global_mut::<Desktop>().transparency = Transparency::Opaque;
            Theme::tokyo_night().apply(cx);
        });
        let readme_store = Store::open(readme_dir.path()).unwrap();
        let mut readme_config = Config::for_test(readme_dir.path());
        readme_config.idle = None;
        let readme = cx
            .open_window(size(px(1060.), px(760.)), |window, cx| {
                cx.new(|cx| Brouyon::new(readme_store, readme_config, window, cx))
            })
            .unwrap();
        readme
            .update(&mut cx, |this, window, cx| {
                this.select(Selection::Note("2026-11-06 09-40-00.md".into()), window, cx);
            })
            .unwrap();
        capture(&mut cx, readme.into(), &out.join("readme.png"));
    }

    #[test]
    fn dates_are_short_and_relative_to_now() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 6, 15, 0, 0)
            .unwrap();
        let at = |y, m, d, h, min| -> SystemTime {
            chrono::Local
                .with_ymd_and_hms(y, m, d, h, min, 0)
                .unwrap()
                .into()
        };
        assert_eq!(format_date(at(2026, 11, 6, 9, 42), now), "09:42");
        assert_eq!(format_date(at(2026, 11, 5, 23, 59), now), "Yesterday");
        assert_eq!(format_date(at(2026, 11, 4, 12, 0), now), "4 Nov");
        assert_eq!(format_date(at(2025, 12, 31, 12, 0), now), "31 Dec 2025");
    }
}
