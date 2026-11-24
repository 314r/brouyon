//! On macOS, Brouyon's windows draw their own title bar: the system's close, minimise
//! and zoom buttons sit in the corner of the bar along the window's top, and the bar's
//! empty parts move the window. On Linux, Hyprland places and moves windows, so none
//! of this does anything there.

use crate::{look::MAC, macos};
use gpui_kit::{
    Div, MouseButton, Pixels, Point, TitlebarOptions, Window, div, point, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

/// Whether the app draws the title bar, rather than the desktop or no one.
pub const DRAWN: bool = MAC;

/// The height of the bars along the windows' top, and the main window's bottom, in
/// rems.
pub const BAR_HEIGHT: f32 = 2.75;

/// The system's buttons, at their usual size and spacing, and where they start.
const BUTTONS_WIDTH: f32 = 3. * 14. + 2. * 6.;
const BUTTONS_HEIGHT: f32 = 14.;
const BUTTONS_LEFT: f32 = 16.;
/// Space between the last button and what follows it in the bar: more than the
/// buttons keep from the window's edge, so the first tab reads as apart from them.
const AFTER_BUTTONS: f32 = 24.;
/// A rem at the default text size, which places the buttons until the first frame
/// knows the real one.
const DEFAULT_REM: f32 = 16.;

/// A transparent title bar with the buttons over the app's own, on macOS. None on
/// Linux, where Hyprland decorates windows.
pub fn options() -> Option<TitlebarOptions> {
    DRAWN.then(|| TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(buttons_at(px(BAR_HEIGHT * DEFAULT_REM))),
    })
}

/// Where the buttons go to sit in the middle of a bar this tall.
fn buttons_at(bar_height: Pixels) -> Point<Pixels> {
    point(
        px(BUTTONS_LEFT),
        ((bar_height - px(BUTTONS_HEIGHT)) / 2.).round(),
    )
}

/// A window's bar along its top, and what it takes to keep it working as a title bar.
#[derive(Default)]
pub struct TitleBar {
    /// Where the buttons were last put.
    placed: Option<Point<Pixels>>,
    /// The left button went down on an empty part of the bar, and the pointer
    /// hasn't moved since.
    pressed: Rc<Cell<bool>>,
}

impl TitleBar {
    /// Keeps the buttons in the middle of a bar this tall, which grows and shrinks
    /// with the text size.
    pub fn place_buttons(&mut self, bar_height: Pixels, window: &Window) {
        let wanted = buttons_at(bar_height);
        if self.placed != Some(wanted) {
            self.placed = Some(wanted);
            macos::place_buttons(wanted, window);
        }
    }

    /// How much of the bar's left the buttons take, with room after them. None where
    /// they aren't over it: on Linux, and in full screen, which hides them.
    pub fn buttons_width(&self, window: &Window) -> Option<Pixels> {
        (DRAWN && !window.is_fullscreen()).then(|| px(BUTTONS_LEFT + BUTTONS_WIDTH + AFTER_BUTTONS))
    }

    /// Makes `area` move the window when dragged, and zoom or minimise it on a double
    /// click, as the system settings say. Only for parts of the bar with no controls in
    /// them: pressing one of those would start a move too.
    pub fn drags(&self, area: Div) -> Div {
        let (pressed, moved, released, elsewhere) = (
            self.pressed.clone(),
            self.pressed.clone(),
            self.pressed.clone(),
            self.pressed.clone(),
        );
        area.on_mouse_down(MouseButton::Left, move |event, window, _| {
            if event.click_count == 2 {
                pressed.set(false);
                window.titlebar_double_click();
            } else {
                pressed.set(true);
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| released.set(false))
        .on_mouse_down_out(move |_, _, _| elsewhere.set(false))
        // Moving, rather than pressing, starts it, as gpui-component's title bar does,
        // so that a plain click leaves the pointer's state alone.
        .on_mouse_move(move |_, window, _| {
            if moved.replace(false) {
                window.start_window_move();
            }
        })
    }

    /// An empty stretch of the bar that moves the window.
    pub fn drag_area(&self) -> Div {
        self.drags(div())
    }
}
