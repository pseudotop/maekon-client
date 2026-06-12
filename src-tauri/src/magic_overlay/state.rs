use maekon_core::config::OverlayMode;

#[derive(Debug)]
pub(super) struct OverlayState {
    pub(super) mode: OverlayMode,
    pub(super) visible: bool,
    pub(super) current_message_id: Option<String>,
    pub(super) detection_active: bool,
    pub(super) suggestions_panel_open: bool,
    pub(super) automation_confirm_active: bool,
}
