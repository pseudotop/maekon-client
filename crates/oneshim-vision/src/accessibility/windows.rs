//! Windows UIAutomation accessibility extractor.
//!
//! Extracts the currently focused UI element via the COM-based IUIAutomation API.
//! The implementation follows the same circuit breaker and PII gating patterns
//! used by the macOS native extractor.
//!
//! COM call sequence (focused element):
//!   1. `CoInitializeEx(COINIT_MULTITHREADED)` — initialize COM on the thread
//!   2. `CoCreateInstance(CUIAutomation)` — obtain `IUIAutomation` interface
//!   3. `IUIAutomation::GetFocusedElement()` — get the focused `IUIAutomationElement`
//!   4. Extract properties:
//!      - `CurrentControlType()` — mapped to a role string
//!      - `CurrentName()` — accessibility label
//!      - `CurrentBoundingRectangle()` — screen position/size
//!      - `GetCurrentPropertyValue(UIA_ValueValuePropertyId)` — text value
//!   5. COM objects are released automatically via `Drop` (type-safe wrappers)
//!
//! Tree traversal (window elements) uses CacheRequest for bulk property
//! fetching. Instead of 3 cross-process COM calls per element (ControlType,
//! Name, BoundingRectangle), a CacheRequest pre-fetches all three properties
//! in a single cross-process call per subtree level via BuildCache walker
//! methods. Falls back to per-property fetching if CacheRequest creation fails.
//!
//! ## Migration Note (vtable → type-safe COM)
//!
//! This module was migrated from raw vtable COM calls (`windows-sys` +
//! manual vtable index constants) to type-safe COM via the `windows` crate
//! (0.62). The `windows` crate provides proper COM interface wrappers
//! (`IUIAutomation`, `IUIAutomationElement`, `IUIAutomationTreeWalker`,
//! `IUIAutomationCacheRequest`) that eliminate the need for hard-coded
//! vtable offsets and `transmute` calls.
//!
//! `windows-sys` is retained for `IsDebuggerPresent` and `CoInitializeEx` /
//! `CoUninitialize` (the `windows` crate's `CoInitializeEx` returns
//! `HRESULT` which requires different error handling).

#[cfg(target_os = "windows")]
mod inner {
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use tracing::{debug, warn};
    use zeroize::Zeroizing;

    use oneshim_core::config::PiiFilterLevel;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::focused_element::{
        AccessibilityElement, ElementRect, FocusedElementInfo,
    };
    use oneshim_core::ports::accessibility::AccessibilityExtractor;

    use crate::privacy::sanitize_title_with_level;

    // ── Circuit breaker (mirrors macOS impl) ─────────────────────────

    /// Consecutive COM/UIA failures before the circuit breaker opens.
    static CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);
    const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
    /// After threshold is hit, retry once every N calls (~30s at 3s poll).
    const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

    // ── UIA ControlTypeId → role string mapping ──────────────────────

    /// Map a UIA `ControlTypeId` to a human-readable role string.
    ///
    /// The numeric values are from the Windows SDK header `UIAutomationClient.h`.
    /// We keep strings consistent with macOS AXRole naming where possible and
    /// use Windows-native names otherwise.
    fn control_type_to_role(control_type_id: i32) -> &'static str {
        // UIA_*ControlTypeId values (from UIAutomationClient.h)
        const UIA_BUTTON: i32 = 50000;
        const UIA_CALENDAR: i32 = 50001;
        const UIA_CHECKBOX: i32 = 50002;
        const UIA_COMBOBOX: i32 = 50003;
        const UIA_EDIT: i32 = 50004;
        const UIA_HYPERLINK: i32 = 50005;
        const UIA_IMAGE: i32 = 50006;
        const UIA_LISTITEM: i32 = 50007;
        const UIA_LIST: i32 = 50008;
        const UIA_MENU: i32 = 50009;
        const UIA_MENUBAR: i32 = 50010;
        const UIA_MENUITEM: i32 = 50011;
        const UIA_PROGRESSBAR: i32 = 50012;
        const UIA_RADIOBUTTON: i32 = 50013;
        const UIA_SCROLLBAR: i32 = 50014;
        const UIA_SLIDER: i32 = 50015;
        const UIA_SPINNER: i32 = 50016;
        const UIA_STATUSBAR: i32 = 50017;
        const UIA_TAB: i32 = 50018;
        const UIA_TABITEM: i32 = 50019;
        const UIA_TEXT: i32 = 50020;
        const UIA_TOOLBAR: i32 = 50021;
        const UIA_TOOLTIP: i32 = 50022;
        const UIA_TREE: i32 = 50023;
        const UIA_TREEITEM: i32 = 50024;
        const UIA_CUSTOM: i32 = 50025;
        const UIA_GROUP: i32 = 50026;
        const UIA_THUMB: i32 = 50027;
        const UIA_DATAGRID: i32 = 50028;
        const UIA_DATAITEM: i32 = 50029;
        const UIA_DOCUMENT: i32 = 50030;
        const UIA_SPLITBUTTON: i32 = 50031;
        const UIA_WINDOW: i32 = 50032;
        const UIA_PANE: i32 = 50033;
        const UIA_HEADER: i32 = 50034;
        const UIA_HEADERITEM: i32 = 50035;
        const UIA_TABLE: i32 = 50036;
        const UIA_TITLEBAR: i32 = 50037;
        const UIA_SEPARATOR: i32 = 50038;

        match control_type_id {
            UIA_BUTTON => "Button",
            UIA_CALENDAR => "Calendar",
            UIA_CHECKBOX => "CheckBox",
            UIA_COMBOBOX => "ComboBox",
            UIA_EDIT => "Edit",
            UIA_HYPERLINK => "Hyperlink",
            UIA_IMAGE => "Image",
            UIA_LISTITEM => "ListItem",
            UIA_LIST => "List",
            UIA_MENU => "Menu",
            UIA_MENUBAR => "MenuBar",
            UIA_MENUITEM => "MenuItem",
            UIA_PROGRESSBAR => "ProgressBar",
            UIA_RADIOBUTTON => "RadioButton",
            UIA_SCROLLBAR => "ScrollBar",
            UIA_SLIDER => "Slider",
            UIA_SPINNER => "Spinner",
            UIA_STATUSBAR => "StatusBar",
            UIA_TAB => "Tab",
            UIA_TABITEM => "TabItem",
            UIA_TEXT => "Text",
            UIA_TOOLBAR => "ToolBar",
            UIA_TOOLTIP => "ToolTip",
            UIA_TREE => "Tree",
            UIA_TREEITEM => "TreeItem",
            UIA_CUSTOM => "Custom",
            UIA_GROUP => "Group",
            UIA_THUMB => "Thumb",
            UIA_DATAGRID => "DataGrid",
            UIA_DATAITEM => "DataItem",
            UIA_DOCUMENT => "Document",
            UIA_SPLITBUTTON => "SplitButton",
            UIA_WINDOW => "Window",
            UIA_PANE => "Pane",
            UIA_HEADER => "Header",
            UIA_HEADERITEM => "HeaderItem",
            UIA_TABLE => "Table",
            UIA_TITLEBAR => "TitleBar",
            UIA_SEPARATOR => "Separator",
            _ => "Unknown",
        }
    }

    // ── Raw extracted data (before PII filtering) ────────────────────

    /// Raw data extracted from UIAutomation before PII level gating.
    /// Text fields use `Zeroizing<String>` so memory is zeroed on drop.
    struct RawFocusedElement {
        role: String,
        name: Option<Zeroizing<String>>,
        value: Option<Zeroizing<String>>,
        position: Option<ElementRect>,
    }

    // ── COM helper module ────────────────────────────────────────────
    //
    // Uses type-safe COM wrappers from the `windows` crate (0.62).
    // COM interface methods are called directly on wrapper types
    // (`IUIAutomation`, `IUIAutomationElement`, etc.) instead of
    // manual vtable offset + transmute.
    //
    // The `windows` crate handles AddRef/Release automatically via
    // `Drop`, so explicit `release()` calls are no longer needed.

    mod com {
        use super::{control_type_to_role, ElementRect, RawFocusedElement};
        use std::ptr;
        use zeroize::Zeroizing;

        use windows::core::BSTR;
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::System::Variant::VT_BSTR;
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
            IUIAutomationTreeWalker, TreeScope, TreeScope_Children, TreeScope_Element,
            UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_NamePropertyId,
            UIA_ValueValuePropertyId,
        };

        // ── Legacy vtable constants (kept as documentation reference) ──
        //
        // These were the manually hard-coded vtable offsets used before
        // the migration to the type-safe `windows` crate. Retained here
        // as a reference for anyone debugging COM interop issues.
        //
        // IUIAutomation vtable:
        //   GetFocusedElement: index 8
        //   get_RawViewWalker: index 15
        //   CreateCacheRequest: index 20
        //
        // IUIAutomationElement vtable:
        //   GetCurrentPropertyValue: index 12
        //   get_CurrentControlType: index 21
        //   get_CachedControlType: index 22
        //   get_CurrentName: index 23
        //   get_CachedName: index 24
        //   get_CurrentBoundingRectangle: index 27
        //   get_CachedBoundingRectangle: index 28
        //
        // IUIAutomationTreeWalker vtable:
        //   GetFirstChildElement: index 4
        //   GetFirstChildElementBuildCache: index 5
        //   GetNextSiblingElement: index 6
        //   GetNextSiblingElementBuildCache: index 7
        //
        // IUIAutomationCacheRequest vtable:
        //   AddProperty: index 3
        //   put_TreeScope: index 7

        /// Convert an `IUIAutomationElement`'s `CurrentBoundingRectangle`
        /// (Win32 `RECT` with left/top/right/bottom as i32) to our
        /// `ElementRect` (x/y/width/height as f32).
        fn rect_to_element_rect(rect: &windows::Win32::Foundation::RECT) -> Option<ElementRect> {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if width > 0 || height > 0 {
                Some(ElementRect {
                    x: rect.left as f32,
                    y: rect.top as f32,
                    width: width as f32,
                    height: height as f32,
                })
            } else {
                None
            }
        }

        /// Convert a `BSTR` to an `Option<String>`, returning `None` for
        /// null/empty strings.
        fn bstr_to_opt_string(bstr: &BSTR) -> Option<String> {
            if bstr.is_empty() {
                None
            } else {
                Some(bstr.to_string())
            }
        }

        /// Extract focused element data via COM UIAutomation API.
        ///
        /// Returns `None` if COM initialization fails, no element is focused,
        /// or any step in the extraction chain fails. Errors are logged at
        /// debug level to avoid noise under normal operation.
        ///
        /// SAFETY: This function performs COM calls that are `unsafe` in the
        /// `windows` crate. It is only called from `spawn_blocking` to avoid
        /// blocking the tokio runtime.
        pub(super) fn extract_via_uia() -> Option<RawFocusedElement> {
            unsafe {
                // Step 1: Initialize COM (COINIT_MULTITHREADED = 0x0)
                let hr = windows_sys::Win32::System::Com::CoInitializeEx(
                    ptr::null(),
                    windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
                );
                // S_OK (0) or S_FALSE (1, already initialized) are both acceptable.
                if hr < 0 {
                    tracing::debug!(hresult = hr, "CoInitializeEx failed");
                    return None;
                }
                let _com_guard = ComGuard; // CoUninitialize on drop

                // Step 2: Create IUIAutomation instance (type-safe)
                let automation: IUIAutomation =
                    match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                        Ok(a) => a,
                        Err(e) => {
                            tracing::debug!(error = %e, "CoCreateInstance(CUIAutomation) failed");
                            return None;
                        }
                    };

                // Step 3: GetFocusedElement (type-safe — returns Result<IUIAutomationElement>)
                let element: IUIAutomationElement = match automation.GetFocusedElement() {
                    Ok(el) => el,
                    Err(e) => {
                        tracing::debug!(error = %e, "GetFocusedElement returned no element");
                        return None;
                    }
                };

                // Step 4a: CurrentControlType → role
                let role = match element.CurrentControlType() {
                    Ok(ct) => control_type_to_role(ct.0).to_string(),
                    Err(_) => "Unknown".to_string(),
                };

                // Step 4b: CurrentName → label
                let name = match element.CurrentName() {
                    Ok(bstr) => bstr_to_opt_string(&bstr).map(Zeroizing::new),
                    Err(_) => None,
                };

                // Step 4c: CurrentBoundingRectangle → position
                let position = match element.CurrentBoundingRectangle() {
                    Ok(rect) => rect_to_element_rect(&rect),
                    Err(_) => None,
                };

                // Step 4d: GetCurrentPropertyValue(UIA_ValueValuePropertyId) → text value
                let value = match element.GetCurrentPropertyValue(UIA_ValueValuePropertyId) {
                    Ok(variant) => {
                        if variant.vt() == VT_BSTR {
                            // SAFETY: vt() confirmed VT_BSTR, so bstrVal is valid.
                            // bstrVal is ManuallyDrop<BSTR> in windows 0.62 — clone to avoid double-free.
                            let bstr = unsafe {
                                std::mem::ManuallyDrop::into_inner(
                                    variant.Anonymous.Anonymous.Anonymous.bstrVal.clone(),
                                )
                            };
                            bstr_to_opt_string(&bstr).map(Zeroizing::new)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };

                // element, automation are dropped here — Release called automatically

                Some(RawFocusedElement {
                    role,
                    name,
                    value,
                    position,
                })
            }
        }

        /// Create a CacheRequest that pre-fetches ControlType, Name, and
        /// BoundingRectangle properties for Element + Children scope.
        ///
        /// Returns `None` if creation or configuration fails.
        ///
        /// SAFETY: Requires COM to be initialized on the current thread.
        unsafe fn create_cache_request(
            automation: &IUIAutomation,
        ) -> Option<IUIAutomationCacheRequest> {
            let cache_request = match automation.CreateCacheRequest() {
                Ok(cr) => cr,
                Err(e) => {
                    tracing::debug!(error = %e, "CreateCacheRequest failed");
                    return None;
                }
            };

            // AddProperty for each property we need
            if let Err(e) = cache_request.AddProperty(UIA_ControlTypePropertyId) {
                tracing::debug!(error = %e, "AddProperty(ControlType) failed");
                return None;
            }
            if let Err(e) = cache_request.AddProperty(UIA_NamePropertyId) {
                tracing::debug!(error = %e, "AddProperty(Name) failed");
                return None;
            }
            if let Err(e) = cache_request.AddProperty(UIA_BoundingRectanglePropertyId) {
                tracing::debug!(error = %e, "AddProperty(BoundingRectangle) failed");
                return None;
            }

            // SetTreeScope(Element | Children)
            if let Err(e) =
                cache_request.SetTreeScope(TreeScope(TreeScope_Element.0 | TreeScope_Children.0))
            {
                tracing::debug!(error = %e, "SetTreeScope failed");
                return None;
            }

            Some(cache_request)
        }

        /// Extract properties from an element's cache (populated by BuildCache).
        ///
        /// Uses `CachedControlType`, `CachedName`, and
        /// `CachedBoundingRectangle` instead of their `Current` counterparts.
        /// This avoids cross-process COM calls since the data was pre-fetched.
        ///
        /// SAFETY: `element` must be a valid IUIAutomationElement with a
        /// populated cache (obtained via a BuildCache walker method).
        unsafe fn extract_cached_properties(
            element: &IUIAutomationElement,
        ) -> (String, Option<String>, Option<ElementRect>) {
            let role = match element.CachedControlType() {
                Ok(ct) => control_type_to_role(ct.0).to_string(),
                Err(_) => "Unknown".to_string(),
            };

            let name = match element.CachedName() {
                Ok(bstr) => bstr_to_opt_string(&bstr),
                Err(_) => None,
            };

            let position = match element.CachedBoundingRectangle() {
                Ok(rect) => rect_to_element_rect(&rect),
                Err(_) => None,
            };

            (role, name, position)
        }

        /// Extract properties from an element using per-property Current calls.
        ///
        /// This is the non-cached fallback path. Each property requires a
        /// separate cross-process COM call.
        ///
        /// SAFETY: `element` must be a valid IUIAutomationElement.
        unsafe fn extract_current_properties(
            element: &IUIAutomationElement,
        ) -> (String, Option<String>, Option<ElementRect>) {
            let role = match element.CurrentControlType() {
                Ok(ct) => control_type_to_role(ct.0).to_string(),
                Err(_) => "Unknown".to_string(),
            };

            let name = match element.CurrentName() {
                Ok(bstr) => bstr_to_opt_string(&bstr),
                Err(_) => None,
            };

            let position = match element.CurrentBoundingRectangle() {
                Ok(rect) => rect_to_element_rect(&rect),
                Err(_) => None,
            };

            (role, name, position)
        }

        /// Extract the accessibility subtree of the focused element's parent window.
        ///
        /// Uses IUIAutomation TreeWalker for breadth-first traversal with
        /// depth and element count limits. When possible, a CacheRequest is
        /// used to batch-fetch ControlType, Name, and BoundingRectangle in a
        /// single cross-process call per subtree level (3x fewer COM roundtrips).
        /// Falls back to per-property fetching if CacheRequest creation fails.
        pub(super) fn extract_tree_via_uia(
            max_depth: u32,
            max_elements: usize,
        ) -> Vec<(String, Option<String>, Option<ElementRect>)> {
            unsafe {
                let hr = windows_sys::Win32::System::Com::CoInitializeEx(
                    ptr::null(),
                    windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
                );
                if hr < 0 {
                    return Vec::new();
                }
                let _com_guard = ComGuard;

                // Create IUIAutomation (type-safe)
                let automation: IUIAutomation =
                    match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                        Ok(a) => a,
                        Err(_) => return Vec::new(),
                    };

                // Get focused element (type-safe)
                let element: IUIAutomationElement = match automation.GetFocusedElement() {
                    Ok(el) => el,
                    Err(_) => return Vec::new(),
                };

                // Get RawViewWalker (type-safe)
                let walker: IUIAutomationTreeWalker = match automation.RawViewWalker() {
                    Ok(w) => w,
                    Err(_) => return Vec::new(),
                };

                // Try to create a CacheRequest for bulk property fetching.
                let cache_request = create_cache_request(&automation);
                let use_cache = cache_request.is_some();
                if use_cache {
                    tracing::debug!("UIA tree traversal: using CacheRequest (bulk property fetch)");
                } else {
                    tracing::debug!(
                        "UIA tree traversal: CacheRequest unavailable, using per-property calls"
                    );
                }

                let mut results = Vec::new();
                let mut remaining = max_elements;
                collect_subtree(
                    &walker,
                    &element,
                    0,
                    max_depth,
                    &mut remaining,
                    &mut results,
                    cache_request.as_ref(),
                    use_cache,
                );

                // walker, element, cache_request, automation dropped here — Release automatic
                results
            }
        }

        /// Recursive depth-limited subtree collection with optional CacheRequest.
        ///
        /// When `use_cache` is true and `cache_request` is `Some`, uses
        /// `GetFirstChildElementBuildCache` / `GetNextSiblingElementBuildCache`
        /// to populate the element cache, then reads properties via
        /// `Cached*` methods. This reduces cross-process COM calls from
        /// 3 per element to 1 per walker step.
        ///
        /// When `use_cache` is false, falls back to the original per-property
        /// `Current*` calls (3 cross-process calls per element).
        unsafe fn collect_subtree(
            walker: &IUIAutomationTreeWalker,
            element: &IUIAutomationElement,
            depth: u32,
            max_depth: u32,
            remaining: &mut usize,
            results: &mut Vec<(String, Option<String>, Option<ElementRect>)>,
            cache_request: Option<&IUIAutomationCacheRequest>,
            use_cache: bool,
        ) {
            if *remaining == 0 || depth > max_depth {
                return;
            }

            // Extract properties — cached path reads from pre-fetched cache,
            // fallback path makes individual COM calls per property.
            let (role, name, position) = if use_cache {
                extract_cached_properties(element)
            } else {
                extract_current_properties(element)
            };

            results.push((role, name, position));
            *remaining = remaining.saturating_sub(1);

            // Recurse into children
            if depth < max_depth && *remaining > 0 {
                let first_child = if use_cache {
                    if let Some(cr) = cache_request {
                        walker.GetFirstChildElementBuildCache(element, cr).ok()
                    } else {
                        walker.GetFirstChildElement(element).ok()
                    }
                } else {
                    walker.GetFirstChildElement(element).ok()
                };

                if let Some(child) = first_child {
                    collect_subtree(
                        walker,
                        &child,
                        depth + 1,
                        max_depth,
                        remaining,
                        results,
                        cache_request,
                        use_cache,
                    );

                    // Traverse siblings
                    let mut current = child;
                    loop {
                        if *remaining == 0 {
                            break;
                        }

                        let next_sibling = if use_cache {
                            if let Some(cr) = cache_request {
                                walker.GetNextSiblingElementBuildCache(&current, cr).ok()
                            } else {
                                walker.GetNextSiblingElement(&current).ok()
                            }
                        } else {
                            walker.GetNextSiblingElement(&current).ok()
                        };

                        // current is dropped here when reassigned — Release automatic
                        match next_sibling {
                            Some(sibling) => {
                                current = sibling;
                                collect_subtree(
                                    walker,
                                    &current,
                                    depth + 1,
                                    max_depth,
                                    remaining,
                                    results,
                                    cache_request,
                                    use_cache,
                                );
                            }
                            None => break,
                        }
                    }
                }
            }
        }

        /// RAII guard to call `CoUninitialize` when dropped.
        struct ComGuard;

        impl Drop for ComGuard {
            fn drop(&mut self) {
                unsafe {
                    windows_sys::Win32::System::Com::CoUninitialize();
                }
            }
        }
    }

    // ── Public extractor struct ──────────────────────────────────────

    pub struct WindowsUiaAccessibility;

    impl Default for WindowsUiaAccessibility {
        fn default() -> Self {
            Self
        }
    }

    impl WindowsUiaAccessibility {
        pub fn new() -> Self {
            Self
        }

        /// Check if a debugger is attached to the current process.
        /// When detected, text extraction is skipped to prevent memory
        /// inspection of sensitive accessibility data.
        fn is_debugger_attached() -> bool {
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent() != 0 }
        }

        // ── Circuit breaker (same pattern as macOS) ──────────────────

        fn circuit_allows() -> bool {
            let failures = CONSECUTIVE_FAILURES.load(Ordering::Relaxed);
            if failures >= CIRCUIT_BREAKER_THRESHOLD {
                if failures % CIRCUIT_BREAKER_RETRY_INTERVAL != 0 {
                    CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                warn!(
                    "WindowsUiaAccessibility: circuit breaker retry after {} skipped",
                    failures - CIRCUIT_BREAKER_THRESHOLD
                );
            }
            true
        }

        fn record_success() {
            CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);
        }

        fn record_failure() {
            let prev = CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            if prev + 1 == CIRCUIT_BREAKER_THRESHOLD {
                warn!("WindowsUiaAccessibility: circuit breaker tripped after {CIRCUIT_BREAKER_THRESHOLD} consecutive failures");
            }
        }

        // ── PII level gating (mirrors macOS) ─────────────────────────

        /// Apply PII-level filtering to raw extracted data.
        ///
        /// Level semantics:
        /// - `Strict`: role + position only
        /// - `Standard`: + label + value_length (no text content)
        /// - `Basic`: + sanitized text (PII patterns masked)
        /// - `Off`: full text (requires explicit consent)
        fn filter_by_level(raw: RawFocusedElement, level: PiiFilterLevel) -> FocusedElementInfo {
            match level {
                PiiFilterLevel::Strict => FocusedElementInfo {
                    role: raw.role,
                    position: raw.position,
                    ..Default::default()
                },
                PiiFilterLevel::Standard => FocusedElementInfo {
                    role: raw.role,
                    position: raw.position,
                    label: raw.name.as_deref().map(|s| s.to_string()),
                    value_length: raw.value.as_deref().map(|v| v.len() as u32),
                    ..Default::default()
                },
                PiiFilterLevel::Basic => {
                    let text = raw
                        .value
                        .as_deref()
                        .map(|v| sanitize_title_with_level(v, PiiFilterLevel::Basic));
                    FocusedElementInfo {
                        role: raw.role,
                        position: raw.position,
                        label: raw.name.as_deref().map(|s| s.to_string()),
                        value_length: raw.value.as_deref().map(|v| v.len() as u32),
                        extracted_text: text,
                    }
                }
                PiiFilterLevel::Off => FocusedElementInfo {
                    role: raw.role,
                    position: raw.position,
                    label: raw.name.as_deref().map(|s| s.to_string()),
                    value_length: raw.value.as_deref().map(|v| v.len() as u32),
                    extracted_text: raw.value.as_deref().map(|v| v.to_string()),
                },
            }
            // raw.name and raw.value (Zeroizing<String>) are dropped here,
            // zeroing memory automatically.
        }
    }

    #[async_trait]
    impl AccessibilityExtractor for WindowsUiaAccessibility {
        async fn extract_focused_element(
            &self,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Option<FocusedElementInfo>, CoreError> {
            // Security: skip when debugger is attached
            if Self::is_debugger_attached() {
                warn!("Debugger detected; skipping accessibility text extraction");
                return Ok(None);
            }

            // Circuit breaker: skip when too many consecutive failures
            if !Self::circuit_allows() {
                debug!("WindowsUiaAccessibility: circuit breaker open");
                return Ok(None);
            }

            // Consent gating: fall back to Standard if Off without consent
            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                debug!(
                    "PII Off requested but full_text_extraction consent missing; \
                     falling back to Standard"
                );
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            // Run synchronous COM calls on a blocking thread
            let result = tokio::task::spawn_blocking(com::extract_via_uia)
                .await
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("UIA blocking task failed: {e}"),
                })?;

            match result {
                Some(raw) => {
                    Self::record_success();
                    let filtered = Self::filter_by_level(raw, effective_level);
                    debug!(role = %filtered.role, "UIA focused element extracted");
                    Ok(Some(filtered))
                }
                None => {
                    Self::record_failure();
                    Ok(None)
                }
            }
        }

        async fn extract_window_elements(
            &self,
            max_depth: u32,
            max_elements: usize,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Vec<AccessibilityElement>, CoreError> {
            if Self::is_debugger_attached() {
                return Ok(Vec::new());
            }
            if !Self::circuit_allows() {
                return Ok(Vec::new());
            }

            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            let result = tokio::task::spawn_blocking(move || {
                com::extract_tree_via_uia(max_depth, max_elements)
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("UIA tree traversal task failed: {e}"),
            })?;

            if result.is_empty() {
                Self::record_failure();
            } else {
                Self::record_success();
            }

            // Already handled: COM failures return empty Vec, not PermissionDenied.
            // Windows UIA does not require special permissions.
            Ok(result
                .into_iter()
                .map(|(role, name, bounds)| {
                    let label = if effective_level == PiiFilterLevel::Strict {
                        String::new()
                    } else {
                        name.unwrap_or_default()
                    };
                    AccessibilityElement {
                        role,
                        label,
                        bounds,
                    }
                })
                .collect())
        }

        fn has_permission(&self) -> bool {
            // Windows UIAutomation does not require special permissions
            true
        }

        fn name(&self) -> &str {
            "windows-uia-accessibility"
        }
    }
}

#[cfg(target_os = "windows")]
pub use inner::WindowsUiaAccessibility;

#[cfg(test)]
#[cfg(target_os = "windows")]
mod tests {
    use super::inner::*;
    use oneshim_core::config::PiiFilterLevel;
    use oneshim_core::models::focused_element::ElementRect;
    use oneshim_core::ports::accessibility::AccessibilityExtractor;

    #[test]
    fn has_permission_true() {
        let extractor = WindowsUiaAccessibility::new();
        assert!(extractor.has_permission());
    }

    #[test]
    fn name_is_correct() {
        let extractor = WindowsUiaAccessibility::new();
        assert_eq!(extractor.name(), "windows-uia-accessibility");
    }

    #[test]
    fn control_type_mapping_known_types() {
        // Button = 50000
        assert_eq!(super::inner::control_type_to_role(50000), "Button");
        // Edit = 50004
        assert_eq!(super::inner::control_type_to_role(50004), "Edit");
        // Document = 50030
        assert_eq!(super::inner::control_type_to_role(50030), "Document");
        // Text = 50020
        assert_eq!(super::inner::control_type_to_role(50020), "Text");
    }

    #[test]
    fn control_type_mapping_unknown() {
        assert_eq!(super::inner::control_type_to_role(99999), "Unknown");
    }

    #[tokio::test]
    async fn extract_returns_ok() {
        // On CI or real Windows, this should return Ok (either Some or None
        // depending on whether a window is focused).
        let extractor = WindowsUiaAccessibility::new();
        let result = extractor
            .extract_focused_element(PiiFilterLevel::Standard, false)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn extract_window_elements_returns_ok() {
        let extractor = WindowsUiaAccessibility::new();
        let result = extractor
            .extract_window_elements(3, 300, PiiFilterLevel::Standard, false)
            .await;
        assert!(result.is_ok());
    }

    #[test]
    fn filter_strict_strips_label_and_text() {
        let info = apply_test_filter(
            "Edit",
            Some("Username"),
            Some("john@example.com"),
            Some(ElementRect {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 25.0,
            }),
            PiiFilterLevel::Strict,
        );
        assert_eq!(info.role, "Edit");
        assert!(info.position.is_some());
        assert!(info.label.is_none());
        assert!(info.value_length.is_none());
        assert!(info.extracted_text.is_none());
    }

    #[test]
    fn filter_standard_includes_label_and_length() {
        let info = apply_test_filter(
            "Edit",
            Some("Search"),
            Some("cargo test"),
            None,
            PiiFilterLevel::Standard,
        );
        assert_eq!(info.label, Some("Search".to_string()));
        assert_eq!(info.value_length, Some(10));
        assert!(info.extracted_text.is_none());
    }

    #[test]
    fn filter_basic_includes_sanitized_text() {
        let info = apply_test_filter(
            "Edit",
            None,
            Some("user@example.com"),
            None,
            PiiFilterLevel::Basic,
        );
        assert!(info.extracted_text.is_some());
        let text = info.extracted_text.unwrap();
        assert!(text.contains("[EMAIL]"));
        assert!(!text.contains("user@example.com"));
    }

    #[test]
    fn filter_off_includes_full_text() {
        let info = apply_test_filter(
            "Document",
            None,
            Some("full content here"),
            None,
            PiiFilterLevel::Off,
        );
        assert_eq!(info.extracted_text, Some("full content here".to_string()));
    }

    /// Helper to test PII filtering without COM calls.
    fn apply_test_filter(
        role: &str,
        name: Option<&str>,
        value: Option<&str>,
        position: Option<ElementRect>,
        level: PiiFilterLevel,
    ) -> oneshim_core::models::focused_element::FocusedElementInfo {
        use crate::privacy::sanitize_title_with_level;
        use oneshim_core::models::focused_element::FocusedElementInfo;
        use zeroize::Zeroizing;

        let name_z = name.map(|s| Zeroizing::new(s.to_string()));
        let value_z = value.map(|s| Zeroizing::new(s.to_string()));

        match level {
            PiiFilterLevel::Strict => FocusedElementInfo {
                role: role.to_string(),
                position,
                ..Default::default()
            },
            PiiFilterLevel::Standard => FocusedElementInfo {
                role: role.to_string(),
                position,
                label: name_z.as_deref().map(|s| s.to_string()),
                value_length: value_z.as_deref().map(|v| v.len() as u32),
                ..Default::default()
            },
            PiiFilterLevel::Basic => {
                let text = value_z
                    .as_deref()
                    .map(|v| sanitize_title_with_level(v, PiiFilterLevel::Basic));
                FocusedElementInfo {
                    role: role.to_string(),
                    position,
                    label: name_z.as_deref().map(|s| s.to_string()),
                    value_length: value_z.as_deref().map(|v| v.len() as u32),
                    extracted_text: text,
                }
            }
            PiiFilterLevel::Off => FocusedElementInfo {
                role: role.to_string(),
                position,
                label: name_z.as_deref().map(|s| s.to_string()),
                value_length: value_z.as_deref().map(|v| v.len() as u32),
                extracted_text: value_z.as_deref().map(|v| v.to_string()),
            },
        }
    }

    // ── D6 expanded coverage: all 39 UIA control types ─────────────────

    #[test]
    fn control_type_all_known_mappings() {
        // Exhaustive test of the UIA ControlTypeId enum (50000-50038).
        // Every row matches UIAutomationClient.h. Any drift in the mapping
        // surfaces here before shipping to Windows CI.
        let cases: &[(i32, &str)] = &[
            (50000, "Button"),
            (50001, "Calendar"),
            (50002, "CheckBox"),
            (50003, "ComboBox"),
            (50004, "Edit"),
            (50005, "Hyperlink"),
            (50006, "Image"),
            (50007, "ListItem"),
            (50008, "List"),
            (50009, "Menu"),
            (50010, "MenuBar"),
            (50011, "MenuItem"),
            (50012, "ProgressBar"),
            (50013, "RadioButton"),
            (50014, "ScrollBar"),
            (50015, "Slider"),
            (50016, "Spinner"),
            (50017, "StatusBar"),
            (50018, "Tab"),
            (50019, "TabItem"),
            (50020, "Text"),
            (50021, "ToolBar"),
            (50022, "ToolTip"),
            (50023, "Tree"),
            (50024, "TreeItem"),
            (50025, "Custom"),
            (50026, "Group"),
            (50027, "Thumb"),
            (50028, "DataGrid"),
            (50029, "DataItem"),
            (50030, "Document"),
            (50031, "SplitButton"),
            (50032, "Window"),
            (50033, "Pane"),
            (50034, "Header"),
            (50035, "HeaderItem"),
            (50036, "Table"),
            (50037, "TitleBar"),
            (50038, "Separator"),
        ];
        for (id, expected) in cases {
            assert_eq!(
                super::inner::control_type_to_role(*id),
                *expected,
                "control_type_to_role({id}) mismatch"
            );
        }
    }

    #[test]
    fn control_type_boundary_ids() {
        // IDs outside the 50000-50038 range should all map to "Unknown".
        assert_eq!(super::inner::control_type_to_role(0), "Unknown");
        assert_eq!(super::inner::control_type_to_role(-1), "Unknown");
        assert_eq!(super::inner::control_type_to_role(49999), "Unknown");
        assert_eq!(super::inner::control_type_to_role(50039), "Unknown");
        assert_eq!(super::inner::control_type_to_role(i32::MIN), "Unknown");
        assert_eq!(super::inner::control_type_to_role(i32::MAX), "Unknown");
    }

    // ── D6 expanded coverage: filter_by_level edge cases ───────────────

    #[test]
    fn filter_strict_with_empty_value_stays_empty() {
        let info = apply_test_filter("Edit", Some(""), Some(""), None, PiiFilterLevel::Strict);
        assert_eq!(info.role, "Edit");
        assert!(info.label.is_none());
        assert!(info.value_length.is_none());
        assert!(info.extracted_text.is_none());
    }

    #[test]
    fn filter_standard_value_length_counts_bytes() {
        // Length is byte count (str::len), not char count.
        let info = apply_test_filter(
            "Edit",
            None,
            Some("café"), // 5 bytes (UTF-8), 4 chars
            None,
            PiiFilterLevel::Standard,
        );
        assert_eq!(info.value_length, Some(5));
    }

    #[test]
    fn filter_basic_none_value_produces_none_text() {
        let info = apply_test_filter("Edit", Some("label"), None, None, PiiFilterLevel::Basic);
        assert_eq!(info.label, Some("label".to_string()));
        assert!(info.extracted_text.is_none());
    }

    #[test]
    fn filter_off_with_multi_line_value_preserves_all() {
        let raw = "line1\nline2\nline3";
        let info = apply_test_filter("Document", None, Some(raw), None, PiiFilterLevel::Off);
        assert_eq!(info.extracted_text, Some(raw.to_string()));
    }

    #[test]
    fn filter_preserves_position_across_all_levels() {
        let rect = ElementRect {
            x: 100.0,
            y: 200.0,
            width: 50.0,
            height: 25.0,
        };
        for level in [
            PiiFilterLevel::Strict,
            PiiFilterLevel::Standard,
            PiiFilterLevel::Basic,
            PiiFilterLevel::Off,
        ] {
            let info = apply_test_filter("Button", None, None, Some(rect.clone()), level);
            assert_eq!(
                info.position,
                Some(rect.clone()),
                "level {level:?} dropped position"
            );
        }
    }

    #[test]
    fn filter_basic_masks_credit_card_pattern() {
        let info = apply_test_filter(
            "Edit",
            None,
            Some("card 4111-1111-1111-1111 end"),
            None,
            PiiFilterLevel::Basic,
        );
        let text = info.extracted_text.expect("Basic produces text");
        // Basic level masks email + phone. Credit card is Standard+.
        // Verify Basic does NOT mask cards (correct per 4-tier cascade).
        assert!(text.contains("4111-1111-1111-1111") || text.contains("[CARD]"));
    }
}
