tray-icon lets you create tray icons for desktop applications.

## Maekon E31 patch note

This local patch keeps Windows and macOS tray support available for Tauri while removing the GTK3/libappindicator dependency path from the Maekon Linux graph. Do not enable a GTK tray backend for Linux until a GTK4/WebKitGTK 6 compatible replacement is selected.

## Platforms supported:

- Windows
- macOS
- Linux (disabled in the Maekon E31 patch; KSNI with the `ksni` feature)
- FreeBSD (disabled in the Maekon E31 patch; KSNI with the `ksni` feature)

## Platform-specific notes:

- On Windows and the Linux/FreeBSD AppIndicator backend, an event loop must be running on the thread. The KSNI backend manages its own worker thread.
- On macOS, an event loop must be running on the main thread so you also need to create the tray icon on the main thread.

### Cargo Features

- `ksni`: Uses the StatusNotifierItem D-Bus backend on Linux and BSD.
- `serde`: Enables de/serializing derives.
- `muda-common-controls-v6`: Forwards muda's `common-controls-v6` feature.
- `muda-gtk4`: Forwards muda's `gtk4` feature.
- `muda-libxdo`: Forwards muda's `libxdo` feature. This is enabled by default.
- `muda-serde`: Forwards muda's `serde` feature. The `serde` feature also enables it.
- `muda-snapshot`: Forwards muda's `snapshot` feature. The `ksni` feature also enables it.

The Maekon E31 patch removes the `libappindicator` and `muda-gtk3` features. Without `ksni`,
Linux and BSD use a disabled backend.

## Dependencies (Linux/BSD)

The default Linux backend uses GTK 3, `libxdo`, and `libappindicator` or
`libayatana-appindicator`. The `ksni` backend does not require these system libraries unless a
muda GTK backend is also enabled.

#### Arch Linux / Manjaro:

```sh
pacman -S gtk3 xdotool libappindicator-gtk3 #or libayatana-appindicator
```

#### Debian / Ubuntu:

```sh
sudo apt install libgtk-3-dev libxdo-dev libappindicator3-dev #or libayatana-appindicator3-dev
```

## Dependencies in FreeBSD

Install this dependencies in order to compile `tray-icon`. Instructions using `pkg`:

```sh
pkg install -y rust glib pkgconf gtk3
```

## Examples

#### Create a tray icon without a menu.

```rs
use tray_icon::TrayIconBuilder;

let tray_icon = TrayIconBuilder::new()
    .with_tooltip("system-tray - tray icon library!")
    .with_icon(icon)
    .build()
    .unwrap();
```

#### Create a tray icon with a menu.

```rs
use tray_icon::{TrayIconBuilder, menu::Menu};

let tray_menu = Menu::new();
let tray_icon = TrayIconBuilder::new()
    .with_menu(Box::new(tray_menu))
    .with_tooltip("system-tray - tray icon library!")
    .with_icon(icon)
    .build()
    .unwrap();
```

## Processing tray events

You can use `TrayIconEvent::receiver` to get a reference to the `TrayIconEventReceiver`
which you can use to listen to events when a click happens on the tray icon

```rs
use tray_icon::TrayIconEvent;

if let Ok(event) = TrayIconEvent::receiver().try_recv() {
    println!("{:?}", event);
}
```

You can also listen for the menu events using `MenuEvent::receiver` to get events for the tray context menu.

```rs
use tray_icon::{TrayIconEvent, menu::{MenuEvent}};

if let Ok(event) = TrayIconEvent::receiver().try_recv() {
    println!("tray event: {:?}", event);
}

if let Ok(event) = MenuEvent::receiver().try_recv() {
    println!("menu event: {:?}", event);
}
```

### Note for [winit] or [tao] users:

You should use [`TrayIconEvent::set_event_handler`] and forward
the tray icon events to the event loop by using [`EventLoopProxy`]
so that the event loop is awakened on each tray icon event.
Same can be done for menu events using [`MenuEvent::set_event_handler`].

```rust
enum UserEvent {
  TrayIconEvent(tray_icon::TrayIconEvent)
  MenuEvent(tray_icon::menu::MenuEvent)
}

let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();

let proxy = event_loop.create_proxy();
tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
    proxy.send_event(UserEvent::TrayIconEvent(event));
}));

let proxy = event_loop.create_proxy();
tray_icon::menu::MenuEvent::set_event_handler(Some(move |event| {
    proxy.send_event(UserEvent::MenuEvent(event));
}));
```

[`EventLoopProxy`]: https://docs.rs/winit/latest/winit/event_loop/struct.EventLoopProxy.html
[winit]: https://docs.rs/winit
[tao]: https://docs.rs/tao

## License

Apache-2.0/MIT
