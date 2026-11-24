//! What it takes to let go of the text in the editor. Whatever replaces the text, or
//! leaves it, takes a value from here that only a save gives.

use gpui_kit::{App, Window};

/// What became of the text in the editor since it was last in step with its file.
#[derive(Default)]
pub struct SaveState(State);

#[derive(Clone, Copy, Default, PartialEq)]
enum State {
    #[default]
    Kept,
    Pending,
    /// A save failed, and the window says so.
    Refused,
}

/// The text is in its file, or there is none to keep.
#[must_use]
pub struct Kept(());

/// The text couldn't be saved, and the window had already said so.
#[must_use]
pub struct Warned(());

impl SaveState {
    /// A text that couldn't be saved hasn't been since, whatever was typed.
    pub fn changed(&mut self) {
        if self.0 == State::Kept {
            self.0 = State::Pending;
        }
    }

    pub fn saved(&mut self) -> Kept {
        self.0 = State::Kept;
        Kept(())
    }

    pub fn refused(&mut self) {
        self.0 = State::Refused;
    }

    pub fn kept(&self) -> Option<Kept> {
        (self.0 == State::Kept).then_some(Kept(()))
    }

    pub fn warned(&self) -> Option<Warned> {
        (self.0 == State::Refused).then_some(Warned(()))
    }
}

/// The app, or the window that holds the text, may go.
#[must_use]
pub struct MayLeave(());

impl Kept {
    pub fn may_leave(self) -> MayLeave {
        MayLeave(())
    }
}

impl Warned {
    pub fn may_leave_anyway(self) -> MayLeave {
        MayLeave(())
    }
}

impl MayLeave {
    #[expect(clippy::disallowed_methods, reason = "the text may go")]
    pub fn quit(self, cx: &App) {
        cx.quit();
    }

    #[expect(clippy::disallowed_methods, reason = "the text may go")]
    pub fn close(self, window: &mut Window) {
        window.remove_window();
    }
}

/// Quits once no window is left to hold a text.
#[expect(clippy::disallowed_methods, reason = "there is no text")]
pub fn quit_without_windows(cx: &App) {
    if cx.windows().is_empty() {
        cx.quit();
    }
}

/// Closes a window that never holds a text.
#[expect(clippy::disallowed_methods, reason = "there is no text")]
pub fn close_textless(window: &mut Window) {
    window.remove_window();
}
