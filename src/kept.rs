use gpui_kit::{App, Window};

#[derive(Default)]
pub struct SaveState(State);

#[derive(Clone, Copy, Default, PartialEq)]
enum State {
    #[default]
    Kept,
    Pending,
    Refused,
}

/// The text is in its file, or there is none to keep.
#[must_use]
pub struct Kept(());

/// The text couldn't be saved, and the window had already said so.
#[must_use]
pub struct Warned(());

impl SaveState {
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

#[expect(clippy::disallowed_methods, reason = "there is no text")]
pub fn quit_without_windows(cx: &App) {
    if cx.windows().is_empty() {
        cx.quit();
    }
}

#[expect(clippy::disallowed_methods, reason = "there is no text")]
pub fn close_textless(window: &mut Window) {
    window.remove_window();
}
