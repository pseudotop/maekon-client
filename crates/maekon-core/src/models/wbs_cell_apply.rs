//! Bound native WBS cell primitives (#12132). These values grant no execution rights.
//!
//! The gated runtime owns consent, policy, audit, capability and signed-ticket checks.
//! There is deliberately no deserialization or arbitrary document-path input here.

use std::fmt;

use super::wbs_assignment_candidates::{valid_id, valid_text, WbsAssignmentContext};

pub const MAX_WBS_CELL_TEXT_BYTES: usize = 1_024;
pub const MAX_WBS_ASSIGNEE_BYTES: usize = 256;

/// A backend registration, never a capability. IDs are scoped to one adapter instance.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WbsCellBinding {
    binding_id: String,
    generation: u64,
}

impl WbsCellBinding {
    pub fn new(binding_id: String, generation: u64) -> Result<Self, WbsCellTargetError> {
        if !valid_id(&binding_id) || generation == 0 {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            binding_id,
            generation,
        })
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Portable identity for one cell. Workbook/sheet IDs are opaque instance identities,
/// not file names. Reopening the same path must produce a different workbook identity.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCellIdentity {
    process_id: u32,
    process_started_at: u64,
    workbook_instance_id: String,
    worksheet_id: String,
    row: u32,
    column: u32,
    context: WbsAssignmentContext,
}

impl WbsCellIdentity {
    pub fn new(
        process_id: u32,
        process_started_at: u64,
        workbook_instance_id: String,
        worksheet_id: String,
        row: u32,
        column: u32,
        context: WbsAssignmentContext,
    ) -> Result<Self, WbsCellTargetError> {
        if process_id == 0
            || process_started_at == 0
            || !valid_id(&workbook_instance_id)
            || !valid_id(&worksheet_id)
            || !(1..=1_048_576).contains(&row)
            || !(1..=16_384).contains(&column)
        {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            process_id,
            process_started_at,
            workbook_instance_id,
            worksheet_id,
            row,
            column,
            context,
        })
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    /// Exact process-start stamp supplied by the OS, never a local clock estimate.
    pub fn process_started_at(&self) -> u64 {
        self.process_started_at
    }
    pub fn workbook_instance_id(&self) -> &str {
        &self.workbook_instance_id
    }
    pub fn worksheet_id(&self) -> &str {
        &self.worksheet_id
    }
    /// One-based row in the actual workbook, not a WBS catalog sequence number.
    pub fn row(&self) -> u32 {
        self.row
    }
    pub fn column(&self) -> u32 {
        self.column
    }
    pub fn context(&self) -> &WbsAssignmentContext {
        &self.context
    }
}

/// Virtual-desktop physical pixels, including negative multi-monitor origins.
/// `dpi` is the verified monitor DPI. Missing/unverified conversion is represented
/// by `None` on a snapshot, not by inventing a default 96-DPI anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WbsCellAnchor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    dpi: u32,
}

impl WbsCellAnchor {
    pub fn new(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        dpi: u32,
    ) -> Result<Self, WbsCellTargetError> {
        if !(-1_000_000..=1_000_000).contains(&x)
            || !(-1_000_000..=1_000_000).contains(&y)
            || !(1..=32_768).contains(&width)
            || !(1..=32_768).contains(&height)
            || !(48..=960).contains(&dpi)
        {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
            dpi,
        })
    }

    pub fn x(&self) -> i32 {
        self.x
    }
    pub fn y(&self) -> i32 {
        self.y
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn dpi(&self) -> u32 {
        self.dpi
    }
}

/// An observed single, plain-text assignee cell. Adapters retain the native object
/// behind `binding`; changing selection must never retarget this snapshot.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCellTargetSnapshot {
    binding: WbsCellBinding,
    identity: WbsCellIdentity,
    before_value: String,
    anchor: Option<WbsCellAnchor>,
}

impl WbsCellTargetSnapshot {
    pub fn new(
        binding: WbsCellBinding,
        identity: WbsCellIdentity,
        before_value: String,
        anchor: Option<WbsCellAnchor>,
    ) -> Result<Self, WbsCellTargetError> {
        if !valid_text(&before_value, MAX_WBS_CELL_TEXT_BYTES, true) {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            binding,
            identity,
            before_value,
            anchor,
        })
    }

    pub fn binding(&self) -> &WbsCellBinding {
        &self.binding
    }
    pub fn identity(&self) -> &WbsCellIdentity {
        &self.identity
    }
    pub fn before_value(&self) -> &str {
        &self.before_value
    }
    pub fn anchor(&self) -> Option<WbsCellAnchor> {
        self.anchor
    }

    /// Compare a fresh observation of the original registration. Anchor movement
    /// alone is not a cell mutation; the runtime separately validates popup/focus.
    pub fn ensure_unchanged(&self, current: &Self) -> Result<(), WbsCellTargetError> {
        if self.binding != current.binding
            || self.identity != current.identity
            || self.before_value != current.before_value
        {
            return Err(WbsCellTargetError::StaleTarget);
        }
        Ok(())
    }
}

/// Raw adapter input after runtime approval. The text must originate from the
/// selected, eligible, request-bound candidate; this constructor does not approve it.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCellApplyRequest {
    request_id: String,
    target: WbsCellTargetSnapshot,
    assignee_text: String,
}

impl WbsCellApplyRequest {
    pub fn new(
        request_id: String,
        target: WbsCellTargetSnapshot,
        assignee_text: String,
    ) -> Result<Self, WbsCellTargetError> {
        if !valid_id(&request_id)
            || !valid_text(&assignee_text, MAX_WBS_ASSIGNEE_BYTES, false)
            || assignee_text.trim() != assignee_text
            || assignee_text.starts_with(['=', '+', '-', '@', '\''])
        {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            request_id,
            target,
            assignee_text,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn target(&self) -> &WbsCellTargetSnapshot {
        &self.target
    }
    pub fn assignee_text(&self) -> &str {
        &self.assignee_text
    }
}

/// A new read of the bound cell, independent of both active selection and the
/// setter's return value. `value` need not equal the snapshot's old value.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsBoundCellReadback {
    binding: WbsCellBinding,
    identity: WbsCellIdentity,
    value: String,
}

impl WbsBoundCellReadback {
    pub fn new(
        binding: WbsCellBinding,
        identity: WbsCellIdentity,
        value: String,
    ) -> Result<Self, WbsCellTargetError> {
        if !valid_text(&value, MAX_WBS_CELL_TEXT_BYTES, true) {
            return Err(WbsCellTargetError::InvalidInput);
        }
        Ok(Self {
            binding,
            identity,
            value,
        })
    }

    pub fn binding(&self) -> &WbsCellBinding {
        &self.binding
    }
    pub fn identity(&self) -> &WbsCellIdentity {
        &self.identity
    }
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Evidence of same-cell readback only, not proof of atomicity or authorization.
    pub fn verify_request(&self, request: &WbsCellApplyRequest) -> Result<(), WbsCellTargetError> {
        if self.binding != request.target.binding || self.identity != request.target.identity {
            return Err(WbsCellTargetError::StaleTarget);
        }
        if self.value != request.assignee_text {
            return Err(WbsCellTargetError::ReadbackMismatch);
        }
        Ok(())
    }
}

/// Neither variant claims a verified cell update. Do not automatically retry an
/// attempted write. A duplicate request must have exactly the original payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbsCellWriteOutcome {
    WrittenUnverified,
    AlreadyAttempted,
}

/// Safe, bounded reasons. Never include document paths, cell text or COM errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WbsCellTargetError {
    #[error("wbs_cell_unsupported")]
    Unsupported,
    #[error("wbs_cell_unavailable")]
    Unavailable,
    #[error("wbs_cell_invalid_input")]
    InvalidInput,
    #[error("wbs_cell_stale_target")]
    StaleTarget,
    #[error("wbs_cell_read_only")]
    ReadOnly,
    #[error("wbs_cell_protected")]
    Protected,
    #[error("wbs_cell_unsupported_selection")]
    UnsupportedSelection,
    #[error("wbs_cell_formula")]
    FormulaCell,
    /// The worker proved the request never started; no native write occurred.
    #[error("wbs_cell_cancelled_before_start")]
    CancelledBeforeStart,
    /// The worker started. Cancellation/timeout does not prove that a write stopped.
    #[error("wbs_cell_outcome_unknown")]
    OutcomeUnknown,
    #[error("wbs_cell_readback_mismatch")]
    ReadbackMismatch,
    /// Use only when the adapter can prove the setter did not mutate the cell.
    #[error("wbs_cell_write_failed")]
    WriteFailed,
}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => { $(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    WbsCellBinding,
    WbsCellIdentity,
    WbsCellTargetSnapshot,
    WbsCellApplyRequest,
    WbsBoundCellReadback
);

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn context() -> TestResult<WbsAssignmentContext> {
        Ok(WbsAssignmentContext::new(
            "org-fixture".into(),
            "private-wbs-row".into(),
            Some("revision-2".into()),
        )?)
    }

    #[test]
    fn registration_preserves_nondefault_generation_and_rejects_invalid_ids() -> TestResult {
        let id = "b".repeat(128);
        for generation in [1, 2, u64::MAX] {
            let binding = WbsCellBinding::new(id.clone(), generation)?;
            assert_eq!(binding.binding_id(), id);
            assert_eq!(binding.generation(), generation);
        }
        assert_ne!(
            WbsCellBinding::new("binding-a".into(), 1)?,
            WbsCellBinding::new("binding-a".into(), 2)?
        );
        assert_eq!(
            WbsCellBinding::new("binding-a".into(), 0),
            Err(WbsCellTargetError::InvalidInput)
        );
        for invalid in ["".to_owned(), "b".repeat(129), "binding/path".into()] {
            assert_eq!(
                WbsCellBinding::new(invalid, 2),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn native_identity_keeps_process_instance_context_and_workbook_boundaries() -> TestResult {
        let ctx = context()?;
        let book = "b".repeat(128);
        let sheet = "s".repeat(128);
        let high = WbsCellIdentity::new(
            42,
            7,
            book.clone(),
            sheet.clone(),
            1_048_576,
            16_384,
            ctx.clone(),
        )?;
        assert_eq!(high.process_id(), 42);
        assert_eq!(high.process_started_at(), 7);
        assert_eq!(high.workbook_instance_id(), book);
        assert_eq!(high.worksheet_id(), sheet);
        assert_eq!((high.row(), high.column()), (1_048_576, 16_384));
        assert_eq!(high.context(), &ctx);
        let low = WbsCellIdentity::new(1, 1, "b".into(), "s".into(), 1, 1, ctx)?;
        assert_eq!((low.row(), low.column()), (1, 1));
        Ok(())
    }

    #[test]
    fn native_identity_rejects_missing_process_and_invalid_document_addresses() -> TestResult {
        let ctx = context()?;
        let make = |pid, started, book: &str, sheet: &str, row, column| {
            WbsCellIdentity::new(
                pid,
                started,
                book.into(),
                sheet.into(),
                row,
                column,
                ctx.clone(),
            )
        };
        for (pid, started) in [(0, 1), (1, 0)] {
            assert_eq!(
                make(pid, started, "b", "s", 1, 1),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        for invalid in ["".to_owned(), "b".repeat(129), "book/path".into()] {
            assert_eq!(
                make(1, 1, &invalid, "s", 1, 1),
                Err(WbsCellTargetError::InvalidInput)
            );
            assert_eq!(
                make(1, 1, "b", &invalid, 1, 1),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        for (row, column) in [(0, 1), (1_048_577, 1), (1, 0), (1, 16_385)] {
            assert_eq!(
                make(1, 1, "b", "s", row, column),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn physical_anchor_keeps_multimonitor_origins_and_explicit_dpi_bounds() -> TestResult {
        for expected in [
            (-1_000_000, 1_000_000, 1, 32_768, 48),
            (1_000_000, -1_000_000, 32_768, 1, 960),
            (-100, 50, 80, 20, 144),
        ] {
            let (x, y, width, height, dpi) = expected;
            let anchor = WbsCellAnchor::new(x, y, width, height, dpi)?;
            assert_eq!(
                (
                    anchor.x(),
                    anchor.y(),
                    anchor.width(),
                    anchor.height(),
                    anchor.dpi()
                ),
                expected
            );
        }
        for (x, y, width, height, dpi) in [
            (-1_000_001, 0, 1, 1, 96),
            (1_000_001, 0, 1, 1, 96),
            (0, -1_000_001, 1, 1, 96),
            (0, 1_000_001, 1, 1, 96),
            (0, 0, 0, 1, 96),
            (0, 0, 32_769, 1, 96),
            (0, 0, 1, 0, 96),
            (0, 0, 1, 32_769, 96),
            (0, 0, 1, 1, 47),
            (0, 0, 1, 1, 961),
        ] {
            assert_eq!(
                WbsCellAnchor::new(x, y, width, height, dpi),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn diagnostic_formatting_redacts_registration_and_document_context() -> TestResult {
        let binding = WbsCellBinding::new("private-binding".into(), 2)?;
        let identity = WbsCellIdentity::new(
            42,
            7,
            "private-book".into(),
            "private-sheet".into(),
            5,
            5,
            context()?,
        )?;
        assert_eq!(format!("{binding:?}"), "WbsCellBinding([REDACTED])");
        assert_eq!(format!("{identity:?}"), "WbsCellIdentity([REDACTED])");
        assert_ne!(
            WbsCellTargetError::CancelledBeforeStart,
            WbsCellTargetError::OutcomeUnknown
        );
        Ok(())
    }
}
