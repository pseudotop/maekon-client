use maekon_core::models::focused_element::ElementRect;
use zeroize::Zeroizing;

/// Raw data extracted from UIAutomation before PII level gating.
/// Text fields use `Zeroizing<String>` so memory is zeroed on drop.
pub(super) struct RawFocusedElement {
    pub(super) role: String,
    pub(super) name: Option<Zeroizing<String>>,
    pub(super) value: Option<Zeroizing<String>>,
    pub(super) position: Option<ElementRect>,
}
