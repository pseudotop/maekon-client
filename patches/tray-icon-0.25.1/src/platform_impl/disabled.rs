// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{Icon, TrayIconAttributes, TrayIconId};

pub(crate) type PlatformIcon = crate::icon::NoIcon;

pub struct TrayIcon;

fn disabled_error() -> crate::Error {
    crate::Error::OsError(io::Error::new(
        io::ErrorKind::Unsupported,
        "Maekon E31 disables the GTK3/libappindicator tray backend on Linux and BSD targets",
    ))
}

impl TrayIcon {
    pub fn new(id: TrayIconId, attrs: TrayIconAttributes) -> crate::Result<Self> {
        let _ = (id, attrs);
        Err(disabled_error())
    }

    pub fn set_icon(&mut self, icon: Option<Icon>) -> crate::Result<()> {
        let _ = icon;
        Err(disabled_error())
    }

    pub fn set_menu(&mut self, menu: Option<Box<dyn crate::menu::ContextMenu>>) {
        let _ = menu;
    }

    pub fn set_tooltip<S: AsRef<str>>(&mut self, tooltip: Option<S>) -> crate::Result<()> {
        let _ = tooltip;
        Err(disabled_error())
    }

    pub fn set_title<S: AsRef<str>>(&mut self, title: Option<S>) {
        let _ = title;
    }

    pub fn set_visible(&mut self, visible: bool) -> crate::Result<()> {
        let _ = visible;
        Err(disabled_error())
    }

    pub fn set_temp_dir_path<P: AsRef<Path>>(&mut self, path: Option<P>) {
        let _ = path.map(|p| PathBuf::from(p.as_ref()));
    }

    pub fn rect(&self) -> Option<crate::Rect> {
        None
    }
}
