use std::{cell::RefCell, rc::Rc};

use gtk::{gdk::ModifierType, prelude::*, EventSequenceState, GestureClick};
use webkit6::{prelude::*, WebView};

pub fn setup(webview: &WebView) {
  let gesture = GestureClick::new();
  gesture.set_button(0);

  let bf_state = BackForwardState(Rc::new(RefCell::new(0)));

  let bf_state_c = bf_state.clone();
  let webview_c = webview.clone();
  gesture.connect_pressed(move |gesture, n_press, x, y| {
    let button = gesture.current_button();
    let mut inhibit = false;
    match button {
      // back button
      8 => {
        inhibit = true;
        bf_state_c.set(BACK);
        webview_c.evaluate_javascript(
          &create_js_mouse_event(
            "mousedown",
            button,
            x,
            y,
            n_press,
            gesture.current_event_state(),
            &bf_state_c,
          ),
          None,
          None,
          None::<&gtk::gio::Cancellable>,
          |_| {},
        );
      }
      // forward button
      9 => {
        inhibit = true;
        bf_state_c.set(FORWARD);
        webview_c.evaluate_javascript(
          &create_js_mouse_event(
            "mousedown",
            button,
            x,
            y,
            n_press,
            gesture.current_event_state(),
            &bf_state_c,
          ),
          None,
          None,
          None::<&gtk::gio::Cancellable>,
          |_| {},
        );
      }
      _ => {}
    }

    if inhibit {
      gesture.set_state(EventSequenceState::Claimed);
    }
  });

  let bf_state_c = bf_state.clone();
  let webview_c = webview.clone();
  gesture.connect_released(move |gesture, n_press, x, y| {
    let button = gesture.current_button();
    let mut inhibit = false;
    match button {
      // back button
      8 => {
        inhibit = true;
        bf_state_c.remove(BACK);
        webview_c.evaluate_javascript(
          &create_js_mouse_event(
            "mouseup",
            button,
            x,
            y,
            n_press,
            gesture.current_event_state(),
            &bf_state_c,
          ),
          None,
          None,
          None::<&gtk::gio::Cancellable>,
          |_| {},
        );
      }
      // forward button
      9 => {
        inhibit = true;
        bf_state_c.remove(FORWARD);
        webview_c.evaluate_javascript(
          &create_js_mouse_event(
            "mouseup",
            button,
            x,
            y,
            n_press,
            gesture.current_event_state(),
            &bf_state_c,
          ),
          None,
          None,
          None::<&gtk::gio::Cancellable>,
          |_| {},
        );
      }
      _ => {}
    }
    if inhibit {
      gesture.set_state(EventSequenceState::Claimed);
    }
  });

  webview.add_controller(gesture);
}

fn create_js_mouse_event(
  event_name: &str,
  native_button: u32,
  x: f64,
  y: f64,
  detail: i32,
  modifers_state: ModifierType,
  state: &BackForwardState,
) -> String {
  // js equivalent https://developer.mozilla.org/en-US/docs/Web/API/MouseEvent/button
  let button = if native_button == 8 { 3 } else { 4 };
  let (x, y) = (x as i32, y as i32);
  let mut buttons = 0;
  // left button
  if modifers_state.contains(ModifierType::BUTTON1_MASK) {
    buttons += 1;
  }
  // right button
  if modifers_state.contains(ModifierType::BUTTON3_MASK) {
    buttons += 2;
  }
  // middle button
  if modifers_state.contains(ModifierType::BUTTON2_MASK) {
    buttons += 4;
  }
  // back button
  if state.has(BACK) {
    buttons += 8;
  }
  // if modifers_state.contains(ModifierType::BUTTON4_MASK) {
  //   buttons += 8;
  // }
  // forward button
  if state.has(FORWARD) {
    buttons += 16;
  }
  // if modifers_state.contains(ModifierType::BUTTON5_MASK) {
  //   buttons += 16;
  // }
  format!(
    r#"(() => {{
        const el = document.elementFromPoint({x},{y});
        const ev = new MouseEvent('{event_name}', {{
          view: window,
          button: {button},
          buttons: {buttons},
          x: {x},
          y: {y},
          bubbles: true,
          detail: {detail},
          cancelBubble: false,
          cancelable: true,
          clientX: {x},
          clientY: {y},
          composed: true,
          layerX: {x},
          layerY: {y},
          pageX: {x},
          pageY: {y},
          screenX: window.screenX + {x},
          screenY: window.screenY + {y},
          ctrlKey: {ctrl_key},
          metaKey: {meta_key},
          shiftKey: {shift_key},
          altKey: {alt_key},
        }});
        el.dispatchEvent(ev)
        if (!ev.defaultPrevented && "{event_name}" === "mouseup") {{
          if (ev.button === 3) {{
            window.history.back();
          }}
          if (ev.button === 4) {{
            window.history.forward();
          }}
        }}
      }})()"#,
    event_name = event_name,
    x = x,
    y = y,
    detail = detail,
    ctrl_key = modifers_state.contains(ModifierType::CONTROL_MASK),
    alt_key = modifers_state.contains(ModifierType::ALT_MASK),
    shift_key = modifers_state.contains(ModifierType::SHIFT_MASK),
    meta_key = modifers_state.contains(ModifierType::SUPER_MASK),
    button = button,
    buttons = buttons,
  )
}

// Internal modifiers to track whether BACK/FORWARD buttons are pressed
const BACK: u8 = 0b01;
const FORWARD: u8 = 0b10;

/// A single u8 that stores whether [BACK] and [FORWARD] are pressed or not
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BackForwardState(Rc<RefCell<u8>>);

impl BackForwardState {
  fn set(&self, button: u8) {
    *self.0.borrow_mut() |= button
  }

  fn remove(&self, button: u8) {
    *self.0.borrow_mut() &= !button
  }

  fn has(&self, button: u8) -> bool {
    let state = *self.0.borrow();
    state & !button != state
  }
}
