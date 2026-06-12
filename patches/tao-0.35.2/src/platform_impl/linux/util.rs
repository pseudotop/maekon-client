use crate::{
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition},
    error::ExternalError,
    window::WindowSizeConstraints,
};
use gtk::{
    gdk::{
        prelude::{DeviceExt, DisplayExt, SeatExt},
        Display,
    },
    glib::{self},
    prelude::{GtkWindowExt, WidgetExt},
};
use std::{cell::RefCell, rc::Rc};

#[inline]
pub fn cursor_position(is_wayland: bool) -> Result<PhysicalPosition<f64>, ExternalError> {
    if is_wayland {
        Ok((0, 0).into())
    } else {
        Display::default()
            .map(|d| d.default_seat().and_then(|s| s.pointer()))
            .map(|p| {
                p.map(|p| {
                    let (_, x, y) = p.surface_at_position();
                    LogicalPosition::new(x, y).to_physical(1.0)
                })
            })
            .map(|p| p.ok_or(ExternalError::Os(os_error!(super::OsError))))
            .ok_or(ExternalError::Os(os_error!(super::OsError)))?
    }
}

pub fn set_size_constraints<W: GtkWindowExt + WidgetExt>(
    window: &W,
    constraints: WindowSizeConstraints,
) {
    let scale_factor = window.scale_factor() as f64;
    let min_size: LogicalSize<i32> = constraints.min_size_logical(scale_factor);
    let min_width = constraints
        .has_min()
        .then_some(min_size.width)
        .unwrap_or(-1);
    let min_height = constraints
        .has_min()
        .then_some(min_size.height)
        .unwrap_or(-1);

    // GTK4 removed geometry hints; keep the enforceable minimum request here.
    window.set_size_request(min_width, min_height);
}

pub struct WindowMaximizeProcess<W: GtkWindowExt + WidgetExt> {
    window: W,
    resizable: bool,
    step: u8,
}

impl<W: GtkWindowExt + WidgetExt> WindowMaximizeProcess<W> {
    pub fn new(window: W, resizable: bool) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            window,
            resizable,
            step: 0,
        }))
    }

    pub fn next_step(&mut self) -> glib::ControlFlow {
        match self.step {
            0 => {
                self.window.set_resizable(true);
                self.step += 1;
                glib::ControlFlow::Continue
            }
            1 => {
                self.window.maximize();
                self.step += 1;
                glib::ControlFlow::Continue
            }
            2 => {
                self.window.set_resizable(self.resizable);
                glib::ControlFlow::Break
            }
            _ => glib::ControlFlow::Break,
        }
    }
}
