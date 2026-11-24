//! The Brouyon window: which note is open, when it is saved, when the app moves
//! on to a blank note, and how all of that is drawn.

use crate::{
    checked::{IdleDelay, NotesDir, TextPercent},
    config::{Config, CopyFormat, ThemeMode},
    desktop::{self, Desktop},
    kept::{self, Kept, MayLeave, SaveState, Warned},
    lists,
    look::MAC,
    macos, markdown,
    settings::{self, SettingsWindow},
    shortcuts::{self, primary},
    store::{self, Note, NoteId, Saved, Store},
    title_bar::{self, BAR_HEIGHT, TitleBar},
    watch::FolderWatcher,
};
use chrono::Datelike;
use gpui_kit::base::{
    Tab, Tabs, VirtualListScrollHandle,
    input::{Editor, EditorState, InputEvent, InputState, Redo},
};
use gpui_kit::{
    Anchor, App, ClipboardItem, Context, DispatchPhase, Entity, Focusable, FontWeight, IntoElement,
    MouseButton, Pixels, Render, ScrollEvent, ScrollStrategy, SharedString, Styled, Subscription,
    Task, Window, WindowHandle, canvas, div, prelude::*, px, relative, rems, size,
    transparent_black,
};
use gpui_omarchy::{
    ActiveTheme, Button, ButtonVariant, IconName, MenuItem, Theme, alert_dialog, button, dialog,
    dialog_button, dialog_description, dialog_popup, dialog_title, icon, input, menu, virtual_list,
    with_tooltip,
};
use std::{
    io,
    rc::Rc,
    time::{Duration, SystemTime},
};

/// Saving waits for this pause in typing, so a burst of keys is one write.
const SAVE_DELAY: Duration = Duration::from_millis(400);
/// How long the strip under the note says that it was copied.
const COPIED_NOTICE: Duration = Duration::from_secs(2);
/// How long it says what became of a note that changed elsewhere: long enough to
/// read while typing.
const CHANGE_NOTICE: Duration = Duration::from_secs(8);
/// A sync client brings its changes in a burst, which is read once it ends.
const FOLDER_SETTLE: Duration = Duration::from_millis(100);
pub(crate) const KEY_CONTEXT: &str = "Brouyon";

gpui_kit::actions!(
    brouyon,
    [
        NewNote,
        FindNote,
        PreviousNote,
        NextNote,
        ToggleTask,
        CopyNote,
        MoveToTrash,
        ToggleTrash,
        GoBack,
        OpenSettings,
        IncreaseTextSize,
        DecreaseTextSize,
        ResetTextSize,
        ToggleShortcuts,
        CloseWindow,
        Quit
    ]
);

pub fn init(cx: &mut App) {
    use gpui_kit::KeyBinding;
    desktop::init(cx);
    cx.on_action(|_: &Quit, cx| cx.defer(ask_to_quit));
    // Scoped to the app's context: a binding with no context would outrank the
    // editor's and the dialog's, so Escape would skip past an open question.
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new(primary!("n"), NewNote, context),
        KeyBinding::new(primary!("f"), FindNote, context),
        KeyBinding::new("alt-up", PreviousNote, context),
        KeyBinding::new("alt-down", NextNote, context),
        KeyBinding::new(primary!("shift-c"), CopyNote, context),
        KeyBinding::new(primary!("shift-backspace"), MoveToTrash, context),
        KeyBinding::new(primary!("shift-t"), ToggleTrash, context),
        KeyBinding::new("escape", GoBack, context),
        KeyBinding::new(primary!(","), OpenSettings, context),
        KeyBinding::new(primary!("="), IncreaseTextSize, context),
        KeyBinding::new(primary!("+"), IncreaseTextSize, context),
        KeyBinding::new(primary!("-"), DecreaseTextSize, context),
        KeyBinding::new(primary!("0"), ResetTextSize, context),
        // Typing `?` has to reach the note. F1 is here too because `/` takes Shift on
        // some layouts, such as AZERTY.
        KeyBinding::new("f1", ToggleShortcuts, context),
        KeyBinding::new(primary!("/"), ToggleShortcuts, context),
        KeyBinding::new(primary!("q"), Quit, context),
        // On Linux gpui-base only binds Ctrl+Y, and on macOS it already binds this
        // one. `Input` is its editor's key context.
        KeyBinding::new(primary!("shift-z"), Redo, Some("Input")),
        // In that context to come before the editor's own use of the key, a newline.
        KeyBinding::new(primary!("enter"), ToggleTask, Some("Input")),
    ]);
    settings::bind_keys(cx);

    // Omarchy's shell uses one face throughout, while gpui-omarchy's widgets use
    // the theme's UI font. Every new theme, including one the desktop switches
    // to, gets the system monospace in its place.
    use_mono_font(cx);
    cx.observe_global::<Theme>(use_mono_font).detach();
    cx.observe_global::<Desktop>(use_mono_font).detach();
}

/// The only way out, from any window, the menu or with no window at all: whoever holds
/// the text is asked.
fn ask_to_quit(cx: &mut App) {
    let holder = |window: gpui_kit::AnyWindowHandle| window.downcast::<Brouyon>();
    match cx.windows().into_iter().find_map(holder) {
        Some(window) => {
            window.update(cx, |this, _, cx| this.quit(cx)).ok();
        }
        None => kept::quit_without_windows(cx),
    }
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
    // On a Mac the system's accent goes over every palette. Each palette also brings
    // back the corners gpui-omarchy follows, which are square without Hyprland, so a
    // Mac's go on last.
    if MAC {
        macos::use_system_accent(cx);
        macos::round_corners(cx);
    }
    // Not left to the observer: at startup it isn't active yet, and it never hears of
    // this theme, because GPUI drops a notification when one is already queued.
    use_mono_font(cx);
}

/// A Mac has no Omarchy theme, so the palette follows its light or dark appearance.
/// `macos::follow_appearance` applies it again when that changes.
#[cfg(not(test))]
fn follow_desktop_theme(cx: &mut App) {
    use gpui_kit::WindowAppearance;
    if !MAC {
        return Theme::follow_system(cx);
    }
    match cx.window_appearance() {
        WindowAppearance::Light | WindowAppearance::VibrantLight => {
            Theme::flexoki_light().apply(cx);
        }
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::tokyo_night().apply(cx),
    }
}

/// The same palette, without watching the real home folder. Any file changing there
/// would wake the test scheduler from another thread, which aborts the whole run.
#[cfg(test)]
fn follow_desktop_theme(cx: &mut App) {
    Theme::system_or_default().apply(cx);
}

// The spacing of the note list, in rems. On a Mac each note is a rounded highlight
// set in from the list's sides, with space between them and at the list's ends, as
// in Finder's sidebar. On Linux the rows fill the list and touch.

/// Between the list's sides and a note's highlight.
const ROW_INSET: f32 = if MAC { 0.375 } else { 0. };
/// Between a note's highlight and its text.
const ROW_PADDING: f32 = if MAC { 0.625 } else { 0.75 };
/// Between two notes' highlights.
const ROW_GAP: f32 = if MAC { 0.25 } else { 0. };
/// Above the first note and below the last, besides half the gap.
const LIST_END: f32 = if MAC { 0.375 } else { 0. };

/// The colours of a note's row: its highlight, if any, its title and the date and
/// preview under it.
struct RowColors {
    fill: Option<gpui_kit::Hsla>,
    title: gpui_kit::Hsla,
    detail: gpui_kit::Hsla,
}

/// The note being shown stands out in the theme's accent. On a Mac, as in Finder,
/// its highlight is the system's accent with white text while the window is in front,
/// and grey once it isn't.
fn row_colors(t: &Theme, selected: bool, active: bool) -> RowColors {
    match (selected, MAC, active) {
        (false, ..) => RowColors {
            fill: None,
            title: t.foreground,
            detail: t.secondary,
        },
        (true, false, _) => RowColors {
            fill: Some(t.hover_fill()),
            title: t.accent,
            detail: t.secondary,
        },
        (true, true, true) => RowColors {
            fill: Some(t.accent),
            title: t.on_accent,
            detail: t.on_accent.opacity(0.8),
        },
        (true, true, false) => RowColors {
            fill: Some(t.selected_fill()),
            title: t.foreground,
            detail: t.secondary,
        },
    }
}

/// The space between the note's text and the window's right edge, in rems.
const NOTE_RIGHT: f32 = 1.;

/// The space the editor keeps inside its own edges: it keeps the note off the
/// window's right edge itself, so that its scrollbar runs along that edge.
fn note_paddings(window: &Window) -> gpui_kit::Edges<Pixels> {
    gpui_kit::Edges {
        right: rems(NOTE_RIGHT).to_pixels(window.rem_size()),
        ..Default::default()
    }
}

/// The space above and below a note's highlight, in rems: half the gap on each side,
/// and at the list's ends, its own space besides.
fn row_spacing(row: usize, count: usize) -> (f32, f32) {
    let half = ROW_GAP / 2.;
    let end = |at_end: bool| if at_end { half + LIST_END } else { half };
    (end(row == 0), end(row + 1 == count))
}

/// The height of the buttons in the bars along the bottom of the main window, in rems.
const BAR_BUTTON_HEIGHT: f32 = 1.875;

/// How far a bottom bar's buttons sit from the window's bottom edge: the bar under
/// its top border, less the button, shared above and below. On a Mac the buttons at
/// the ends keep as far from the side edges, so their corners can follow the
/// window's.
fn bar_inset(window: &Window) -> Pixels {
    let rem = window.rem_size();
    (rems(BAR_HEIGHT).to_pixels(rem) - px(1.) - rems(BAR_BUTTON_HEIGHT).to_pixels(rem)) / 2.
}

/// A button in a bottom bar. On a Mac its corners are concentric with the window's:
/// as round as the window's, less the distance between them.
fn bar_button(button: Button, window: &Window) -> Button {
    button.h(rems(BAR_BUTTON_HEIGHT)).when(MAC, |button| {
        button.rounded((px(macos::WINDOW_CORNER) - bar_inset(window)).max(px(0.)))
    })
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

/// The open note's file as it was last read or written. A save that finds the file
/// holding anything else leaves it alone, so that what another program or another
/// device wrote is never lost. Compared with the file, never with the editor.
struct Loaded {
    text: String,
    modified: SystemTime,
}

/// What a save did with the editor's text.
enum Flushed {
    /// There was nothing to write: no text, or the text the file already holds.
    Nothing,
    /// The note's file holds the text. For a blank note, the file is new.
    Wrote(NoteId),
    /// The note was emptied and its file removed.
    Removed,
    /// The note was emptied, but its file changed elsewhere and was kept.
    KeptTheirs,
    /// The file changed elsewhere, so the text went into a new note beside it.
    KeptBoth(NoteId),
}

/// A destructive action waiting for an answer in the confirmation dialog.
#[derive(Clone)]
enum Confirm {
    /// The note, and its text as it was shown.
    DeleteForever(NoteId, SharedString),
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
    _editor_events: [Subscription; 2],
    /// What the editor held when last looked at, to tell a change to its text from
    /// anything else that happens in it.
    seen: SharedString,
    /// The field above the list. What it holds only counts while it is open.
    filter: Entity<InputState>,
    filter_open: bool,
    /// What the list was last narrowed down to, to start again from its top when it changes.
    listed_for: Vec<String>,
    save_state: SaveState,
    /// `None` for a blank note and in the Trash, which have no file to save to.
    loaded: Option<Loaded>,
    problem: Option<SharedString>,
    /// What became of a note that changed elsewhere, said for a moment.
    notice: Option<SharedString>,
    notice_task: Option<Task<()>>,
    /// Follows the notes folder, once [`Self::watch_notes`] has started it.
    notes_watch: Option<Task<()>>,
    /// What went wrong with the last setting, shown until one succeeds.
    pub settings_problem: Option<SharedString>,
    settings_window: Option<WindowHandle<SettingsWindow>>,
    save_task: Option<Task<()>>,
    idle_task: Option<Task<()>>,
    /// How many words the editor holds, kept up as they are typed.
    words: usize,
    /// How the open note was just copied, for the strip to say so.
    copied: Option<CopyFormat>,
    copied_task: Option<Task<()>>,
    /// The menu of formats beside the copy button is showing.
    copy_menu_open: bool,
    confirm: Option<Confirm>,
    shortcuts_open: bool,
    /// Held by whichever dialog is open.
    dialog_focus: gpui_kit::FocusHandle,
    list_scroll: VirtualListScrollHandle,
    title_bar: TitleBar,
    _subscriptions: Vec<Subscription>,
}

impl Brouyon {
    pub fn new(store: Store, config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = build_editor("", false, window, cx);
        let editor_events = Self::follow_editor(&editor, window, cx);
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let subscriptions = vec![
            cx.subscribe_in(&filter, window, Self::on_filter_event),
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
            view.update(cx, |this, cx| this.on_close(cx).is_some())
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
            seen: SharedString::default(),
            filter,
            filter_open: false,
            listed_for: Vec::new(),
            save_state: SaveState::default(),
            loaded: None,
            problem: None,
            notice: None,
            notice_task: None,
            notes_watch: None,
            settings_problem: None,
            settings_window: None,
            save_task: None,
            idle_task: None,
            words: 0,
            copied: None,
            copied_task: None,
            copy_menu_open: false,
            confirm: None,
            shortcuts_open: false,
            dialog_focus: cx.focus_handle(),
            list_scroll: VirtualListScrollHandle::new(),
            title_bar: TitleBar::default(),
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
        self.seen = self.editor.read(cx).value();
        self.words = markdown::word_count(text);
        self._editor_events = Self::follow_editor(&self.editor, window, cx);
        self.focus_content(window, cx);
    }

    fn follow_editor(
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [Subscription; 2] {
        [
            cx.observe_in(editor, window, Self::on_editor_notify),
            cx.subscribe_in(editor, window, Self::on_editor_event),
        ]
    }

    /// Looks for a change to the text, whatever happened in the editor. Its
    /// `InputEvent::Change` is no substitute: it isn't emitted for what an input method
    /// composes, which stays in the text when the composition is just ended.
    fn on_editor_notify(
        &mut self,
        editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = editor.read(cx);
        if *editor.text() == *self.seen {
            return;
        }
        self.seen = editor.value();
        self.save_state.changed();
        let words = markdown::word_count(&self.seen);
        if words != self.words {
            self.words = words;
            cx.notify();
        }
        self.restart_idle_timer(window, cx);
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                // A change that arrived during the typing was left for
                // the save to settle, and can be taken now.
                if this.flush(cx).is_some() {
                    this.refresh_open_note(window, cx);
                }
            })
            .ok();
        }));
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The editor has made its new line by now. Shift+Enter keeps it plain.
        if let InputEvent::PressEnter { shift: false, .. } = event {
            self.edit_list(lists::after_enter, window, cx);
        }
    }

    fn on_filter_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Enter opens the first note found. With nothing to find, it goes back
            // to the note.
            InputEvent::PressEnter { .. } if self.terms(cx).is_empty() => {
                self.close_filter();
                self.focus_content(window, cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                if let Some(first) = self.rows(cx).into_iter().next() {
                    self.select(first, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Shows the field that narrows the list down, empty, and puts the cursor in it.
    /// Already shown, its text is selected, ready to be typed over.
    fn open_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nothing_kept = self.terms(cx).is_empty();
        self.filter_open = true;
        self.filter.update(cx, |state, cx| {
            if nothing_kept {
                state.set_value("", window, cx);
            }
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.notify();
    }

    /// Lists every note again. Whoever calls this decides where the keyboard goes.
    fn close_filter(&mut self) {
        self.filter_open = false;
    }

    /// The words a note has to hold to be listed. None lists every note.
    fn terms(&self, cx: &App) -> Vec<String> {
        if !self.filter_open {
            return Vec::new();
        }
        store::terms(&self.filter.read(cx).value())
    }

    /// Makes the edit `edit` finds for the text and the cursor, if any. It goes
    /// through the editor like typing does, so it is saved and can be undone.
    fn edit_list(
        &mut self,
        edit: fn(&str, usize) -> Option<lists::Edit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |state, cx| {
            let selection = state.selected_range();
            if !state.is_editable() || !selection.is_empty() {
                return;
            }
            let Some(edit) = edit(&state.value(), selection.start) else {
                return;
            };
            let cursor = edit.moved(selection.start);
            state.set_selected_range(edit.range, cx);
            state.replace(edit.text, window, cx);
            state.set_selected_range(cursor..cursor, cx);
        });
    }

    /// Writes any pending text. Nothing if it could not be saved, in which case it
    /// stays in the editor.
    ///
    /// A file that changed elsewhere since it was read is never written over: the
    /// text goes into a new note beside it, which the editor carries on in.
    fn flush(&mut self, cx: &mut Context<Self>) -> Option<Kept> {
        self.save_task = None;
        if let Some(kept) = self.save_state.kept() {
            return Some(kept);
        }
        let text = self.editor.read(cx).value();
        let empty = text.trim().is_empty();
        // A note always has what it was loaded as. Should it not, nothing but a
        // missing file would match, and the text would go beside the file.
        let loaded = self.loaded.as_ref().map_or("", |loaded| &loaded.text);
        let flushed = match &self.selection {
            // Typed and taken back, such as with undo.
            Selection::Note(_) if loaded == &*text => Ok(Flushed::Nothing),
            Selection::Note(id) if empty => {
                self.store
                    .remove_unchanged(id, loaded)
                    .map(|removed| match removed {
                        true => Flushed::Removed,
                        false => Flushed::KeptTheirs,
                    })
            }
            Selection::Note(id) => match self.store.save(id, loaded, &text) {
                Ok(Saved::Written) => Ok(Flushed::Wrote(id.clone())),
                Ok(Saved::Changed) => self.store.create(&text).map(Flushed::KeptBoth),
                Err(error) => Err(error),
            },
            Selection::Blank if !empty => self.store.create(&text).map(Flushed::Wrote),
            _ => Ok(Flushed::Nothing),
        };
        match flushed {
            Ok(flushed) => {
                let kept = self.save_state.saved();
                self.problem = None;
                self.record_save(flushed, &text, cx);
                cx.notify();
                Some(kept)
            }
            Err(error) => {
                self.save_state.refused();
                self.problem = Some(format!("Couldn't save: {error}").into());
                cx.notify();
                None
            }
        }
    }

    /// Follows a save in the list and in what is open. An ordinary save moves the
    /// note to the top without rescanning the folder on every pause in typing.
    fn record_save(&mut self, flushed: Flushed, text: &str, cx: &mut Context<Self>) {
        let (id, kept_both) = match flushed {
            Flushed::Nothing => return,
            Flushed::Removed | Flushed::KeptTheirs => {
                if matches!(flushed, Flushed::KeptTheirs) {
                    self.reload_lists();
                    self.say("Changed elsewhere, so it wasn't removed.", cx);
                } else if let Selection::Note(id) = &self.selection {
                    self.notes.retain(|note| note.id != *id);
                }
                self.selection = Selection::Blank;
                self.loaded = None;
                return;
            }
            Flushed::Wrote(id) => (id, false),
            Flushed::KeptBoth(id) => (id, true),
        };
        let previous_row = match &self.selection {
            Selection::Note(open) => self.notes.iter().position(|note| note.id == *open),
            _ => None,
        };
        // A note just begun has to be in the list, whatever was being looked for.
        if self.selection == Selection::Blank {
            self.close_filter();
        }
        let modified = self
            .store
            .modified(&id)
            .unwrap_or_else(|_| SystemTime::now());
        if kept_both {
            // The other version is a note of its own from now on, to list as it is.
            self.reload_lists();
            self.say("Changed elsewhere. Your version is a new note.", cx);
        } else {
            self.notes.retain(|note| note.id != id);
            self.notes
                .insert(0, Note::new(id.clone(), text.to_owned(), modified));
        }
        self.selection = Selection::Note(id);
        self.loaded = Some(Loaded {
            text: text.to_owned(),
            modified,
        });
        if kept_both || previous_row != Some(0) {
            self.reveal_selection(cx);
        }
    }

    /// Says `notice` under the note for a moment.
    fn say(&mut self, notice: &'static str, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHANGE_NOTICE).await;
            this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            })
            .ok();
        }));
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

    /// Opens `selection`, saving the current note first. Stays put if either step fails.
    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kept) = self.flush(cx) else {
            return;
        };
        // Reselecting the open note keeps its cursor and undo history. A blank
        // note is always rebuilt, which drops the history of an emptied note.
        if selection == self.selection && selection != Selection::Blank {
            self.focus_content(window, cx);
            return;
        }
        self.open(selection, kept, window, cx);
    }

    /// Shows `selection` in a fresh editor.
    fn open(&mut self, selection: Selection, _: Kept, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &selection {
            Selection::Blank => Ok(String::new()),
            Selection::Note(id) => self.store.read(id),
            Selection::Trashed(id) => self.store.read_trashed(id),
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
        // What was looked for belongs to the list it was typed over, and a blank
        // note starts afresh, as when the app opens.
        if selection == Selection::Blank || readonly != self.selection.in_trash() {
            self.close_filter();
        }
        self.loaded = match &selection {
            Selection::Note(id) => Some(self.loaded_now(id, text.clone())),
            _ => None,
        };
        self.selection = selection;
        self.copied = None;
        self.copied_task = None;
        self.copy_menu_open = false;
        self.replace_editor(&text, readonly, window, cx);
        self.reveal_selection(cx);
        self.restart_idle_timer(window, cx);
        cx.notify();
    }

    fn restart_idle_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(delay) = self.config.idle else {
            return;
        };
        self.idle_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay.get()).await;
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
        let pending = self.save_state.kept().is_none();
        if self.selection == Selection::Blank && !pending {
            if self.filter_open {
                self.close_filter();
                self.focus_content(window, cx);
                cx.notify();
            }
            return;
        }
        self.select(Selection::Blank, window, cx);
        // Stays on a note whose save failed, and tries again later.
        if self.save_state.kept().is_none() {
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

    /// `text` as just read from a note's file, with the file's date.
    fn loaded_now(&self, id: &str, text: String) -> Loaded {
        let modified = self
            .store
            .modified(id)
            .unwrap_or_else(|_| SystemTime::now());
        Loaded { text, modified }
    }

    /// Follows the notes folder, so that what another program or a sync client
    /// changes shows as it happens, not only when the window next gets focus.
    /// Without it nothing is lost, since a save never writes over such a change.
    pub fn watch_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders = self.store.folders();
        self.notes_watch = match FolderWatcher::new(&folders, false) {
            Ok((mut watcher, events)) => Some(cx.spawn_in(window, async move |this, cx| {
                while events.recv().await.is_ok() {
                    cx.background_executor().timer(FOLDER_SETTLE).await;
                    while events.try_recv().is_ok() {}
                    // The trash may have been replaced, and is read after it is watched.
                    watcher.arm(&folders);
                    let synced =
                        this.update_in(cx, |this, window, cx| this.sync_with_disk(window, cx));
                    if synced.is_err() {
                        break;
                    }
                }
            })),
            Err(error) => {
                eprintln!("brouyon: can't watch the notes: {error}");
                None
            }
        };
    }

    /// Picks up changes made outside the app, such as a note edited in another
    /// program or on another device.
    fn sync_with_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_lists();
        self.refresh_open_note(window, cx);
        cx.notify();
    }

    /// Brings the open note up to date with its file, in the editor it is in: this
    /// can happen at any moment, and must not take the keyboard from wherever it is.
    /// A note with pending edits is left for its next save, which keeps both.
    fn refresh_open_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kept) = self.save_state.kept() else {
            return;
        };
        match self.selection.clone() {
            Selection::Note(id) => match self.store.read(&id) {
                Ok(text) => {
                    if self
                        .loaded
                        .as_ref()
                        .is_none_or(|loaded| loaded.text != text)
                    {
                        self.show_in_place(&text, kept, window, cx);
                    }
                    // The date alone can change, and is shown.
                    self.loaded = Some(self.loaded_now(&id, text));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.selection = Selection::Blank;
                    self.loaded = None;
                    self.show_in_place("", kept, window, cx);
                }
                // Not to be read just now, which is no reason to let go of it.
                Err(_) => {}
            },
            // A dialog on screen holds the keyboard, and may be about this note.
            Selection::Trashed(id)
                if !self.dialog_is_open() && !self.trash.iter().any(|note| note.id == id) =>
            {
                self.show_next_trashed(kept, window, cx);
            }
            _ => {}
        }
    }

    /// Replaces the editor's text without a new editor, which would take the
    /// keyboard. The cursor goes back to the start and the undo history is dropped,
    /// as the text it belonged to is no longer there.
    fn show_in_place(&mut self, text: &str, _: Kept, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            state.set_value(text.to_owned(), window, cx);
        });
        self.seen = self.editor.read(cx).value();
        self.words = markdown::word_count(text);
        self.copied = None;
        self.copied_task = None;
        self.copy_menu_open = false;
    }

    /// Saves before the window goes. The first attempt to leave after a failed save
    /// is refused so the error can be seen, and a second one is let through. Only a
    /// failed save counts: another problem on screen says nothing about this text.
    fn may_leave(&mut self, cx: &mut Context<Self>) -> Option<MayLeave> {
        let warned = self.save_state.warned();
        let kept = self.flush(cx);
        kept.map(Kept::may_leave)
            .or_else(|| warned.map(Warned::may_leave_anyway))
    }

    /// Quitting skips the window's own close, so it asks the same question.
    fn quit(&mut self, cx: &mut Context<Self>) {
        if let Some(leave) = self.may_leave(cx) {
            leave.quit(cx);
        }
    }

    /// Closes the window from a key or a menu, as its own button would.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(leave) = self.on_close(cx) {
            leave.close(window);
        }
    }

    fn on_close(&mut self, cx: &mut Context<Self>) -> Option<MayLeave> {
        let leave = self.may_leave(cx);
        if leave.is_some()
            && let Some(settings) = self.settings_window
        {
            // The app runs until its last window closes, and Settings can't
            // outlive the window it belongs to.
            settings
                .update(cx, |_, window, _| kept::close_textless(window))
                .ok();
        }
        leave
    }

    /// The rows of the list being shown, top to bottom: the notes or the trash,
    /// narrowed down to what is being looked for. A blank note has a row of its own
    /// above the notes, unless something is being looked for.
    fn rows(&self, cx: &App) -> Vec<Selection> {
        let terms = self.terms(cx);
        let found = |note: &&Note| note.matches(&terms);
        if self.selection.in_trash() {
            let trash = self.trash.iter().filter(found);
            trash
                .map(|note| Selection::Trashed(note.id.clone()))
                .collect()
        } else {
            let blank = self.selection == Selection::Blank && terms.is_empty();
            let notes = self.notes.iter().filter(found);
            blank
                .then_some(Selection::Blank)
                .into_iter()
                .chain(notes.map(|note| Selection::Note(note.id.clone())))
                .collect()
        }
    }

    fn selected_row(&self, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|row| *row == self.selection)
    }

    fn reveal_selection(&self, cx: &App) {
        if let Some(row) = self.selected_row(cx) {
            self.list_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    /// Moves through the list. Going up from the first note reaches a blank one,
    /// and going down from a note that isn't listed reaches the first that is.
    fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.flush(cx).is_none() {
            return;
        }
        let rows = self.rows(cx);
        let target = match (self.selected_row(cx), down) {
            (Some(row), true) => row + 1,
            (None, true) => 0,
            (Some(0), false) => {
                if !self.selection.in_trash() {
                    self.select(Selection::Blank, window, cx);
                }
                return;
            }
            (Some(row), false) => row - 1,
            (None, false) => return,
        };
        if let Some(selection) = rows.get(target) {
            self.select(selection.clone(), window, cx);
        }
    }

    fn move_to_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Text typed moments ago may not have a file yet.
        let before = self.selection.clone();
        let Some(kept) = self.flush(cx) else {
            return;
        };
        let Selection::Note(id) = self.selection.clone() else {
            return;
        };
        // The save found the note changed elsewhere and kept both. Neither is what
        // was on screen when the key was pressed, so neither goes without a look.
        if matches!(before, Selection::Note(_)) && before != self.selection {
            return;
        }
        match self.store.trash(&id) {
            Ok(_) => {
                self.reload_lists();
                self.open(Selection::Blank, kept, window, cx);
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
        if self.selection.in_trash() {
            return;
        }
        let Some(kept) = self.flush(cx) else {
            return;
        };
        self.reload_lists();
        let Some(first) = self.trash.first().map(|note| note.id.clone()) else {
            cx.notify();
            return;
        };
        self.before_trash = match &self.selection {
            Selection::Note(id) => Some(id.clone()),
            _ => None,
        };
        self.open(Selection::Trashed(first), kept, window, cx);
    }

    fn toggle_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.in_trash() {
            self.show_notes(window, cx);
        } else {
            self.show_trash(window, cx);
        }
    }

    /// Once a note has left the trash: the first one still listed, or the notes.
    /// When none of what was looked for is left, the whole trash is listed again.
    fn show_next_trashed(&mut self, kept: Kept, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows(cx).is_empty() {
            self.close_filter();
        }
        match self.rows(cx).into_iter().next() {
            Some(next) => self.open(next, kept, window, cx),
            None => self.show_notes(window, cx),
        }
    }

    fn restore(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kept) = self.flush(cx) else {
            return;
        };
        match self.store.restore(&id) {
            Ok(restored) => {
                self.reload_lists();
                self.open(Selection::Note(restored), kept, window, cx);
            }
            Err(error) => {
                self.problem = Some(format!("Couldn't restore: {error}").into());
                cx.notify();
            }
        }
    }

    /// Puts the open note on the clipboard as it stands, saved or not, and keeps
    /// `format` for the button's next click. A note with nothing in it is left out,
    /// so it can't replace what the clipboard holds.
    fn copy_note(&mut self, format: CopyFormat, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value();
        if text.trim().is_empty() {
            return;
        }
        let text = match format {
            CopyFormat::Markdown => text.to_string(),
            CopyFormat::Text => markdown::plain_text(&text),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.set_copy_as(format, cx);
        self.copied = Some(format);
        self.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_NOTICE).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn ask(&mut self, confirm: Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = Some(confirm);
        self.copy_menu_open = false;
        self.dialog_focus.focus(window, cx);
        cx.notify();
    }

    /// Asks about the note on screen, with the text it shows.
    fn ask_to_delete_forever(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(Confirm::DeleteForever(id, self.seen.clone()), window, cx);
    }

    fn dialog_is_open(&self) -> bool {
        self.confirm.is_some() || self.shortcuts_open
    }

    fn toggle_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcuts_open = !self.shortcuts_open;
        if self.shortcuts_open {
            self.copy_menu_open = false;
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
        if confirmed && let Some(kept) = self.flush(cx) {
            let result = match &confirm {
                Confirm::DeleteForever(id, shown) => self.store.purge(id, shown),
                Confirm::EmptyTrash => self.store.empty_trash().map(|()| true),
            };
            self.reload_lists();
            match result {
                Ok(true) => self.show_next_trashed(kept, window, cx),
                Ok(false) => {
                    self.open(self.selection.clone(), kept, window, cx);
                    self.say("Changed elsewhere, so it wasn't deleted.", cx);
                }
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

    fn set_copy_as(&mut self, format: CopyFormat, cx: &mut Context<Self>) {
        if format != self.config.copy_as {
            let saved = self.config.set_copy_as(format);
            self.report_setting(saved, cx);
        }
    }

    pub fn set_text_percent(&mut self, percent: TextPercent, cx: &mut Context<Self>) {
        let saved = self.config.set_text_percent(percent);
        self.report_setting(saved, cx);
    }

    pub fn set_idle(
        &mut self,
        idle: Option<IdleDelay>,
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
        dir: Option<NotesDir>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = dir
            .clone()
            .unwrap_or_else(|| self.config.default_notes_dir.clone());
        if target == self.config.notes_dir {
            return;
        }
        let Some(kept) = self.flush(cx) else {
            self.settings_problem =
                Some("The open note couldn't be saved, so the folder didn't change.".into());
            cx.notify();
            return;
        };
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
        if self.notes_watch.is_some() {
            self.watch_notes(window, cx);
        }
        // A question about a note in the old folder must not act on the new one,
        // where a note can have the same name.
        self.confirm = None;
        let saved = self.config.set_notes_dir(dir);
        self.reload_lists();
        self.open(Selection::Blank, kept, window, cx);
        self.report_setting(saved, cx);
    }

    fn sidebar(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let in_trash = self.selection.in_trash();
        // Wide enough for a title and its date on one line, unless the editor needs it.
        let narrow = is_narrow(window);
        let rem = window.rem_size();
        let mut width = rems(if narrow { 11.25 } else { 20. }).to_pixels(rem);
        // On macOS the window's buttons take the header's left, which the narrow
        // list needs for its tabs. They replace its padding.
        let buttons = self.title_bar.buttons_width(window);
        if narrow && let Some(buttons) = buttons {
            width += buttons - rems(0.75).to_pixels(rem);
        }
        // A tab's bar reaches past its label, which stays in line with the titles
        // in the list below: the first tab starts that much before them.
        let tabs = Tabs::new("note-tabs")
            .flex()
            .h_full()
            .ml(-rems(TAB_PADDING))
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
            format!("New note ({})", shortcuts::NEW_NOTE),
        );
        // The narrow list has no room for it beside the tabs. The key still works.
        let find = (!narrow).then(|| {
            with_tooltip(
                button("find-note", "", ButtonVariant::Secondary, cx)
                    .accessibility_label("Find a note")
                    .debug_selector(|| "find-note".into())
                    .child(icon(IconName::Search))
                    .on_click(cx.listener(|this, _, window, cx| this.open_filter(window, cx))),
                format!("Find a note ({})", shortcuts::FIND_NOTE),
            )
        });
        // Under the window's buttons, and between the tabs and the header's own, the
        // header moves the window as a title bar would.
        let under_buttons = buttons.map(|buttons| {
            self.title_bar
                .drag_area()
                .flex_shrink_0()
                .w(buttons)
                .h_full()
        });
        let between = buttons.map(|_| self.title_bar.drag_area().flex_1().h_full());
        let header = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(rems(BAR_HEIGHT))
            .pl(rems(0.75))
            .pr(rems(0.125))
            .border_b_1()
            .border_color(t.divider())
            .when(under_buttons.is_some(), |header| header.pl_0())
            .children(under_buttons)
            .child(tabs)
            .children(between)
            .child(div().flex().flex_shrink_0().children(find).child(new_note));

        let filter = self.filter_open.then(|| {
            div()
                .flex_shrink_0()
                .p(rems(0.375))
                .border_b_1()
                .border_color(t.divider())
                .child(
                    input("note-filter", &self.filter, window, cx)
                        .debug_selector(|| "note-filter".into())
                        .prefix(icon(IconName::Search).size(rems(0.875))),
                )
        });

        let row_height = rems(3.25).to_pixels(window.rem_size());
        let rows = Rc::new(self.rows(cx));
        let terms = self.terms(cx);
        let nothing_found = rows.is_empty() && !terms.is_empty();
        if terms != self.listed_for {
            if !terms.is_empty() {
                self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            self.listed_for = terms;
        }
        let count = rows.len();
        let sizes = Rc::new(
            (0..count)
                .map(|row| {
                    let around = row_spacing(row, count);
                    let around = rems(around.0 + around.1).to_pixels(window.rem_size());
                    size(gpui_kit::Pixels::ZERO, row_height + around)
                })
                .collect::<Vec<_>>(),
        );
        let list = virtual_list(
            cx.entity(),
            "note-list",
            sizes,
            move |this, range, window, cx| {
                range
                    .filter_map(|row| {
                        Some(this.render_row(row, count, rows.get(row)?, row_height, window, cx))
                    })
                    .collect::<Vec<_>>()
            },
            cx,
        )
        .track_scroll(&self.list_scroll)
        .size_full()
        // gpui-omarchy gives lists the theme's solid background, which would hide
        // the window's own where it's see-through.
        .bg(transparent_black());
        let list = if nothing_found {
            div()
                .px(rems(0.75))
                .py(rems(1.))
                .text_color(t.secondary)
                .child("Nothing found")
                .into_any_element()
        } else {
            list.into_any_element()
        };

        // What is about the app and the list is under the list. The strip beside
        // it, under the note, only holds what is about that note.
        let settings = with_tooltip(
            bar_button(
                button("settings", "", ButtonVariant::Secondary, cx)
                    .accessibility_label("Settings")
                    .debug_selector(|| "settings".into())
                    .child(icon(IconName::Settings))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
                window,
            ),
            format!("Settings ({})", shortcuts::SETTINGS),
        );
        let empty_trash = in_trash.then(|| {
            bar_button(
                button("empty-trash", "Empty Trash", ButtonVariant::Outline, cx),
                window,
            )
            .debug_selector(|| "empty-trash".into())
            .on_click(cx.listener(|this, _, window, cx| {
                this.ask(Confirm::EmptyTrash, window, cx);
            }))
        });
        let footer = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(rems(BAR_HEIGHT))
            // The gear's own padding sets its icon in line with the titles, except on
            // a Mac, where it keeps off the window's corner.
            .pl(if MAC {
                bar_inset(window)
            } else {
                rems(0.0625).to_pixels(window.rem_size())
            })
            .pr(rems(0.5))
            .border_t_1()
            .border_color(t.divider())
            .child(settings)
            .children(empty_trash);

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(width)
            .h_full()
            .border_r_1()
            .border_color(t.divider())
            .child(header)
            .children(filter)
            .child(
                div()
                    .debug_selector(|| "note-list".into())
                    .flex_1()
                    .min_h_0()
                    .child(list),
            )
            .child(footer)
    }

    fn render_row(
        &self,
        row: usize,
        count: usize,
        selection: &Selection,
        height: gpui_kit::Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let selection = selection.clone();
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
            // While looking for something, the line it was found on says more than
            // the one under the title.
            Some(note) => (
                note.title.clone().into(),
                format_date(note.modified, chrono::Local::now()).into(),
                note.found_line(&self.terms(cx))
                    .unwrap_or_else(|| note.preview.clone())
                    .into(),
            ),
            None => ("New note".into(), "".into(), "".into()),
        };
        let colors = row_colors(&t, selected, macos::is_key_window(window));
        let (above, below) = row_spacing(row, count);
        let row = div()
            .id(("note-row", row))
            .debug_selector(move || format!("note-row-{row}"))
            .flex()
            .flex_col()
            .justify_center()
            .flex_shrink_0()
            .w_full()
            .h(height)
            .px(rems(ROW_PADDING))
            .overflow_hidden()
            .when(ROW_INSET > 0., |row| {
                row.rounded(gpui_kit::base::Theme::global(cx).tokens.radius.md)
            })
            .when_some(colors.fill, |row, fill| row.bg(fill))
            .when(!selected, |row| row.hover(|row| row.bg(t.hover_fill())))
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
                            .text_color(colors.title)
                            .when(selected, |title| title.font_weight(FontWeight::BOLD))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.6875))
                            .text_color(colors.detail)
                            .child(date),
                    ),
            )
            // Empty, it keeps its line, so a title sits at the same height in every row.
            .child(
                div()
                    .debug_selector(|| format!("note-row-{row}-{preview}"))
                    .truncate()
                    .text_size(rems(0.6875))
                    .text_color(colors.detail)
                    .child(preview),
            );
        div()
            .flex_shrink_0()
            .w_full()
            .px(rems(ROW_INSET))
            .pt(rems(above))
            .pb(rems(below))
            .child(row)
    }

    fn editor_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = self.editor.clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(desktop::content_background(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(rems(1.5))
                    .pt(rems(1.25))
                    .text_size(rems(0.875))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        editor.update(cx, |state, cx| state.focus(window, cx));
                    })
                    .child(
                        div()
                            .debug_selector(|| "note-editor".into())
                            .size_full()
                            .child(Editor::new(&self.editor)),
                    )
                    // Above the note, the window's top edge moves it, as a title bar
                    // would on macOS.
                    .when(title_bar::DRAWN, |pane| {
                        pane.relative().child(
                            self.title_bar
                                .drag_area()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(rems(1.25)),
                        )
                    }),
            )
            .child(self.status_strip(window, cx))
    }

    fn status_strip(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = cx.omarchy().clone();
        let problem = self.problem.as_ref().or(self.settings_problem.as_ref());
        let status = match (problem, &self.notice, self.copied, &self.selection) {
            (Some(problem), ..) => div()
                .debug_selector(|| "status-problem".into())
                .text_color(t.danger)
                .child(problem.clone()),
            (None, Some(notice), ..) => div()
                .debug_selector(|| "status-notice".into())
                .child(notice.clone()),
            (None, None, Some(format), _) => div().child(format!("Copied as {}", format.label())),
            (None, None, None, Selection::Note(_)) => match &self.loaded {
                Some(loaded) => div().child(format!(
                    "Saved · {}",
                    format_date(loaded.modified, chrono::Local::now())
                )),
                None => div(),
            },
            (None, None, None, Selection::Trashed(_)) => div().child("In the Trash"),
            _ => div(),
        };
        let actions = div().flex().flex_shrink_0().gap(rems(0.25));
        let actions = match self.selection.clone() {
            Selection::Note(_) => actions.child(with_tooltip(
                bar_button(
                    button("move-to-trash", "", ButtonVariant::Secondary, cx),
                    window,
                )
                .accessibility_label("Move to Trash")
                .debug_selector(|| "move-to-trash".into())
                .child(icon(IconName::Trash))
                .on_click(cx.listener(|this, _, window, cx| this.move_to_trash(window, cx))),
                format!("Move to Trash ({})", shortcuts::MOVE_TO_TRASH),
            )),
            Selection::Trashed(id) => {
                let restore = id.clone();
                actions
                    .child(
                        bar_button(
                            button("restore", "Restore", ButtonVariant::Secondary, cx),
                            window,
                        )
                        .debug_selector(|| "restore".into())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.restore(restore.clone(), window, cx);
                            },
                        )),
                    )
                    .child(
                        bar_button(
                            button(
                                "delete-forever",
                                "Delete forever",
                                ButtonVariant::Danger,
                                cx,
                            ),
                            window,
                        )
                        .debug_selector(|| "delete-forever".into())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.ask_to_delete_forever(id.clone(), window, cx);
                            },
                        )),
                    )
            }
            _ => actions,
        };
        // A narrow window has no room for them beside Restore and Delete forever.
        // The key still works.
        let copy = match self.selection {
            Selection::Blank => false,
            Selection::Note(_) => true,
            Selection::Trashed(_) => !is_narrow(window),
        }
        .then(|| self.copy_buttons(window, cx));
        let copying = copy.is_some();
        let noted = self.selection != Selection::Blank;
        // A narrow window has no room for it. A blank note has none to count.
        let words = (!is_narrow(window) && (noted || self.words > 0)).then(|| {
            div()
                .debug_selector(|| "word-count".into())
                .child(match self.words {
                    1 => "1 word".to_owned(),
                    words => format!("{words} words"),
                })
        });
        // Gone with its button, so that it doesn't come back open.
        self.copy_menu_open &= copying;
        // Copying is at one end and the Trash at the other, so that reaching for
        // the first can't land on the second.
        let left = div()
            .flex()
            .items_center()
            .gap(rems(0.75))
            .min_w_0()
            .children(copy)
            .child(div().truncate().child(status));
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap(rems(0.75))
            .h(rems(BAR_HEIGHT))
            // In line with the note's text: the status, or the copy button's icon,
            // which the button's own padding sets in.
            .pl(rems(if copying { 0.8125 } else { 1.5 }))
            // On a Mac, the button in the corner keeps as far off the side as off the
            // bottom.
            .pr(if MAC {
                bar_inset(window)
            } else {
                rems(0.125).to_pixels(window.rem_size())
            })
            .border_t_1()
            .border_color(t.divider())
            .text_size(rems(0.6875))
            .text_color(t.secondary)
            .child(left)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rems(0.5))
                    // A note just begun has no button there to keep the count
                    // off the edge.
                    .when(!noted, |right| right.pr(rems(0.625)))
                    .children(words)
                    .child(actions),
            )
    }

    /// The button that copies the note in the format last chosen, and beside it
    /// the menu that copies it in either one.
    fn copy_buttons(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let format = self.config.copy_as;
        let pressed = cx.omarchy().pressed_fill();
        let label = format!("Copy as {}", format.label());
        let copy = with_tooltip(
            bar_button(
                button("copy-note", "", ButtonVariant::Secondary, cx),
                window,
            )
            .accessibility_label(label.clone())
            .debug_selector(|| "copy-note".into())
            .child(
                icon(if self.copied.is_some() {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(rems(0.875)),
            )
            .child("Copy")
            .on_click(cx.listener(move |this, _, window, cx| {
                this.copy_note(format, cx);
                this.focus_content(window, cx);
            })),
            format!("{label} ({})", shortcuts::COPY_NOTE),
        );
        let trigger = bar_button(button("copy-as", "", ButtonVariant::Secondary, cx), window)
            .accessibility_label("Choose what to copy")
            .debug_selector(|| "copy-as".into())
            .px(rems(0.125))
            .styles(move |styles| styles.selected(move |style| style.bg(pressed)))
            .child(icon(IconName::ChevronUp).size(rems(0.875)));
        // Its tooltip would come up over the open menu.
        let trigger = if self.copy_menu_open {
            trigger
        } else {
            with_tooltip(trigger, "Copy as…")
        };
        let (chosen, changed) = (cx.entity(), cx.entity());
        let formats = menu(
            "copy-formats",
            trigger,
            CopyFormat::ALL
                .map(|option| {
                    MenuItem::new(format!("Copy as {}", option.label())).checked(option == format)
                })
                .to_vec(),
            move |index, _, cx| {
                chosen.update(cx, |this, cx| this.copy_note(CopyFormat::ALL[index], cx));
            },
        )
        // The strip is at the bottom of the window, so the menu opens above it.
        .anchor(Anchor::BottomLeft)
        // Kept here so that whatever takes the keyboard elsewhere can close the menu.
        .open(self.copy_menu_open)
        .on_open_change(move |open, window, cx| {
            changed.update(cx, |this, cx| {
                this.copy_menu_open = *open;
                // However it closes, typing carries on in the note.
                if !*open {
                    this.focus_content(window, cx);
                }
                cx.notify();
            });
        });
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .child(copy)
            .child(formats)
    }

    fn confirm_dialog(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let confirm = self.confirm.as_ref()?;
        let (title, description, label) = match confirm {
            Confirm::DeleteForever(..) => (
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
                                    .debug_selector(|| "cancel".into())
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(gpui_kit::base::actions::Cancel),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                dialog_button("confirm", label, ButtonVariant::Danger, cx)
                                    .debug_selector(|| "confirm".into())
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

    fn shortcuts_dialog(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.shortcuts_open {
            return None;
        }
        // What a small window can't show at once is scrolled to. The rest of the
        // height is the dialog's title, its padding and a margin around it.
        let room = window.viewport_size().height - rems(7.).to_pixels(window.rem_size());
        let t = cx.omarchy().clone();
        let close = cx.listener(|this, _: &bool, window, cx| this.toggle_shortcuts(window, cx));
        let rows = shortcuts::ALL.map(|(keys, action)| {
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
                    .child(
                        div()
                            .id("shortcut-rows")
                            .flex()
                            .flex_col()
                            .gap(rems(0.375))
                            .max_h(room.max(rems(3.).to_pixels(window.rem_size())))
                            .overflow_y_scroll()
                            .children(rows),
                    ),
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
        self.title_bar
            .place_buttons(rems(BAR_HEIGHT).to_pixels(window.rem_size()), window);
        let t = cx.omarchy().clone();
        let style = markdown::editor_style(&t);
        let paddings = note_paddings(window);
        self.editor.update(cx, |state, _| {
            state.set_editor_style(style);
            state.set_editor_paddings(paddings);
        });
        let sidebar = self.sidebar(window, cx);
        let editor_pane = self.editor_pane(window, cx);
        let dialog = self.confirm_dialog(cx);
        let shortcuts = self.shortcuts_dialog(window, cx);
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
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| this.close(window, cx)))
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
                .on_action(cx.listener(|this, _: &FindNote, window, cx| {
                    this.open_filter(window, cx);
                }))
                .on_action(cx.listener(|this, _: &PreviousNote, window, cx| {
                    this.step(false, window, cx);
                }))
                .on_action(cx.listener(|this, _: &NextNote, window, cx| {
                    this.step(true, window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleTask, window, cx| {
                    // Anywhere but in the note, the key keeps its usual meaning.
                    if this.editor.focus_handle(cx).is_focused(window) {
                        this.edit_list(lists::toggle_task, window, cx);
                    } else {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &CopyNote, _, cx| {
                    this.copy_note(this.config.copy_as, cx);
                }))
                .on_action(cx.listener(|this, _: &MoveToTrash, window, cx| {
                    this.move_to_trash(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleTrash, window, cx| {
                    this.toggle_trash(window, cx);
                }))
                .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                    if this.filter_open {
                        this.close_filter();
                        this.focus_content(window, cx);
                        cx.notify();
                    } else if this.selection.in_trash() {
                        this.show_notes(window, cx);
                    } else {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                    this.open_settings(window, cx);
                }))
                .on_action(cx.listener(|this, _: &IncreaseTextSize, _, cx| {
                    this.set_text_percent(this.config.text_percent.larger(), cx);
                }))
                .on_action(cx.listener(|this, _: &DecreaseTextSize, _, cx| {
                    this.set_text_percent(this.config.text_percent.smaller(), cx);
                }))
                .on_action(cx.listener(|this, _: &ResetTextSize, _, cx| {
                    this.set_text_percent(TextPercent::default(), cx);
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

/// Too narrow for everything, so the list is thinner and buttons with a key to
/// do the same are left out where they'd crowd the rest.
fn is_narrow(window: &Window) -> bool {
    window.viewport_size().width < rems(45.).to_pixels(window.rem_size())
}

/// How far a tab's bar reaches past its label and count on each side, in rems.
/// Two tabs side by side keep twice this between their labels.
const TAB_PADDING: f32 = 0.5;

/// A tab above the list: a small capital label with its count after it, and a
/// bar in the accent under the current one, wider than the two by the same on
/// both sides. The bar lies over the tab's bottom edge and takes no room there,
/// so the label stays in the middle of the header, level with its buttons.
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
        .relative()
        .h_full()
        .px(rems(TAB_PADDING))
        .gap(rems(0.375))
        .text_size(rems(0.6875))
        .font_weight(FontWeight::BOLD)
        .text_color(t.secondary)
        .when(enabled && !selected, |tab| {
            tab.hover(move |tab| tab.text_color(hover))
        })
        .styles(|styles| {
            styles
                .selected(|tab| tab.text_color(bright))
                .disabled(|tab| tab.opacity(0.45))
        })
        .child(
            div()
                .debug_selector(move || format!("{id}-label"))
                .child(label.to_uppercase()),
        )
        .when(count > 0, |tab| {
            tab.child(
                div()
                    .debug_selector(move || format!("{id}-count"))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.secondary)
                    .child(count.to_string()),
            )
        })
        .when(selected, |tab| {
            tab.child(
                div()
                    .debug_selector(move || format!("{id}-underline"))
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(2.))
                    .bg(accent),
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

    pub fn has_problem(&self) -> bool {
        self.problem.is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::desktop::Transparency;
    use chrono::TimeZone;
    use gpui_kit::{
        EntityInputHandler, Focusable, Modifiers, TestAppContext, VisualTestContext, point, px,
    };
    use std::{
        fs::{self, File},
        path::{Path, PathBuf},
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
            macos::bind_editor_keys(cx);
        });
        let store = Store::open(&NotesDir::new(dir.path())).unwrap();
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

    /// The tests press shortcuts with Linux's Ctrl, which is ⌘ on macOS.
    pub(crate) fn on_this_system(keystrokes: &str) -> String {
        if MAC {
            keystrokes.replace("ctrl-", "cmd-")
        } else {
            keystrokes.to_owned()
        }
    }

    pub(crate) fn keys(cx: &mut VisualTestContext, keystrokes: &str) {
        cx.simulate_keystrokes(&on_this_system(keystrokes));
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

    /// Has an input method show `text` in the note, in place of what it showed before,
    /// as it does until a word is settled on.
    fn compose(view: &Entity<Brouyon>, cx: &mut VisualTestContext, text: &str) {
        with_input_method(view, cx, |state, window, cx| {
            state.replace_and_mark_text_in_range(None, text, None, window, cx);
        });
    }

    /// Ends the composition with its text left as it is. A click does this, and so does
    /// choosing a suggestion on an iPhone.
    fn keep_composed(view: &Entity<Brouyon>, cx: &mut VisualTestContext) {
        with_input_method(view, cx, |state, window, cx| state.unmark_text(window, cx));
    }

    fn with_input_method(
        view: &Entity<Brouyon>,
        cx: &mut VisualTestContext,
        act: impl FnOnce(&mut EditorState, &mut Window, &mut Context<EditorState>),
    ) {
        cx.update(|window, cx| {
            let editor = view.read(cx).editor.clone();
            editor.update(cx, |state, cx| act(state, window, cx));
        });
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn composed_text_is_saved_after_a_pause(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        compose(&view, cx, "compos");
        wait(cx, SAVE_DELAY / 2);
        compose(&view, cx, "composed");
        keep_composed(&view, cx);
        wait(cx, SAVE_DELAY / 2);
        assert!(note_texts(dir.path()).is_empty(), "still within the pause");

        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["composed"]);
        assert_eq!(titles(&view, cx), ["composed"]);
    }

    #[gpui_kit::test]
    fn composed_text_is_saved_when_leaving_the_note(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        compose(&view, cx, "composed");
        keep_composed(&view, cx);
        keys(cx, "ctrl-n");
        assert_eq!(editor_text(&view, cx), "");
        assert_eq!(note_texts(dir.path()), ["composed"]);
    }

    #[gpui_kit::test]
    fn composed_text_is_saved_when_the_window_closes(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        compose(&view, cx, "composed");
        keep_composed(&view, cx);
        assert!(cx.simulate_close());
        assert_eq!(note_texts(dir.path()), ["composed"]);
    }

    /// What is left of a word is what is on screen, so that is what is saved.
    #[gpui_kit::test]
    fn a_composition_cut_short_or_taken_back_leaves_what_is_on_screen(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "typed");
        compose(&view, cx, " and");
        keys(cx, "ctrl-n");
        assert_eq!(note_texts(dir.path()), ["typed and"], "left half-way");

        compose(&view, cx, "n");
        wait(cx, SAVE_DELAY);
        compose(&view, cx, "");
        assert_eq!(editor_text(&view, cx), "");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["typed and"], "taken back");
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
        let mono = Theme::tokyo_night().mono_font;
        cx.update(|cx| {
            assert_eq!(cx.omarchy().font, mono);
            assert_eq!(cx.omarchy().mono_font, mono);
        });
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
    fn notes_are_spaced_and_set_in_as_the_look_has_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        for n in 1..=60 {
            fs::write(dir.path().join(format!("{n}.md")), "a note").unwrap();
        }
        let (_dir, _view, cx) = open_with(cx, dir);
        let list = cx.debug_bounds("note-list").unwrap();
        let (first, second) = (
            cx.debug_bounds("note-row-0").unwrap(),
            cx.debug_bounds("note-row-1").unwrap(),
        );
        cx.simulate_event(ScrollEvent {
            position: list.center(),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
            ..Default::default()
        });
        draw(cx);
        let last = cx.debug_bounds("note-row-60").unwrap();

        let spaced = |space: Pixels| match MAC {
            true => space > px(0.),
            false => space.abs() < px(0.5),
        };
        let gap = second.top() - first.bottom();
        let above = first.top() - list.top();
        let below = list.bottom() - last.bottom();
        assert!(spaced(gap), "{gap:?} between two");
        assert!(spaced(first.left()), "{:?} at the side", first.left());
        assert!(spaced(above - gap), "{above:?} above the first");
        assert!(spaced(below - gap), "{below:?} below the last");
    }

    /// A test window has no AppKit window to ask, so GPUI's answer stands.
    #[gpui_kit::test]
    fn without_appkit_the_window_is_in_front_when_gpui_says_so(cx: &mut TestAppContext) {
        let (_dir, _view, cx) = open(cx);
        let in_front =
            |cx: &mut VisualTestContext| cx.update(|window, _| macos::is_key_window(window));
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert!(in_front(cx));
        cx.deactivate_window();
        assert!(!in_front(cx));
    }

    /// The editor's scrollbar runs along its right edge, which is the window's.
    #[gpui_kit::test]
    fn the_note_scrollbar_runs_along_the_window_edge(cx: &mut TestAppContext) {
        let (_dir, _view, cx) = open(cx);
        let width = cx.update(|window, _| window.viewport_size().width);
        let editor = cx.debug_bounds("note-editor").unwrap();
        assert_eq!(editor.right(), width);
    }

    #[test]
    fn the_shown_note_stands_out() {
        let t = Theme::flexoki_light();
        let plain = row_colors(&t, false, true);
        assert!(plain.fill.is_none());
        let shown = row_colors(&t, true, true);
        if MAC {
            assert_eq!(shown.fill, Some(t.accent));
            assert_eq!(shown.title, t.on_accent, "readable on the accent");
            assert_eq!(row_colors(&t, true, false).fill, Some(t.selected_fill()));
        } else {
            assert_eq!(shown.fill, Some(t.hover_fill()));
            assert_eq!(shown.title, t.accent);
        }
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

    /// The editor tells of more than its text, such as its cursor, and of text it was
    /// given to show.
    #[gpui_kit::test]
    fn looking_at_a_note_or_following_its_file_never_rewrites_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let dated = |text: &str, seconds| {
            fs::write(&path, text).unwrap();
            let file = File::options().write(true).open(&path).unwrap();
            let date = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
            file.set_modified(date).unwrap();
            date
        };
        let mut date = dated("one\r\n\ttwo  \n\n", 1);
        let (_dir, view, cx) = open_with(cx, dir);
        let date_once_left = |cx: &mut VisualTestContext| {
            wait(cx, IDLE);
            assert_eq!(selection(&view, cx), Selection::Blank);
            fs::metadata(&path).unwrap().modified().unwrap()
        };
        keys(cx, "alt-down");
        assert_eq!(editor_text(&view, cx), "one\r\n\ttwo  \n\n");
        keys(cx, "right shift-down");
        assert_eq!(date_once_left(cx), date);

        // With text waiting to be saved, the change wouldn't show yet.
        keys(cx, "alt-down");
        for (text, seconds) in [("three\r\n", 2), ("four\r\n", 3)] {
            keys(cx, "right shift-down");
            date = dated(text, seconds);
            notice_changes(&view, cx);
            assert_eq!(editor_text(&view, cx), text);
        }
        keys(cx, "right shift-down");
        assert_eq!(date_once_left(cx), date);
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
        assert!(selection(&view, cx).in_trash());
        assert_eq!(editor_text(&view, cx), "keep me");

        click("restore", cx);
        assert!(matches!(selection(&view, cx), Selection::Note(_)));
        assert_eq!(note_texts(dir.path()), ["keep me"]);
        assert!(trashed_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn the_buttons_in_the_bottom_corners_keep_as_far_from_both_edges(cx: &mut TestAppContext) {
        if !MAC {
            return;
        }
        let (_dir, _view, cx) = open(cx);
        type_and_save(cx, "keep me");
        let window = cx.update(|window, _| window.viewport_size());
        let settings = cx.debug_bounds("settings").unwrap();
        let trash = cx.debug_bounds("move-to-trash").unwrap();
        for (bottom, side) in [
            (window.height - settings.bottom(), settings.left()),
            (window.height - trash.bottom(), window.width - trash.right()),
        ] {
            assert!(
                (bottom - side).abs() < px(0.5),
                "{bottom:?} below, {side:?} beside"
            );
            assert!(
                side < px(macos::WINDOW_CORNER),
                "within the window's corner"
            );
        }
    }

    #[gpui_kit::test]
    fn the_trash_button_is_at_the_other_end_of_the_strip_from_copy(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "keep me");
        let copy = cx.debug_bounds("copy-as").unwrap();
        let trash = cx.debug_bounds("move-to-trash").unwrap();
        let strip = trash.right() - copy.left();
        assert!(
            trash.left() - copy.right() > strip / 2.,
            "{copy:?} and {trash:?} are too close"
        );

        click("move-to-trash", cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(trashed_texts(dir.path()), ["keep me"]);
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
    fn a_tab_label_is_level_with_the_header_buttons_in_the_middle_of_its_bar(
        cx: &mut TestAppContext,
    ) {
        let (_dir, _view, cx) = open(cx);
        type_and_save(cx, "keep me");
        let tab = cx.debug_bounds("notes-tab").unwrap();
        let label = cx.debug_bounds("notes-tab-label").unwrap();
        let button = cx.debug_bounds("new-note").unwrap();
        assert!(
            (label.center().y - button.center().y).abs() < px(0.5),
            "{label:?} isn't level with {button:?}"
        );

        let count = cx.debug_bounds("notes-tab-count").unwrap();
        let underline = cx.debug_bounds("notes-tab-underline").unwrap();
        assert_eq!(
            (underline.left(), underline.right(), underline.bottom()),
            (tab.left(), tab.right(), tab.bottom())
        );
        let (before, after) = (
            label.left() - underline.left(),
            underline.right() - count.right(),
        );
        assert!(
            before > px(4.),
            "the bar starts {before:?} before the label"
        );
        assert_eq!(before, after, "the bar is as long on both sides");

        // The label starts where the titles under it do.
        if !MAC {
            let rem = cx.update(|window, _| window.rem_size());
            let row = cx.debug_bounds("note-row-0").unwrap();
            assert_eq!(label.left(), row.left() + rems(ROW_PADDING).to_pixels(rem));
        }
        assert!(cx.debug_bounds("trash-tab-underline").is_none());
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
        click("confirm", cx);
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

    fn question_is_open(cx: &mut VisualTestContext) -> bool {
        cx.debug_bounds("confirm").is_some()
    }

    #[gpui_kit::test]
    fn deleting_forever_asks_first_and_cancel_keeps_the_note(cx: &mut TestAppContext) {
        let (dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        assert!(question_is_open(cx));

        // Escape answers the dialog rather than also leaving the trash.
        keys(cx, "escape");
        assert!(!question_is_open(cx));
        assert!(selection(&view, cx).in_trash());

        click("delete-forever", cx);
        click("cancel", cx);
        assert!(!question_is_open(cx));
        assert!(selection(&view, cx).in_trash());
        assert_eq!(trashed_texts(dir.path()), ["trashed"]);
    }

    #[gpui_kit::test]
    fn app_shortcuts_are_ignored_while_a_question_is_open(cx: &mut TestAppContext) {
        let (dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        keys(cx, "ctrl-n");
        keys(cx, "ctrl-shift-t");
        assert!(selection(&view, cx).in_trash());
        assert!(question_is_open(cx));
        assert_eq!(trashed_texts(dir.path()), ["trashed"]);
    }

    #[gpui_kit::test]
    fn confirming_delete_forever_removes_the_file(cx: &mut TestAppContext) {
        let (dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        click("confirm", cx);
        assert!(trashed_texts(dir.path()).is_empty());
        assert_eq!(selection(&view, cx), Selection::Blank, "back on the notes");
    }

    #[gpui_kit::test]
    fn empty_trash_removes_only_trashed_notes(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        type_and_save(cx, "kept");
        keys(cx, "ctrl-n");
        type_and_save(cx, "binned");
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        click("empty-trash", cx);
        click("confirm", cx);
        assert!(trashed_texts(dir.path()).is_empty());
        assert_eq!(note_texts(dir.path()), ["kept"]);
    }

    #[gpui_kit::test]
    fn the_idle_move_waits_for_an_open_question(cx: &mut TestAppContext) {
        let (_dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        wait(cx, IDLE * 2);
        assert!(selection(&view, cx).in_trash());
        assert!(question_is_open(cx));
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
        let (_dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        keys(cx, "f1");
        assert!(!shortcuts_open(&view, cx));
        assert!(question_is_open(cx));
    }

    #[gpui_kit::test]
    fn both_redo_keys_bring_back_what_undo_took(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("words");
        // Linux also has Ctrl+Y. macOS only has ⇧⌘Z.
        let redo_keys: &[&str] = if MAC {
            &["ctrl-shift-z"]
        } else {
            &["ctrl-shift-z", "ctrl-y"]
        };
        for &redo in redo_keys {
            keys(cx, "ctrl-z");
            assert_eq!(editor_text(&view, cx), "", "before {redo}");
            keys(cx, redo);
            assert_eq!(editor_text(&view, cx), "words", "{redo}");
        }
    }

    #[gpui_kit::test]
    fn enter_starts_the_next_item_of_a_list(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        cx.simulate_input("- milk");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- milk\n- ");
        cx.simulate_input("eggs");
        assert_eq!(editor_text(&view, cx), "- milk\n- eggs");
        wait(cx, SAVE_DELAY);
        assert_eq!(
            note_texts(dir.path()),
            ["- milk\n- eggs"],
            "saved like anything typed"
        );
    }

    #[gpui_kit::test]
    fn an_empty_nested_item_goes_back_a_level_then_ends_the_list(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("- a");
        keys(cx, "shift-enter");
        cx.simulate_input("  - b");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- a\n  - b\n  - ");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- a\n  - b\n- ");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- a\n  - b\n");

        // An item added between two levels belongs to the one above it.
        keys(cx, "up up end enter");
        cx.simulate_input("c");
        assert_eq!(editor_text(&view, cx), "- a\n- c\n  - b\n");
    }

    #[gpui_kit::test]
    fn numbers_and_tasks_carry_on_and_an_empty_item_ends_the_list(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("1. first");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "1. first\n2. ");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "1. first\n", "the list ended");

        cx.simulate_input("- [x] done");
        keys(cx, "enter");
        cx.simulate_input("next");
        assert_eq!(editor_text(&view, cx), "1. first\n- [x] done\n- [ ] next");
    }

    #[gpui_kit::test]
    fn shift_enter_and_plain_text_get_a_plain_new_line(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("- milk");
        keys(cx, "shift-enter");
        assert_eq!(editor_text(&view, cx), "- milk\n");
        cx.simulate_input("words");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- milk\nwords\n");
    }

    #[gpui_kit::test]
    fn undo_takes_back_the_marker_and_then_the_line(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("- milk");
        keys(cx, "enter");
        keys(cx, "ctrl-z");
        assert_eq!(editor_text(&view, cx), "- milk\n");
        keys(cx, "ctrl-z");
        assert_eq!(editor_text(&view, cx), "- milk");
    }

    #[gpui_kit::test]
    fn ctrl_enter_ticks_a_task_where_the_cursor_is_and_saves_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        cx.simulate_input("- [ ] call the plumber");
        keys(cx, "ctrl-enter");
        assert_eq!(editor_text(&view, cx), "- [x] call the plumber");
        cx.simulate_input("!");
        assert_eq!(
            editor_text(&view, cx),
            "- [x] call the plumber!",
            "the cursor stayed where it was"
        );
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["- [x] call the plumber!"]);

        keys(cx, "ctrl-enter");
        assert_eq!(editor_text(&view, cx), "- [ ] call the plumber!");
        keys(cx, "enter");
        cx.simulate_input("buy milk");
        keys(cx, "up ctrl-enter");
        assert_eq!(
            editor_text(&view, cx),
            "- [x] call the plumber!\n- [ ] buy milk"
        );
    }

    #[gpui_kit::test]
    fn ctrl_enter_makes_a_plain_line_a_task(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.simulate_input("call the plumber");
        keys(cx, "ctrl-enter");
        assert_eq!(editor_text(&view, cx), "- [ ] call the plumber");
    }

    #[gpui_kit::test]
    fn a_trashed_note_is_not_ticked(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "- [ ] gone");
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        keys(cx, "ctrl-enter");
        keys(cx, "enter");
        assert_eq!(editor_text(&view, cx), "- [ ] gone");
        assert_eq!(trashed_texts(dir.path()), ["- [ ] gone"]);
    }

    /// A folder of notes to look through, and one in the trash.
    fn open_with_notes_to_find(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Brouyon>, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        for (name, secs, text) in [
            ("a.md", 400, "# Plumber\n\nCall about the kitchen Tap"),
            ("b.md", 300, "Groceries\n- milk\n- tap water"),
            ("c.md", 200, "Book list\nfrom the club"),
            (".trash/d.md", 100, "Old plumber quote"),
            (".trash/e.md", 50, "Old shopping list"),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, text).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        }
        open_with(cx, dir)
    }

    /// The titles of the rows in the list, as shown.
    fn listed(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|_, cx| {
            let this = view.read(cx);
            let title = |notes: &[Note], id: &NoteId| {
                let note = notes.iter().find(|note| note.id == *id).unwrap();
                note.title.clone()
            };
            this.rows(cx)
                .iter()
                .map(|row| match row {
                    Selection::Blank => "New note".to_string(),
                    Selection::Note(id) => title(&this.notes, id),
                    Selection::Trashed(id) => title(&this.trash, id),
                })
                .collect()
        })
    }

    fn filter_is_open(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| view.read(cx).filter_open) && cx.debug_bounds("note-filter").is_some()
    }

    fn editor_is_focused(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| view.read(cx).editor.focus_handle(cx).is_focused(window))
    }

    fn find(cx: &mut VisualTestContext, words: &str) {
        keys(cx, "ctrl-f");
        cx.simulate_input(words);
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn leaving_find_shows_the_note_gone_back_to(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".trash")).unwrap();
        fs::write(dir.path().join(".trash/z.md"), "trashed").unwrap();
        for n in 0..60 {
            fs::write(dir.path().join(format!("{n:02}.md")), format!("note {n}")).unwrap();
        }
        let (_dir, _view, cx) = open_with(cx, dir);
        assert!(cx.debug_bounds("note-row-59").is_none());
        keys(cx, &["alt-down"; 60].join(" "));
        keys(cx, "ctrl-shift-t");
        find(cx, "trashed");
        keys(cx, "ctrl-shift-t");
        draw(cx);
        let last = cx.debug_bounds("note-row-59");
        assert!(last.is_some(), "the note gone back to is out of sight");
    }

    #[gpui_kit::test]
    fn finding_follows_what_an_input_method_composes(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        keys(cx, "ctrl-f");
        cx.update(|window, cx| {
            let filter = view.read(cx).filter.clone();
            filter.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "tap", None, window, cx);
                state.unmark_text(window, cx);
            });
        });
        cx.run_until_parked();
        draw(cx);
        assert_eq!(listed(&view, cx), ["Plumber", "Groceries"]);
        keys(cx, "enter");
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));
    }

    #[gpui_kit::test]
    fn finding_narrows_the_list_to_notes_holding_every_word(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        assert_eq!(
            listed(&view, cx),
            ["New note", "Plumber", "Groceries", "Book list"]
        );
        assert!(!filter_is_open(&view, cx));

        find(cx, "tap");
        assert!(filter_is_open(&view, cx));
        assert_eq!(listed(&view, cx), ["Plumber", "Groceries"], "any case");
        assert_eq!(
            editor_text(&view, cx),
            "",
            "typed in the field, not the note"
        );

        cx.simulate_input(" KITCHEN");
        cx.run_until_parked();
        assert_eq!(listed(&view, cx), ["Plumber"], "every word");

        cx.simulate_input(" nowhere");
        cx.run_until_parked();
        draw(cx);
        assert!(listed(&view, cx).is_empty());
    }

    #[gpui_kit::test]
    fn enter_opens_the_first_note_found_and_escape_lists_them_all_again(cx: &mut TestAppContext) {
        let (dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "water");
        keys(cx, "enter");
        assert_eq!(selection(&view, cx), Selection::Note("b.md".into()));
        assert!(editor_is_focused(&view, cx), "ready to edit it");
        assert_eq!(listed(&view, cx), ["Groceries"], "still narrowed down");

        keys(cx, "escape");
        assert!(!filter_is_open(&view, cx));
        assert_eq!(listed(&view, cx), ["Plumber", "Groceries", "Book list"]);
        assert_eq!(selection(&view, cx), Selection::Note("b.md".into()));
        assert!(editor_is_focused(&view, cx));
        assert_eq!(note_texts(dir.path()).len(), 3, "nothing was written");
    }

    #[gpui_kit::test]
    fn escape_in_the_field_stops_finding_and_goes_back_to_the_note(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "tap");
        keys(cx, "escape");
        assert!(!filter_is_open(&view, cx));
        assert_eq!(
            listed(&view, cx),
            ["New note", "Plumber", "Groceries", "Book list"]
        );
        cx.simulate_input("typed in the note");
        assert_eq!(editor_text(&view, cx), "typed in the note");

        // The field comes back empty.
        keys(cx, "ctrl-f");
        assert_eq!(listed(&view, cx).len(), 4);
    }

    #[gpui_kit::test]
    fn the_arrows_walk_through_what_was_found(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "tap");
        keys(cx, "alt-down");
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));
        keys(cx, "alt-down");
        assert_eq!(selection(&view, cx), Selection::Note("b.md".into()));
        keys(cx, "alt-down");
        assert_eq!(
            selection(&view, cx),
            Selection::Note("b.md".into()),
            "the last one found"
        );
        keys(cx, "alt-up");
        assert_eq!(selection(&view, cx), Selection::Note("a.md".into()));
    }

    #[gpui_kit::test]
    fn the_key_again_selects_the_words_to_type_over(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "tap");
        keys(cx, "enter");
        find(cx, "club");
        assert_eq!(listed(&view, cx), ["Book list"]);
    }

    #[gpui_kit::test]
    fn the_button_opens_the_field_too(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        click("find-note", cx);
        assert!(filter_is_open(&view, cx));
        cx.simulate_input("club");
        cx.run_until_parked();
        assert_eq!(listed(&view, cx), ["Book list"]);
    }

    #[gpui_kit::test]
    fn a_new_note_lists_everything_again(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "tap");
        keys(cx, "enter");
        keys(cx, "ctrl-n");
        assert!(!filter_is_open(&view, cx));
        assert_eq!(listed(&view, cx).len(), 4);

        // So does a note begun on the blank page while looking for another.
        find(cx, "tap");
        click_editor(cx);
        type_and_save(cx, "A new thought");
        assert!(!filter_is_open(&view, cx));
        assert_eq!(
            listed(&view, cx),
            ["A new thought", "Plumber", "Groceries", "Book list"]
        );
    }

    fn click_editor(cx: &mut VisualTestContext) {
        cx.simulate_click(point(px(700.), px(300.)), Modifiers::default());
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui_kit::test]
    fn the_idle_move_stops_finding_too(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "tap");
        wait(cx, IDLE);
        assert!(!filter_is_open(&view, cx));
        assert!(editor_is_focused(&view, cx));

        find(cx, "tap");
        keys(cx, "enter");
        wait(cx, IDLE);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert!(!filter_is_open(&view, cx));
    }

    #[gpui_kit::test]
    fn each_tab_is_looked_through_on_its_own(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_with_notes_to_find(cx);
        find(cx, "plumber");
        assert_eq!(listed(&view, cx), ["Plumber"]);

        click("trash-tab", cx);
        assert!(!filter_is_open(&view, cx), "the words were for the notes");
        assert_eq!(
            listed(&view, cx),
            ["Old plumber quote", "Old shopping list"]
        );

        find(cx, "shopping");
        assert_eq!(listed(&view, cx), ["Old shopping list"]);
        keys(cx, "enter");
        assert_eq!(selection(&view, cx), Selection::Trashed("e.md".into()));

        keys(cx, "escape");
        assert!(
            selection(&view, cx).in_trash(),
            "the first Esc stops finding"
        );
        assert_eq!(listed(&view, cx).len(), 2);
        keys(cx, "escape");
        assert!(
            !selection(&view, cx).in_trash(),
            "the second leaves the Trash"
        );
    }

    #[gpui_kit::test]
    fn deleting_the_last_note_found_in_the_trash_lists_the_rest_of_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = open_with_notes_to_find(cx);
        click("trash-tab", cx);
        find(cx, "shopping");
        keys(cx, "enter");
        click("delete-forever", cx);
        click("confirm", cx);

        assert_eq!(trashed_texts(dir.path()), ["Old plumber quote"]);
        assert_eq!(selection(&view, cx), Selection::Trashed("d.md".into()));
        assert!(!filter_is_open(&view, cx));
    }

    #[gpui_kit::test]
    fn a_found_note_shows_the_line_it_was_found_on(cx: &mut TestAppContext) {
        let (_dir, _view, cx) = open_with_notes_to_find(cx);
        assert!(cx.debug_bounds("note-row-2-milk").is_some());
        find(cx, "water");
        assert!(cx.debug_bounds("note-row-0-tap water").is_some());
    }

    #[gpui_kit::test]
    fn finding_waits_for_an_open_question(cx: &mut TestAppContext) {
        let (_dir, view, cx, _) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        keys(cx, "ctrl-f");
        assert!(!filter_is_open(&view, cx));
    }

    const MARKED_UP: &str = "# Title\n\nSome **bold** words\n";
    const NOT_MARKED_UP: &str = "Title\n\nSome bold words\n";

    /// Opens on a note written in Markdown.
    fn open_marked_up(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Brouyon>, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), MARKED_UP).unwrap();
        let (dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        (dir, view, cx)
    }

    fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    fn copy_format(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> CopyFormat {
        cx.update(|_, cx| view.read(cx).config.copy_as)
    }

    #[gpui_kit::test]
    fn the_copy_button_copies_the_note_as_it_is_written(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        click("copy-note", cx);
        assert_eq!(clipboard(cx).as_deref(), Some(MARKED_UP));
        assert!(editor_is_focused(&view, cx), "typing carries on");
    }

    #[gpui_kit::test]
    fn the_menu_copies_as_text_and_the_button_does_the_same_from_then_on(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        click("copy-as", cx);
        click("omarchy-menu-item-1", cx);
        assert_eq!(clipboard(cx).as_deref(), Some(NOT_MARKED_UP));
        assert!(cx.debug_bounds("omarchy-menu-content").is_none());
        assert!(editor_is_focused(&view, cx), "typing carries on");
        assert_eq!(copy_format(&view, cx), CopyFormat::Text);
        cx.update(|_, cx| {
            let saved = view.read(cx).config.saved_text();
            assert!(saved.contains("copy_as = \"text\""), "{saved}");
        });

        cx.write_to_clipboard(ClipboardItem::new_string("something else".into()));
        click("copy-note", cx);
        assert_eq!(clipboard(cx).as_deref(), Some(NOT_MARKED_UP));

        click("copy-as", cx);
        click("omarchy-menu-item-0", cx);
        assert_eq!(clipboard(cx).as_deref(), Some(MARKED_UP));
        assert_eq!(copy_format(&view, cx), CopyFormat::Markdown);
    }

    #[gpui_kit::test]
    fn the_key_copies_text_typed_moments_ago(cx: &mut TestAppContext) {
        let (dir, _view, cx) = open(cx);
        cx.simulate_input("*not* saved yet");
        keys(cx, "ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("*not* saved yet"));
        assert!(note_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn a_blank_note_has_nothing_to_copy(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open(cx);
        cx.write_to_clipboard(ClipboardItem::new_string("kept".into()));
        assert!(cx.debug_bounds("copy-note").is_none());
        cx.simulate_input("  ");
        keys(cx, "ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("kept"));
        cx.update(|_, cx| assert_eq!(view.read(cx).copied, None));
    }

    #[gpui_kit::test]
    fn a_narrow_window_has_the_copy_buttons_except_in_the_trash(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        cx.simulate_resize(size(px(480.), px(420.)));
        draw(cx);
        let window = cx.update(|window, _| window.viewport_size().width);
        let fits = |selector, cx: &mut VisualTestContext| {
            cx.debug_bounds(selector)
                .is_some_and(|bounds| bounds.right() <= window)
        };
        assert!(fits("copy-note", cx) && fits("copy-as", cx));
        assert!(fits("move-to-trash", cx) && fits("settings", cx));

        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        assert!(selection(&view, cx).in_trash());
        assert!(cx.debug_bounds("copy-note").is_none());
        assert!(cx.debug_bounds("copy-as").is_none());
        assert!(fits("settings", cx));
        keys(cx, "ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some(MARKED_UP));
    }

    #[gpui_kit::test]
    fn a_trashed_note_can_be_copied_and_escape_closes_the_menu_first(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        assert!(selection(&view, cx).in_trash());

        click("copy-as", cx);
        assert!(cx.debug_bounds("omarchy-menu-content").is_some());
        keys(cx, "escape");
        assert!(cx.debug_bounds("omarchy-menu-content").is_none());
        assert!(selection(&view, cx).in_trash(), "still in the Trash");
        assert_eq!(clipboard(cx), None);
        assert!(
            editor_is_focused(&view, cx),
            "the keyboard is back in the note"
        );

        click("copy-note", cx);
        assert_eq!(clipboard(cx).as_deref(), Some(MARKED_UP));
    }

    #[gpui_kit::test]
    fn the_menu_closes_when_the_keyboard_goes_elsewhere(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "first").unwrap();
        fs::write(dir.path().join("b.md"), "second").unwrap();
        let (_dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        let menu_is_open =
            |cx: &mut VisualTestContext| cx.debug_bounds("omarchy-menu-content").is_some();

        click("copy-as", cx);
        assert!(menu_is_open(cx));
        keys(cx, "alt-down");
        assert!(!menu_is_open(cx), "another note");
        assert!(editor_is_focused(&view, cx));

        click("copy-as", cx);
        keys(cx, "f1");
        assert!(!menu_is_open(cx), "the shortcuts");
        assert!(shortcuts_open(&view, cx));
        keys(cx, "f1");

        click("copy-as", cx);
        click_editor(cx);
        assert!(!menu_is_open(cx), "a click outside it");
        assert!(editor_is_focused(&view, cx));
        assert_eq!(clipboard(cx), None);
    }

    #[gpui_kit::test]
    fn the_strip_counts_the_words_as_they_are_typed(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        let words = |cx: &mut VisualTestContext| cx.update(|_, cx| view.read(cx).words);
        let shown = |cx: &mut VisualTestContext| {
            draw(cx);
            cx.debug_bounds("word-count").is_some()
        };
        assert_eq!(words(cx), 4, "without the # and the **");
        assert!(shown(cx));

        keys(cx, "ctrl-n");
        assert_eq!(words(cx), 0);
        assert!(!shown(cx), "a blank note has none to count");
        cx.simulate_input("- [ ] three more words");
        assert_eq!(words(cx), 3, "before it is saved");
        compose(&view, cx, " and one");
        assert_eq!(words(cx), 5, "before it is settled on");
        compose(&view, cx, "");
        assert_eq!(words(cx), 3);
        assert!(shown(cx));
        let count = cx.debug_bounds("word-count").unwrap();
        let window = cx.update(|window, _| window.viewport_size().width);
        assert!(count.right() < window - px(8.), "off the edge: {count:?}");
        keys(cx, "ctrl-z");
        assert_eq!(words(cx), 0);

        keys(cx, "alt-down");
        assert_eq!(words(cx), 4);
        cx.simulate_resize(size(px(480.), px(420.)));
        assert!(!shown(cx), "no room in a narrow window");
    }

    #[gpui_kit::test]
    fn the_strip_says_what_was_copied_for_a_moment(cx: &mut TestAppContext) {
        let (_dir, view, cx) = open_marked_up(cx);
        let copied = |cx: &mut VisualTestContext| cx.update(|_, cx| view.read(cx).copied);
        keys(cx, "ctrl-shift-c");
        assert_eq!(copied(cx), Some(CopyFormat::Markdown));
        wait(cx, COPIED_NOTICE / 2);
        keys(cx, "ctrl-shift-c");
        wait(cx, COPIED_NOTICE / 2);
        assert_eq!(
            copied(cx),
            Some(CopyFormat::Markdown),
            "counted from the last copy"
        );
        wait(cx, COPIED_NOTICE / 2);
        assert_eq!(copied(cx), None);

        keys(cx, "ctrl-shift-c");
        keys(cx, "ctrl-n");
        assert_eq!(copied(cx), None, "another note wasn't copied");
    }

    #[gpui_kit::test]
    fn copying_waits_for_an_open_question(cx: &mut TestAppContext) {
        let (_dir, _view, cx) = open_marked_up(cx);
        keys(cx, "ctrl-shift-backspace");
        keys(cx, "ctrl-shift-t");
        click("empty-trash", cx);
        keys(cx, "ctrl-shift-c");
        assert_eq!(clipboard(cx), None);

        click("cancel", cx);
        keys(cx, "ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some(MARKED_UP));
    }

    fn set_desktop(cx: &mut VisualTestContext, change: impl FnOnce(&mut Desktop)) {
        cx.update(|_, cx| change(cx.global_mut::<Desktop>()));
        cx.run_until_parked();
        draw(cx);
    }

    /// On a Mac the root is the sidebar, tinted over the blur, and the note is solid.
    #[gpui_kit::test]
    fn only_the_sidebar_shows_through_on_a_mac(cx: &mut TestAppContext) {
        if !MAC {
            return;
        }
        let (_dir, _view, cx) = open(cx);
        cx.update(|window, cx| {
            let root = desktop::root_background(window, cx);
            assert_eq!(root.a, macos::sidebar_opacity(cx.omarchy()));
            assert!(root.a < 1., "the sidebar shows through");
            assert_eq!(desktop::content_background(cx).a, 1.);
            assert_eq!(cx.omarchy().background.a, 1., "dialogs paint this one");
        });
    }

    #[gpui_kit::test]
    fn only_the_root_background_follows_the_transparency_mode(cx: &mut TestAppContext) {
        if MAC {
            return;
        }
        let (_dir, _view, cx) = open(cx);
        let alpha = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| desktop::root_background(window, cx).a)
        };
        assert_eq!(alpha(cx), 1., "solid until told otherwise");

        set_desktop(cx, |desktop| {
            desktop.transparency = desktop::Transparency::Glassy;
        });
        assert_eq!(alpha(cx), 0.65 * 0.96);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert_eq!(alpha(cx), 0.65 * 0.985, "more solid in front");
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
    fn mac_corners_survive_every_theme(cx: &mut TestAppContext) {
        if !MAC {
            return;
        }
        let (_dir, view, cx) = open(cx);
        let (small, control) = macos::CORNERS;
        for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Omarchy] {
            view.update(cx, |this, cx| this.set_theme(mode, cx));
            cx.update(|_, cx| {
                let radius = gpui_kit::base::Theme::global(cx).tokens.radius;
                assert_eq!(radius.sm, px(small), "{mode:?}");
                assert_eq!(radius.md, px(control), "{mode:?}");
                assert!(radius.full >= px(9999.), "{mode:?}: switches are capsules");
            });
        }
    }

    #[gpui_kit::test]
    fn every_theme_takes_the_system_accent_on_a_mac(cx: &mut TestAppContext) {
        if !MAC {
            return;
        }
        let (_dir, view, cx) = open(cx);
        let Some(accent) = macos::system_accent() else {
            return;
        };
        for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Omarchy] {
            view.update(cx, |this, cx| this.set_theme(mode, cx));
            cx.update(|_, cx| assert_eq!(cx.omarchy().accent, accent, "{mode:?}"));
        }
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
    fn quitting_shows_a_failed_save_before_it_lets_go(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        fs::remove_dir_all(dir.path()).unwrap();
        cx.simulate_input("precious");
        keys(cx, "ctrl-q");
        cx.update(|_, cx| {
            let problem = view.read(cx).problem.clone().expect("the save was tried");
            assert!(problem.contains("Couldn't save"), "{problem}");
        });
        assert!(is_said("status-problem", cx));
    }

    #[gpui_kit::test]
    fn closing_shows_a_failed_save_even_after_another_problem(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        type_and_save(cx, "first");
        fs::remove_dir_all(dir.path()).unwrap();
        keys(cx, "ctrl-shift-backspace");
        cx.update(|_, cx| {
            let problem = view.read(cx).problem.clone().unwrap();
            assert!(problem.contains("Couldn't move to Trash"), "{problem}");
        });
        assert!(is_said("status-problem", cx));

        cx.simulate_input(" and more");
        assert!(!cx.simulate_close(), "this failure hasn't been seen yet");
        draw(cx);
        assert!(is_said("status-problem", cx));
        assert!(cx.simulate_close(), "a second attempt closes anyway");
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

    /// What the watcher does when something in the notes folder changes.
    fn notice_changes(view: &Entity<Brouyon>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            view.update(cx, |this, cx| this.sync_with_disk(window, cx));
        });
        cx.run_until_parked();
        draw(cx);
    }

    /// Types a note, saves it, and returns where its file is.
    fn saved_note(
        dir: &Path,
        view: &Entity<Brouyon>,
        cx: &mut VisualTestContext,
        text: &str,
    ) -> PathBuf {
        type_and_save(cx, text);
        let Selection::Note(id) = selection(view, cx) else {
            panic!("expected a saved note");
        };
        dir.join(id)
    }

    /// Whether the strip under the note has something to read there.
    pub(crate) fn is_said(selector: &'static str, cx: &mut VisualTestContext) -> bool {
        cx.debug_bounds(selector)
            .is_some_and(|said| said.size.width > px(0.))
    }

    fn notice(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> Option<SharedString> {
        let notice = cx.update(|_, cx| view.read(cx).notice.clone());
        assert_eq!(is_said("status-notice", cx), notice.is_some());
        notice
    }

    #[gpui_kit::test]
    fn delete_forever_leaves_a_note_that_changed_since_it_was_shown(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        fs::write(dir.path().join(".trash").join(&id), "rewritten elsewhere").unwrap();
        notice_changes(&view, cx);
        let delete = |cx: &mut VisualTestContext| {
            click("delete-forever", cx);
            click("confirm", cx);
        };
        delete(cx);
        assert_eq!(trashed_texts(dir.path()), ["rewritten elsewhere"]);
        assert_eq!(editor_text(&view, cx), "rewritten elsewhere");
        assert!(notice(&view, cx).unwrap().contains("wasn't deleted"));

        delete(cx);
        assert!(trashed_texts(dir.path()).is_empty(), "now it was shown");
    }

    #[gpui_kit::test]
    fn composed_text_is_kept_when_its_note_changes_elsewhere(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        compose(&view, cx, " composed");
        keep_composed(&view, cx);
        fs::write(&path, "theirs").unwrap();
        notice_changes(&view, cx);
        assert_eq!(editor_text(&view, cx), "mine composed");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["mine composed", "theirs"]);
    }

    fn cursor(view: &Entity<Brouyon>, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| view.read(cx).editor.read(cx).cursor())
    }

    #[gpui_kit::test]
    fn a_note_changed_elsewhere_while_typing_is_kept_beside_the_typed_one(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        cx.simulate_input(" and more");
        fs::write(&path, "theirs").unwrap();

        // Unsaved typing is never replaced by what arrives.
        notice_changes(&view, cx);
        assert_eq!(editor_text(&view, cx), "mine and more");
        assert_eq!(note_texts(dir.path()), ["theirs"]);

        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["mine and more", "theirs"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert_eq!(titles(&view, cx), ["mine and more", "theirs"]);
        assert!(notice(&view, cx).unwrap().contains("new note"));

        // The typing carries on in the new note, where it was.
        assert_eq!(editor_text(&view, cx), "mine and more");
        assert_eq!(cursor(&view, cx), "mine and more".len());
        type_and_save(cx, "!");
        assert_eq!(note_texts(dir.path()), ["mine and more!", "theirs"]);
        keys(cx, "ctrl-z");
        assert_ne!(
            editor_text(&view, cx),
            "mine and more!",
            "the history came along"
        );

        wait(cx, CHANGE_NOTICE);
        assert_eq!(notice(&view, cx), None);
    }

    #[gpui_kit::test]
    fn typing_into_a_note_left_open_never_overwrites_a_change_made_elsewhere(
        cx: &mut TestAppContext,
    ) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        // Nothing looked at the folder since: the editor still shows the old text.
        fs::write(&path, "theirs").unwrap();
        type_and_save(cx, " too");
        assert_eq!(note_texts(dir.path()), ["mine too", "theirs"]);
    }

    #[gpui_kit::test]
    fn the_apps_own_save_keeps_the_editor_and_its_history(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        saved_note(dir.path(), &view, cx, "first");
        notice_changes(&view, cx);

        cx.simulate_input(" second");
        assert_eq!(editor_text(&view, cx), "first second", "the cursor stayed");
        keys(cx, "ctrl-z");
        keys(cx, "ctrl-z");
        assert_eq!(editor_text(&view, cx), "", "and so did the history");
        assert_eq!(notice(&view, cx), None);
    }

    #[gpui_kit::test]
    fn a_note_only_touched_elsewhere_keeps_its_editor_and_shows_the_new_date(
        cx: &mut TestAppContext,
    ) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "first");
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        notice_changes(&view, cx);
        cx.update(|_, cx| {
            let loaded = view.read(cx).loaded.as_ref().unwrap();
            assert_eq!(loaded.modified, SystemTime::UNIX_EPOCH);
        });

        type_and_save(cx, "!");
        assert_eq!(note_texts(dir.path()), ["first!"], "the cursor stayed");
    }

    #[gpui_kit::test]
    fn a_note_changed_elsewhere_shows_as_it_changes(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        fs::write(&path, "theirs").unwrap();
        notice_changes(&view, cx);
        assert_eq!(editor_text(&view, cx), "theirs");
        assert_eq!(titles(&view, cx), ["theirs"]);

        // What was read is what the next save has to find.
        type_and_save(cx, "ours, ");
        assert_eq!(note_texts(dir.path()), ["ours, theirs"]);
        assert_eq!(notice(&view, cx), None);
    }

    #[gpui_kit::test]
    fn typing_taken_back_takes_the_change_made_elsewhere(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        cx.simulate_input("!");
        fs::write(&path, "theirs").unwrap();
        keys(cx, "backspace");
        assert_eq!(editor_text(&view, cx), "mine");

        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["theirs"], "nothing new to keep");
        assert_eq!(editor_text(&view, cx), "theirs");
    }

    #[gpui_kit::test]
    fn a_note_deleted_elsewhere_while_typing_comes_back(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        fs::remove_file(&path).unwrap();
        type_and_save(cx, " still");
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine still");
        assert_eq!(note_texts(dir.path()), ["mine still"]);
    }

    #[gpui_kit::test]
    fn emptying_a_note_changed_elsewhere_keeps_the_file(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        fs::write(&path, "theirs").unwrap();

        keys(cx, "ctrl-a backspace");
        wait(cx, SAVE_DELAY);
        assert_eq!(note_texts(dir.path()), ["theirs"]);
        assert_eq!(selection(&view, cx), Selection::Blank);
        assert_eq!(titles(&view, cx), ["theirs"]);
        assert!(notice(&view, cx).unwrap().contains("wasn't removed"));
    }

    #[gpui_kit::test]
    fn a_change_elsewhere_leaves_the_keyboard_in_find(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        find(cx, "th");
        fs::write(&path, "theirs").unwrap();
        notice_changes(&view, cx);
        assert_eq!(editor_text(&view, cx), "theirs");

        cx.simulate_input("eir");
        cx.run_until_parked();
        assert!(filter_is_open(&view, cx));
        assert!(!editor_is_focused(&view, cx));
        assert_eq!(editor_text(&view, cx), "theirs", "the words went to Find");

        // The same when the open note is gone, which shows a blank one.
        fs::remove_file(&path).unwrap();
        notice_changes(&view, cx);
        assert_eq!(selection(&view, cx), Selection::Blank);
        cx.simulate_input("s");
        cx.run_until_parked();
        assert!(filter_is_open(&view, cx));
        assert_eq!(editor_text(&view, cx), "");
        assert!(note_texts(dir.path()).is_empty());
    }

    #[gpui_kit::test]
    fn moving_to_trash_stops_when_the_note_changed_elsewhere(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        fs::write(&path, "theirs").unwrap();
        cx.simulate_input(" too");

        keys(cx, "ctrl-shift-backspace");
        assert_eq!(note_texts(dir.path()), ["mine too", "theirs"]);
        assert!(trashed_texts(dir.path()).is_empty());
        assert_eq!(editor_text(&view, cx), "mine too");

        // Asked again, it is the note on screen that goes.
        keys(cx, "ctrl-shift-backspace");
        assert_eq!(note_texts(dir.path()), ["theirs"]);
        assert_eq!(trashed_texts(dir.path()), ["mine too"]);
    }

    #[gpui_kit::test]
    fn closing_keeps_both_versions(cx: &mut TestAppContext) {
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        fs::write(&path, "theirs").unwrap();
        cx.simulate_input(" too");
        assert!(cx.simulate_close());
        assert_eq!(note_texts(dir.path()), ["mine too", "theirs"]);
    }

    #[gpui_kit::test]
    fn a_note_with_windows_line_endings_is_saved_in_its_own_file(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        fs::write(&path, "one\r\ntwo").unwrap();
        let (dir, view, cx) = open_with(cx, dir);
        keys(cx, "alt-down");
        let shown = editor_text(&view, cx);

        // Whatever the editor makes of them, looking again changes nothing.
        cx.simulate_input("!");
        keys(cx, "backspace");
        notice_changes(&view, cx);
        wait(cx, SAVE_DELAY);
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\r\ntwo");
        assert_eq!(editor_text(&view, cx), shown);

        type_and_save(cx, "zero ");
        assert_eq!(note_texts(dir.path()).len(), 1, "no second note");
        assert!(fs::read_to_string(&path).unwrap().starts_with("zero "));
    }

    #[gpui_kit::test]
    fn a_trashed_note_deleted_elsewhere_waits_for_an_open_question(cx: &mut TestAppContext) {
        let (dir, view, cx, id) = with_one_trashed_note(cx);
        click("delete-forever", cx);
        fs::remove_file(dir.path().join(".trash").join(&id)).unwrap();
        notice_changes(&view, cx);
        assert_eq!(selection(&view, cx), Selection::Trashed(id));
        assert!(question_is_open(cx));
    }

    /// The one test here with real file events. They arrive on their own thread, so
    /// it lets the scheduler be woken from outside and waits in real time.
    #[gpui_kit::test]
    fn the_watcher_shows_a_change_while_the_window_stays_in_front(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (dir, view, cx) = open(cx);
        let path = saved_note(dir.path(), &view, cx, "mine");
        cx.update(|window, cx| view.update(cx, |this, cx| this.watch_notes(window, cx)));
        cx.run_until_parked();

        // As a sync client does: the file arrives beside the notes and moves in.
        let arriving = dir.path().join(".trash/arriving");
        fs::write(&arriving, "theirs").unwrap();
        fs::rename(&arriving, &path).unwrap();

        let until_listed = |cx: &mut VisualTestContext, expected: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while titles(&view, cx) != [expected] {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{expected} never showed"
                );
                std::thread::sleep(Duration::from_millis(10));
                wait(cx, FOLDER_SETTLE);
            }
        };
        until_listed(cx, "theirs");
        assert_eq!(editor_text(&view, cx), "theirs");

        // The app's own saves go by the watcher too, and change nothing.
        type_and_save(cx, "ours, ");
        std::thread::sleep(Duration::from_millis(200));
        wait(cx, FOLDER_SETTLE);
        assert_eq!(editor_text(&view, cx), "ours, theirs");
        assert_eq!(note_texts(dir.path()), ["ours, theirs"]);
        assert_eq!(cursor(&view, cx), "ours, ".len());

        // Another folder is followed from the moment it is chosen.
        let other = tempfile::tempdir().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.set_notes_dir(Some(NotesDir::new(other.path())), window, cx)
            });
        });
        fs::write(other.path().join("there.md"), "there").unwrap();
        until_listed(cx, "there");
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
        let store = Store::open(&NotesDir::new(dir.path())).unwrap();
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
            Arc::new(crate::assets::Assets),
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
            .update(&mut cx, |this, _, cx| {
                this.copy_menu_open = true;
                cx.notify();
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("02-copy-menu.png"));
        window
            .update(&mut cx, |this, _, cx| {
                this.copy_menu_open = false;
                this.copy_note(CopyFormat::Text, cx);
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("02-copied.png"));
        window
            .update(&mut cx, |this, _, cx| {
                this.copied = None;
                this.set_copy_as(CopyFormat::Markdown, cx);
            })
            .unwrap();

        let look_for =
            |this: &mut Brouyon, words: &str, window: &mut Window, cx: &mut Context<Brouyon>| {
                this.open_filter(window, cx);
                this.filter
                    .update(cx, |state, cx| state.set_value(words, window, cx));
            };
        window
            .update(&mut cx, |this, window, cx| {
                look_for(this, "pour", window, cx)
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("02-finding.png"));
        window
            .update(&mut cx, |this, window, cx| {
                look_for(this, "introuvable", window, cx)
            })
            .unwrap();
        capture(&mut cx, any_window, &out.join("02-nothing-found.png"));
        window
            .update(&mut cx, |this, window, cx| {
                this.close_filter();
                this.focus_content(window, cx);
                cx.notify();
            })
            .unwrap();

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
                this.set_text_percent(TextPercent::nearest(200), cx);
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
                let store = Store::open(&NotesDir::new(dir.path())).unwrap();
                cx.new(|cx| Brouyon::new(store, narrow_config, window, cx))
            })
            .unwrap();
        let narrow: AnyWindowHandle = narrow_window.into();
        window
            .update(&mut cx, |this, _, cx| {
                this.set_text_percent(TextPercent::nearest(100), cx)
            })
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow.png"));
        narrow_window
            .update(&mut cx, |this, window, cx| {
                look_for(this, "pour", window, cx)
            })
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow-finding.png"));
        narrow_window
            .update(&mut cx, |this, window, cx| this.toggle_trash(window, cx))
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow-trash.png"));
        narrow_window
            .update(&mut cx, |this, window, cx| {
                this.show_notes(window, cx);
                this.select(Selection::Note("2026-11-06 09-40-00.md".into()), window, cx);
            })
            .unwrap();
        capture(&mut cx, narrow, &out.join("14-narrow-note.png"));
        narrow_window
            .update(&mut cx, |this, window, cx| {
                this.select(Selection::Blank, window, cx);
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
        let readme_store = Store::open(&NotesDir::new(readme_dir.path())).unwrap();
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
