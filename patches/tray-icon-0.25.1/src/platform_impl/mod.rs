// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod platform;
#[cfg(all(
    any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ),
    not(feature = "ksni")
))]
// Maekon E31 patch note: the GTK3/libappindicator backend is removed from this crate.
// Without `ksni`, Linux and BSD use a disabled backend whose constructor returns an
// unsupported-platform error.
#[path = "disabled.rs"]
mod platform;
#[cfg(all(
    any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ),
    feature = "ksni"
))]
#[path = "ksni/mod.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod platform;

pub(crate) use self::platform::*;
