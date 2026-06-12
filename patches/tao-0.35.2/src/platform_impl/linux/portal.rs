#[cfg(feature = "dbus")]
use dbus::{
    arg::Variant,
    blocking::{Connection, SyncConnection},
    message::MatchRule,
    Error,
};
use log::warn;
use std::{thread, time::Duration};

use crate::window::Theme;

pub fn theme() -> Result<Theme, Error> {
    let conn = Connection::new_session()?;
    let proxy = conn.with_proxy(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        Duration::from_secs(5),
    );

    let result: (Variant<Variant<u32>>,) = proxy.method_call(
        "org.freedesktop.portal.Settings",
        "Read",
        ("org.freedesktop.appearance", "color-scheme"),
    )?;

    Ok(color_scheme_to_theme(result.0 .0 .0))
}

pub fn receive_theme_changed(theme_tx: crossbeam_channel::Sender<Theme>) -> Result<(), Error> {
    let conn = SyncConnection::new_session()?;
    let match_rule = MatchRule::new_signal("org.freedesktop.portal.Settings", "SettingChanged");

    conn.add_match(match_rule, move |_: (), _, msg| {
        let mut iter = msg.iter_init();
        if let (Ok("org.freedesktop.appearance"), Ok("color-scheme"), Ok(value)) = (
            iter.read::<&str>(),         // Namespace
            iter.read::<&str>(),         // Key
            iter.read::<Variant<u32>>(), // Value
        ) {
            if let Err(e) = theme_tx.send(color_scheme_to_theme(value.0)) {
                warn!("Failed to send theme change request: {}", e);
            }
        }
        true
    })?;

    thread::spawn(move || loop {
        if let Err(e) = conn.process(Duration::from_secs(5)) {
            warn!("D-Bus message processing error: {}", e);
            break;
        }
    });

    Ok(())
}

fn color_scheme_to_theme(color_scheme: u32) -> Theme {
    match color_scheme {
        1 => Theme::Dark,  // Prefer Dark
        2 => Theme::Light, // Prefer Light
        _ => Theme::Light, // No Preference, default to Light
    }
}
