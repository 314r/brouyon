use crate::{look::MAC, macos};
use gpui_kit::{
    Div, MouseButton, Pixels, Point, TitlebarOptions, Window, div, point, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

pub const DRAWN: bool = MAC;

pub const BAR_HEIGHT: f32 = 2.75;

const BUTTONS_WIDTH: f32 = 3. * 14. + 2. * 6.;
const BUTTONS_HEIGHT: f32 = 14.;
const BUTTONS_LEFT: f32 = 16.;
const AFTER_BUTTONS: f32 = 24.;
const DEFAULT_REM: f32 = 16.;

pub fn options() -> Option<TitlebarOptions> {
    DRAWN.then(|| TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(buttons_at(px(BAR_HEIGHT * DEFAULT_REM))),
    })
}

fn buttons_at(bar_height: Pixels) -> Point<Pixels> {
    point(
        px(BUTTONS_LEFT),
        ((bar_height - px(BUTTONS_HEIGHT)) / 2.).round(),
    )
}

#[derive(Default)]
pub struct TitleBar {
    placed: Option<Point<Pixels>>,
    pressed: Rc<Cell<bool>>,
}

impl TitleBar {
    pub fn place_buttons(&mut self, bar_height: Pixels, window: &Window) {
        let wanted = buttons_at(bar_height);
        if self.placed != Some(wanted) {
            self.placed = Some(wanted);
            macos::place_buttons(wanted, window);
        }
    }

    pub fn buttons_width(&self, window: &Window) -> Option<Pixels> {
        (DRAWN && !window.is_fullscreen()).then(|| px(BUTTONS_LEFT + BUTTONS_WIDTH + AFTER_BUTTONS))
    }

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

    pub fn drag_area(&self) -> Div {
        self.drags(div())
    }
}
