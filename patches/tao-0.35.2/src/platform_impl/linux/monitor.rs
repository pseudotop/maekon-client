// Copyright 2014-2021 The winit contributors
// Copyright 2021-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;

use gtk::{
    gdk::{
        self,
        prelude::{Cast, DisplayExt, MonitorExt},
        Display,
    },
    gio::prelude::ListModelExt,
};

use crate::{
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize},
    monitor::{MonitorHandle as RootMonitorHandle, VideoMode as RootVideoMode},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonitorHandle {
    pub(crate) monitor: gdk::Monitor,
}

impl MonitorHandle {
    pub fn new(display: &gdk::Display, number: u32) -> Self {
        let monitor = monitor(display, number).unwrap();
        Self { monitor }
    }

    #[inline]
    pub fn name(&self) -> Option<String> {
        self.monitor.model().map(|s| s.as_str().to_string())
    }

    #[inline]
    pub fn size(&self) -> PhysicalSize<u32> {
        let rect = self.monitor.geometry();
        LogicalSize {
            width: rect.width() as u32,
            height: rect.height() as u32,
        }
        .to_physical(self.scale_factor())
    }

    #[inline]
    pub fn position(&self) -> PhysicalPosition<i32> {
        let rect = self.monitor.geometry();
        LogicalPosition {
            x: rect.x(),
            y: rect.y(),
        }
        .to_physical(self.scale_factor())
    }

    #[inline]
    pub fn scale_factor(&self) -> f64 {
        self.monitor.scale_factor() as f64
    }

    #[inline]
    pub fn video_modes(&self) -> Box<dyn Iterator<Item = RootVideoMode>> {
        Box::new(Vec::new().into_iter())
    }
}

unsafe impl Send for MonitorHandle {}
unsafe impl Sync for MonitorHandle {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VideoMode;

impl VideoMode {
    #[inline]
    pub fn size(&self) -> PhysicalSize<u32> {
        panic!("VideoMode is unsupported on Linux.")
    }

    #[inline]
    pub fn bit_depth(&self) -> u16 {
        panic!("VideoMode is unsupported on Linux.")
    }

    #[inline]
    pub fn refresh_rate(&self) -> u16 {
        panic!("VideoMode is unsupported on Linux.")
    }

    #[inline]
    pub fn monitor(&self) -> RootMonitorHandle {
        panic!("VideoMode is unsupported on Linux.")
    }
}

pub fn monitor(display: &Display, number: u32) -> Option<gdk::Monitor> {
    display
        .monitors()
        .item(number)
        .and_then(|item| item.downcast::<gdk::Monitor>().ok())
}

pub fn available(display: &Display) -> VecDeque<MonitorHandle> {
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitor(display, i).map(|monitor| MonitorHandle { monitor }))
        .collect()
}

pub fn primary(display: &Display) -> Option<MonitorHandle> {
    monitor(display, 0).map(|monitor| MonitorHandle { monitor })
}

pub fn from_point(display: &Display, x: f64, y: f64) -> Option<MonitorHandle> {
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitor(display, i))
        .find(|monitor| {
            let rect = monitor.geometry();
            let x = x as i32;
            let y = y as i32;
            x >= rect.x()
                && x < rect.x() + rect.width()
                && y >= rect.y()
                && y < rect.y() + rect.height()
        })
        .map(|monitor| MonitorHandle { monitor })
}
