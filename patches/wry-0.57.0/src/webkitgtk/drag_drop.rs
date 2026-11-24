// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use webkit6::WebView;

use crate::DragDropEvent;

pub(crate) fn connect_drag_event(_webview: &WebView, _handler: Box<dyn Fn(DragDropEvent) -> bool>) {
  // GTK4 removed the GTK3 drag signals used by Wry 0.55.1. Keep the handler
  // disabled for the E31 compile migration until a DropTarget-based port lands.
}
