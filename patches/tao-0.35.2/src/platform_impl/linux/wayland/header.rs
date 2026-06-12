use gtk::{prelude::*, ApplicationWindow, HeaderBar, Label};

pub struct WlHeader;

impl WlHeader {
  pub fn setup(window: &ApplicationWindow, title: &str) {
    let title = Label::new(Some(title));
    let header = HeaderBar::builder()
      .show_title_buttons(true)
      .decoration_layout("menu:minimize,maximize,close")
      .title_widget(&title)
      .build();

    window.set_titlebar(Some(&header));
    Self::connect_resize_window(&header, window);
  }

  fn connect_resize_window(header: &HeaderBar, window: &ApplicationWindow) {
    let header_weak = header.downgrade();
    window.connect_resizable_notify(move |window| {
      if let Some(header) = header_weak.upgrade() {
        let is_resizable = window.is_resizable();
        header.set_decoration_layout(if !is_resizable {
          Some("menu:minimize,close")
        } else {
          Some("menu:minimize,maximize,close")
        });
      }
    });
  }
}
