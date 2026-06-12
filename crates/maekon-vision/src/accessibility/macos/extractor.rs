//! MacOsNativeAccessibility — extract, batch, traverse, filter,
//! AccessibilityExtractor trait impl.

use std::process::Command;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFRelease, CFTypeRef};
use core_foundation_sys::string::CFStringRef;
use tracing::{debug, warn};
use zeroize::Zeroizing;

use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::error_codes::NotFoundCode;
use maekon_core::models::focused_element::{AccessibilityElement, ElementRect, FocusedElementInfo};
use maekon_core::ports::accessibility::AccessibilityExtractor;

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};

use crate::accessibility::ffi_macos::ax::*;

/// Circuit breaker: skip AX calls after consecutive failures.
static CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
/// Retry every 10 ticks (~30s at 3s poll) after circuit opens.
const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

/// Raw data extracted from the accessibility API before PII filtering.
struct RawFocusedElement {
    role: String,
    title: Option<Zeroizing<String>>,
    value: Option<Zeroizing<String>>,
    placeholder: Option<String>,
    position: Option<ElementRect>,
}

/// Result of a batched attribute fetch for a single AX element.
/// Used by `batch_get_attributes()` to return role, title, description,
/// and bounds from a single `AXUIElementCopyMultipleAttributeValues` call.
struct BatchAttributes {
    role: String,
    title: Option<String>,
    description: Option<String>,
    position_and_size: Option<ElementRect>,
}

pub struct MacOsNativeAccessibility;

impl Default for MacOsNativeAccessibility {
    fn default() -> Self {
        Self
    }
}

impl MacOsNativeAccessibility {
    pub fn new() -> Self {
        Self
    }

    /// Check if accessibility permission is granted.
    fn check_permission() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(ptr::null()) }
    }

    fn candidate_process_names(app_name: &str) -> Vec<String> {
        let trimmed = app_name.trim();
        let without_app_suffix = trimmed.strip_suffix(".app").unwrap_or(trimmed);
        let last_path_component = without_app_suffix
            .rsplit('/')
            .next()
            .unwrap_or(without_app_suffix);

        let mut candidates = vec![last_path_component.to_string()];
        if last_path_component.eq_ignore_ascii_case("maekon dev")
            || last_path_component.eq_ignore_ascii_case("maekon")
        {
            candidates.push("maekon".to_string());
        }
        candidates.sort();
        candidates.dedup();
        candidates
    }

    fn pgrep_exact(name: &str) -> Option<PidT> {
        let output = Command::new("/usr/bin/pgrep")
            .args(["-x", name])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.trim().parse::<PidT>().ok())
    }

    fn is_current_process_candidate(name: &str) -> bool {
        let Ok(current_exe) = std::env::current_exe() else {
            return false;
        };
        let Some(executable_name) = current_exe.file_stem().and_then(|name| name.to_str()) else {
            return false;
        };
        name.eq_ignore_ascii_case(executable_name)
            || current_exe
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .filter_map(|component| component.strip_suffix(".app"))
                .any(|bundle_name| name.eq_ignore_ascii_case(bundle_name))
    }

    fn find_application_pid(app_name: &str) -> Result<PidT, CoreError> {
        for candidate in Self::candidate_process_names(app_name) {
            if Self::is_current_process_candidate(&candidate) {
                return Ok(std::process::id() as PidT);
            }
            if let Some(pid) = Self::pgrep_exact(&candidate) {
                return Ok(pid);
            }
        }
        Err(CoreError::NotFound {
            code: NotFoundCode::ResourceMissing,
            resource_type: "macos_accessibility_application".to_string(),
            id: app_name.to_string(),
        })
    }

    /// Circuit breaker: check if calls are allowed.
    fn circuit_allows() -> bool {
        let failures = CONSECUTIVE_FAILURES.load(Ordering::Relaxed);
        if failures >= CIRCUIT_BREAKER_THRESHOLD {
            if !failures.is_multiple_of(CIRCUIT_BREAKER_RETRY_INTERVAL) {
                CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            warn!(
                "MacOsNativeAccessibility: circuit breaker retry after {} skipped",
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
            warn!(
                "MacOsNativeAccessibility: circuit breaker tripped after {CIRCUIT_BREAKER_THRESHOLD} consecutive failures"
            );
        }
    }

    /// Extract the focused element via AXUIElement API (synchronous).
    ///
    /// SAFETY: All CFTypeRef values are released after use. The function
    /// returns owned Rust strings -- no dangling Core Foundation references.
    fn extract_raw() -> Option<RawFocusedElement> {
        unsafe {
            let system_wide = AXUIElementCreateSystemWide();
            if system_wide.is_null() {
                return None;
            }

            // Build attribute key CFStrings
            let focused_attr = ax_attr(AX_FOCUSED_UI_ELEMENT_ATTR);

            // Get focused element
            let mut focused: CFTypeRef = ptr::null();
            let err =
                AXUIElementCopyAttributeValue(system_wide, as_cf_ref(&focused_attr), &mut focused);
            CFRelease(system_wide);

            if err != kAXErrorSuccess || focused.is_null() {
                if err == kAXErrorAPIDisabled {
                    warn!("Accessibility permission revoked at runtime; returning None");
                }
                return None;
            }

            // Extract role
            let role_key = ax_attr(AX_ROLE_ATTR);
            let role = Self::get_string_attr(focused, as_cf_ref(&role_key)).unwrap_or_default();

            // Extract title/description for label
            let title_key = ax_attr(AX_TITLE_ATTR);
            let desc_key = ax_attr(AX_DESCRIPTION_ATTR);
            let title = Self::get_string_attr(focused, as_cf_ref(&title_key))
                .or_else(|| Self::get_string_attr(focused, as_cf_ref(&desc_key)))
                .map(Zeroizing::new);

            // Extract value (raw text content) -- zeroized
            let value_key = ax_attr(AX_VALUE_ATTR);
            let value = Self::get_string_attr(focused, as_cf_ref(&value_key)).map(Zeroizing::new);

            // Extract placeholder
            let placeholder_key = ax_attr(AX_PLACEHOLDER_VALUE_ATTR);
            let placeholder = Self::get_string_attr(focused, as_cf_ref(&placeholder_key));

            // Extract position + size
            let position = Self::get_position_and_size(focused);

            CFRelease(focused);

            Some(RawFocusedElement {
                role,
                title,
                value,
                placeholder,
                position,
            })
        }
    }

    /// Helper: get a string attribute from an AXUIElement.
    unsafe fn get_string_attr(element: AXUIElementRef, attr: CFStringRef) -> Option<String> {
        let mut value: CFTypeRef = ptr::null();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut value);
        if err != kAXErrorSuccess || value.is_null() {
            return None;
        }
        // CFTypeRef -> CFStringRef -> CFString -> Rust String
        // AXUIElementCopyAttributeValue follows the "Create Rule":
        // the caller owns the returned CFTypeRef.
        let cf_str = CFString::wrap_under_create_rule(value as CFStringRef);
        Some(cf_str.to_string())
    }

    /// Helper: extract position (CGPoint) and size (CGSize) from element.
    unsafe fn get_position_and_size(element: AXUIElementRef) -> Option<ElementRect> {
        let pos_key = ax_attr(AX_POSITION_ATTR);
        let size_key = ax_attr(AX_SIZE_ATTR);

        let mut pos_ref: CFTypeRef = ptr::null();
        let mut size_ref: CFTypeRef = ptr::null();

        let pos_err = AXUIElementCopyAttributeValue(element, as_cf_ref(&pos_key), &mut pos_ref);
        let size_err = AXUIElementCopyAttributeValue(element, as_cf_ref(&size_key), &mut size_ref);

        if pos_err != kAXErrorSuccess || size_err != kAXErrorSuccess {
            if !pos_ref.is_null() {
                CFRelease(pos_ref);
            }
            if !size_ref.is_null() {
                CFRelease(size_ref);
            }
            return None;
        }

        let mut point = CGPoint::default();
        let mut size = CGSize::default();

        let got_point = AXValueGetValue(
            pos_ref,
            kAXValueCGPointType,
            &mut point as *mut _ as *mut std::ffi::c_void,
        );
        let got_size = AXValueGetValue(
            size_ref,
            kAXValueCGSizeType,
            &mut size as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(pos_ref);
        CFRelease(size_ref);

        if got_point && got_size {
            Some(ElementRect {
                x: point.x as f32,
                y: point.y as f32,
                width: size.width as f32,
                height: size.height as f32,
            })
        } else {
            None
        }
    }

    /// Fetch role, title, description, position, and size in a single IPC call
    /// using `AXUIElementCopyMultipleAttributeValues`.
    ///
    /// Returns `None` if the batch call fails (caller should fall back to
    /// individual `AXUIElementCopyAttributeValue` calls).
    ///
    /// SAFETY: Caller must ensure `element` is a valid AXUIElementRef.
    /// All returned CFTypeRef values are released within this function.
    unsafe fn batch_get_attributes(element: AXUIElementRef) -> Option<BatchAttributes> {
        use core_foundation::array::CFArray;
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;

        // Build the attribute names array: [AXRole, AXTitle, AXDescription, AXPosition, AXSize]
        let attr_role = ax_attr(AX_ROLE_ATTR);
        let attr_title = ax_attr(AX_TITLE_ATTR);
        let attr_desc = ax_attr(AX_DESCRIPTION_ATTR);
        let attr_pos = ax_attr(AX_POSITION_ATTR);
        let attr_size = ax_attr(AX_SIZE_ATTR);

        let attrs: CFArray<CFString> =
            CFArray::from_CFTypes(&[attr_role, attr_title, attr_desc, attr_pos, attr_size]);

        let mut values_ref: CFArrayRef = ptr::null();
        let err = AXUIElementCopyMultipleAttributeValues(
            element,
            attrs.as_concrete_TypeRef(),
            0, // default options: return kAXValueNotFound for missing attrs
            &mut values_ref,
        );

        if err != kAXErrorSuccess || values_ref.is_null() {
            return None;
        }

        let count = CFArrayGetCount(values_ref);
        if count < 5 {
            CFRelease(values_ref as CFTypeRef);
            return None;
        }

        // Helper: extract a String from a CFTypeRef that may be a CFString or
        // an error marker (kCFNull / AXValueNotFound sentinel).
        let extract_string = |idx: isize| -> Option<String> {
            let val = CFArrayGetValueAtIndex(values_ref, idx);
            if val.is_null() {
                return None;
            }
            // The batch API returns kCFNull for unsupported/missing attributes.
            // kCFNull has a different CFTypeID than CFString.
            let type_id = core_foundation_sys::base::CFGetTypeID(val);
            let string_type_id = core_foundation_sys::string::CFStringGetTypeID();
            if type_id != string_type_id {
                return None;
            }
            let cf_str = CFString::wrap_under_get_rule(val as CFStringRef);
            Some(cf_str.to_string())
        };

        // Index 0: role
        let role = extract_string(0).unwrap_or_default();
        // Index 1: title
        let title = extract_string(1);
        // Index 2: description
        let description = extract_string(2);

        // Index 3 & 4: position (AXValue<CGPoint>) and size (AXValue<CGSize>)
        let position_and_size = {
            let pos_val = CFArrayGetValueAtIndex(values_ref, 3);
            let size_val = CFArrayGetValueAtIndex(values_ref, 4);

            if pos_val.is_null() || size_val.is_null() {
                None
            } else {
                let mut point = CGPoint::default();
                let mut size = CGSize::default();

                let got_point = AXValueGetValue(
                    pos_val,
                    kAXValueCGPointType,
                    &mut point as *mut _ as *mut std::ffi::c_void,
                );
                let got_size = AXValueGetValue(
                    size_val,
                    kAXValueCGSizeType,
                    &mut size as *mut _ as *mut std::ffi::c_void,
                );

                if got_point && got_size {
                    Some(ElementRect {
                        x: point.x as f32,
                        y: point.y as f32,
                        width: size.width as f32,
                        height: size.height as f32,
                    })
                } else {
                    None
                }
            }
        };

        // Release the values array (the individual elements are not owned by us
        // since we used CFArrayGetValueAtIndex which follows the Get Rule).
        CFRelease(values_ref as CFTypeRef);

        Some(BatchAttributes {
            role,
            title,
            description,
            position_and_size,
        })
    }

    /// Recursively traverse the accessibility tree from an element.
    ///
    /// Uses `AXUIElementCopyMultipleAttributeValues` to fetch role, title,
    /// description, position, and size in a single IPC call per element
    /// (down from 4-5 individual calls). Falls back to individual
    /// `AXUIElementCopyAttributeValue` calls if the batch API returns an error.
    ///
    /// SAFETY: All CFTypeRef values are released. The function returns owned
    /// Rust data. `remaining` is decremented for each element collected to
    /// enforce the max_elements cap.
    unsafe fn traverse_tree(
        element: AXUIElementRef,
        depth: u32,
        max_depth: u32,
        remaining: &mut usize,
        pii_level: PiiFilterLevel,
    ) -> Vec<AccessibilityElement> {
        if depth > max_depth || *remaining == 0 {
            return Vec::new();
        }

        let mut results = Vec::new();

        // Try batch fetch first (1 IPC call instead of 4-5).
        let (role, label, bounds) = if let Some(batch) = Self::batch_get_attributes(element) {
            let lbl = if pii_level != PiiFilterLevel::Strict {
                batch.title.or(batch.description).unwrap_or_default()
            } else {
                String::new()
            };
            (batch.role, lbl, batch.position_and_size)
        } else {
            // Fallback: individual attribute fetches.
            let role_key = ax_attr(AX_ROLE_ATTR);
            let role = Self::get_string_attr(element, as_cf_ref(&role_key)).unwrap_or_default();

            let lbl = if pii_level != PiiFilterLevel::Strict {
                let title_key = ax_attr(AX_TITLE_ATTR);
                let desc_key = ax_attr(AX_DESCRIPTION_ATTR);
                Self::get_string_attr(element, as_cf_ref(&title_key))
                    .or_else(|| Self::get_string_attr(element, as_cf_ref(&desc_key)))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let bounds = Self::get_position_and_size(element);
            (role, lbl, bounds)
        };

        results.push(AccessibilityElement {
            role,
            label,
            bounds,
        });
        *remaining = remaining.saturating_sub(1);

        // Recurse into children
        if depth < max_depth && *remaining > 0 {
            let children_key = ax_attr(AX_CHILDREN_ATTR);
            let mut children_ref: CFTypeRef = ptr::null();
            let err =
                AXUIElementCopyAttributeValue(element, as_cf_ref(&children_key), &mut children_ref);
            if err == kAXErrorSuccess && !children_ref.is_null() {
                let count = CFArrayGetCount(children_ref as CFArrayRef);
                for i in 0..count {
                    if *remaining == 0 {
                        break;
                    }
                    let child = CFArrayGetValueAtIndex(children_ref as CFArrayRef, i);
                    if !child.is_null() {
                        let child_elements =
                            Self::traverse_tree(child, depth + 1, max_depth, remaining, pii_level);
                        results.extend(child_elements);
                    }
                }
                CFRelease(children_ref);
            }
        }

        results
    }

    /// Apply PII-level filtering to raw extracted data.
    fn filter_by_level(raw: RawFocusedElement, level: PiiFilterLevel) -> FocusedElementInfo {
        // Resolve the macOS label (title, falling back to placeholder), then
        // apply the shared per-level redaction — the single source of truth in
        // `crate::accessibility::pii_filter` (#5120).
        //
        // Strict exposes no label, so do NOT materialize it there: `title` is a
        // `Zeroizing<String>` and a `.to_string()` copy would be dropped
        // un-zeroed. Resolving it only when the level actually uses it keeps the
        // Strict path copy-free (#5131 follow-up).
        let label = if level == PiiFilterLevel::Strict {
            None
        } else {
            raw.title
                .as_deref()
                .map(|s| s.to_string())
                .or_else(|| raw.placeholder.clone())
        };
        super::super::pii_filter::apply_pii_level(
            raw.role,
            label,
            // `raw.value` is `Option<Zeroizing<String>>`; deref to `&str`.
            raw.value.as_deref().map(String::as_str),
            raw.position,
            level,
        )
        // raw.title and raw.value (Zeroizing<String>) are dropped here, zeroing
        // memory automatically.
    }
}

#[async_trait]
impl AccessibilityExtractor for MacOsNativeAccessibility {
    async fn extract_focused_element(
        &self,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Option<FocusedElementInfo>, CoreError> {
        if !Self::circuit_allows() {
            debug!("MacOsNativeAccessibility: circuit breaker open");
            return Ok(None);
        }

        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            debug!("PII Off requested but full_text_extraction consent missing; falling back to Standard");
            PiiFilterLevel::Standard
        } else {
            pii_level
        };

        // Run synchronous FFI on a blocking thread to avoid stalling tokio
        let result = tokio::task::spawn_blocking(Self::extract_raw)
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("AX blocking task failed: {e}"),
            })?;

        match result {
            Some(raw) => {
                Self::record_success();
                let filtered = Self::filter_by_level(raw, effective_level);
                debug!(role = %filtered.role, "AX focused element extracted");
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
        if !Self::check_permission() {
            return Err(CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PermissionDenied,
                message: "macOS Accessibility permission not granted. \
                 Enable in System Settings > Privacy & Security > Accessibility."
                    .to_string(),
            });
        }
        if !Self::circuit_allows() {
            return Ok(Vec::new());
        }

        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            PiiFilterLevel::Standard
        } else {
            pii_level
        };

        let result = tokio::task::spawn_blocking(move || unsafe {
            let system_wide = AXUIElementCreateSystemWide();
            if system_wide.is_null() {
                return Vec::new();
            }

            // Get focused element
            let focused_window_key = ax_attr(AX_FOCUSED_UI_ELEMENT_ATTR);
            let mut focused: CFTypeRef = ptr::null();
            let err = AXUIElementCopyAttributeValue(
                system_wide,
                as_cf_ref(&focused_window_key),
                &mut focused,
            );
            CFRelease(system_wide);

            if err != kAXErrorSuccess || focused.is_null() {
                return Vec::new();
            }

            // Try to get the window containing the focused element
            let window_key = ax_attr(AX_WINDOW_ATTR);
            let mut window_ref: CFTypeRef = ptr::null();
            let w_err =
                AXUIElementCopyAttributeValue(focused, as_cf_ref(&window_key), &mut window_ref);

            let traverse_root = if w_err == kAXErrorSuccess && !window_ref.is_null() {
                CFRelease(focused);
                window_ref
            } else {
                // Fallback: traverse from focused element itself
                focused
            };

            let mut remaining = max_elements;
            let elements =
                Self::traverse_tree(traverse_root, 0, max_depth, &mut remaining, effective_level);
            CFRelease(traverse_root);
            elements
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("AX tree traversal task failed: {e}"),
        })?;

        if result.is_empty() {
            Self::record_failure();
        } else {
            Self::record_success();
            debug!(count = result.len(), "AX window tree extracted");
        }

        Ok(result)
    }

    async fn extract_application_elements(
        &self,
        app_name: &str,
        max_depth: u32,
        max_elements: usize,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Vec<AccessibilityElement>, CoreError> {
        if !Self::check_permission() {
            return Err(CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PermissionDenied,
                message: "macOS Accessibility permission not granted. \
                 Enable in System Settings > Privacy & Security > Accessibility."
                    .to_string(),
            });
        }
        if !Self::circuit_allows() {
            return Ok(Vec::new());
        }

        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            PiiFilterLevel::Standard
        } else {
            pii_level
        };
        let app_name = app_name.to_string();

        let result =
            tokio::task::spawn_blocking(move || -> Result<Vec<AccessibilityElement>, CoreError> {
                unsafe {
                    let pid = Self::find_application_pid(&app_name)?;
                    let app_ref = AXUIElementCreateApplication(pid);
                    if app_ref.is_null() {
                        return Ok(Vec::new());
                    }

                    let mut remaining = max_elements;
                    let elements =
                        Self::traverse_tree(app_ref, 0, max_depth, &mut remaining, effective_level);
                    CFRelease(app_ref);
                    Ok(elements)
                }
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("AX application tree traversal task failed: {e}"),
            })??;

        if result.is_empty() {
            Self::record_failure();
        } else {
            Self::record_success();
            debug!(count = result.len(), "AX application tree extracted");
        }

        Ok(result)
    }

    fn has_permission(&self) -> bool {
        Self::check_permission()
    }

    fn name(&self) -> &str {
        "macos-native-accessibility"
    }

    fn request_permission(&self) -> bool {
        unsafe {
            use core_foundation::boolean::CFBoolean;
            use core_foundation::dictionary::CFDictionary;

            let key = CFString::new("AXTrustedCheckOptionPrompt");
            let value = CFBoolean::true_value();
            let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as CFTypeRef)
        }
    }
}
