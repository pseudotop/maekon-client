//! Raw native cell primitive for the gated WBS runtime (#12132, ADR-002).

use async_trait::async_trait;
use std::time::Instant;

use crate::models::wbs_cell_apply::{
    WbsBoundCellReadback, WbsCellApplyRequest, WbsCellTargetError, WbsCellTargetSnapshot,
    WbsCellWriteOutcome,
};

/// No method grants consent, policy approval or an execution capability. Only the
/// runtime may compose this port after its current privacy/audit/session/ticket gates.
/// It must not be exposed directly through Tauri/web handlers or general Suggestion/Run.
#[async_trait]
pub trait WbsCellTargetPort: Send + Sync {
    /// Bind a supported single selection in an explicitly allowed, running workbook.
    /// Never create an app, open a document or scan other workbooks as a fallback.
    async fn capture_selection(&self) -> Result<WbsCellTargetSnapshot, WbsCellTargetError>;

    /// Recheck the original native registration, active row/cell and exact old text.
    /// Unsupported multi-cell/merged/formula/protected/read-only cells refuse writes.
    /// Validation and a subsequent write are not an atomic compare-and-set promise.
    async fn validate_bound_cell(
        &self,
        target: &WbsCellTargetSnapshot,
    ) -> Result<(), WbsCellTargetError>;

    /// Independently read the original cell even after selection/focus changes.
    /// Do not compare with `before_value`, echo a write argument, or retarget the active cell.
    async fn read_bound_cell(
        &self,
        target: &WbsCellTargetSnapshot,
    ) -> Result<WbsBoundCellReadback, WbsCellTargetError>;

    /// Revalidate immediately before a single literal-text setter on the bound cell.
    /// `deadline` is the caller's monotonic authority expiry; implementations must
    /// fail closed before starting the setter when it has elapsed.
    /// Keep a bounded duplicate-request ledger; changed payloads with reused IDs refuse.
    /// No save, capacity reservation, assignment confirmation or email side effects.
    /// A queued cancellation proves no write; in-flight cancellation is OutcomeUnknown.
    /// WrittenUnverified/AlreadyAttempted require independent readback, never blind retry.
    async fn apply_bound_cell(
        &self,
        request: &WbsCellApplyRequest,
        deadline: Instant,
    ) -> Result<WbsCellWriteOutcome, WbsCellTargetError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::wbs_assignment_candidates::WbsAssignmentContext;
    use crate::models::wbs_cell_apply::{WbsCellAnchor, WbsCellBinding, WbsCellIdentity};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn context(item: &str) -> TestResult<WbsAssignmentContext> {
        Ok(WbsAssignmentContext::new(
            "org-fixture".into(),
            item.into(),
            Some("v2".into()),
        )?)
    }

    fn identity(row: u32) -> TestResult<WbsCellIdentity> {
        Ok(WbsCellIdentity::new(
            42,
            7,
            "workbook-session".into(),
            "sheet-session".into(),
            row,
            5,
            context("wbs-row-a")?,
        )?)
    }

    fn snapshot(value: &str) -> TestResult<WbsCellTargetSnapshot> {
        Ok(WbsCellTargetSnapshot::new(
            WbsCellBinding::new("binding-a".into(), 1)?,
            identity(5)?,
            value.into(),
            None,
        )?)
    }

    #[test]
    fn readback_accepts_new_value_but_rejects_same_text_from_another_cell() -> TestResult {
        let target = snapshot("")?;
        let request = WbsCellApplyRequest::new("request-a".into(), target.clone(), "Kim".into())?;
        let actual = WbsBoundCellReadback::new(
            target.binding().clone(),
            target.identity().clone(),
            "Kim".into(),
        )?;
        assert_eq!(actual.verify_request(&request), Ok(()));
        assert_ne!(actual.value(), target.before_value());
        let other =
            WbsBoundCellReadback::new(target.binding().clone(), identity(6)?, "Kim".into())?;
        assert_eq!(
            other.verify_request(&request),
            Err(WbsCellTargetError::StaleTarget)
        );
        let wrong_value = WbsBoundCellReadback::new(
            target.binding().clone(),
            target.identity().clone(),
            "Lee".into(),
        )?;
        assert_eq!(
            wrong_value.verify_request(&request),
            Err(WbsCellTargetError::ReadbackMismatch)
        );
        Ok(())
    }

    #[test]
    fn old_value_comparison_is_exact_and_never_normalizes_whitespace() -> TestResult {
        let original = snapshot(" Kim ")?;
        assert_eq!(original.ensure_unchanged(&original.clone()), Ok(()));
        for value in ["Kim", " Lee ", ""] {
            assert_eq!(
                original.ensure_unchanged(&snapshot(value)?),
                Err(WbsCellTargetError::StaleTarget)
            );
        }
        let moved_anchor = WbsCellTargetSnapshot::new(
            original.binding().clone(),
            original.identity().clone(),
            original.before_value().into(),
            Some(WbsCellAnchor::new(-100, 50, 80, 20, 144)?),
        )?;
        assert_eq!(original.ensure_unchanged(&moved_anchor), Ok(()));
        assert_ne!(original, moved_anchor);
        Ok(())
    }

    #[test]
    fn identity_changes_reject_even_when_address_or_value_looks_identical() -> TestResult {
        let original = snapshot("Kim")?;
        let variants = [
            WbsCellIdentity::new(
                43,
                7,
                "workbook-session".into(),
                "sheet-session".into(),
                5,
                5,
                context("wbs-row-a")?,
            )?,
            WbsCellIdentity::new(
                42,
                8,
                "workbook-session".into(),
                "sheet-session".into(),
                5,
                5,
                context("wbs-row-a")?,
            )?,
            WbsCellIdentity::new(
                42,
                7,
                "reopened-workbook".into(),
                "sheet-session".into(),
                5,
                5,
                context("wbs-row-a")?,
            )?,
            WbsCellIdentity::new(
                42,
                7,
                "workbook-session".into(),
                "other-sheet".into(),
                5,
                5,
                context("wbs-row-a")?,
            )?,
            WbsCellIdentity::new(
                42,
                7,
                "workbook-session".into(),
                "sheet-session".into(),
                5,
                6,
                context("wbs-row-a")?,
            )?,
            WbsCellIdentity::new(
                42,
                7,
                "workbook-session".into(),
                "sheet-session".into(),
                5,
                5,
                context("wbs-row-b")?,
            )?,
            identity(6)?,
        ];
        for changed in variants {
            let current = WbsCellTargetSnapshot::new(
                original.binding().clone(),
                changed,
                "Kim".into(),
                None,
            )?;
            assert_eq!(
                original.ensure_unchanged(&current),
                Err(WbsCellTargetError::StaleTarget)
            );
        }
        let regenerated = WbsCellTargetSnapshot::new(
            WbsCellBinding::new("binding-a".into(), 2)?,
            original.identity().clone(),
            "Kim".into(),
            None,
        )?;
        assert_eq!(
            original.ensure_unchanged(&regenerated),
            Err(WbsCellTargetError::StaleTarget)
        );
        Ok(())
    }

    #[test]
    fn literal_assignee_boundary_rejects_formula_and_control_prefixes() -> TestResult {
        let target = snapshot("")?;
        for invalid in [
            "",
            "=SUM(A1:A2)",
            "+1",
            "-1",
            "@cmd",
            "'hidden",
            " =1",
            "\t=1",
            "name\nother",
            "x\0y",
            "name ",
        ] {
            assert_eq!(
                WbsCellApplyRequest::new("request-a".into(), target.clone(), invalid.into()),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        for valid in ["Kim + Lee".to_owned(), "홍".repeat(85), "a".repeat(256)] {
            let request =
                WbsCellApplyRequest::new("request-a".into(), target.clone(), valid.clone())?;
            assert_eq!(request.assignee_text(), valid);
            assert_eq!(request.target(), &target);
            assert_eq!(request.request_id(), "request-a");
        }
        assert_eq!(
            WbsCellApplyRequest::new("request-a".into(), target, "홍".repeat(86)),
            Err(WbsCellTargetError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn bounds_accept_the_boundary_and_reject_the_next_value() -> TestResult {
        let boundary_binding = WbsCellBinding::new("x".repeat(128), 1)?;
        assert_eq!(boundary_binding.binding_id(), "x".repeat(128));
        assert_eq!(boundary_binding.generation(), 1);
        for (id, generation) in [("x".repeat(129), 1), ("x".into(), 0), ("a/b".into(), 1)] {
            assert_eq!(
                WbsCellBinding::new(id, generation),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        let boundary_snapshot = snapshot(&"a".repeat(1_024))?;
        assert_eq!(boundary_snapshot.before_value(), "a".repeat(1_024));
        let error = snapshot(&"a".repeat(1_025))
            .expect_err("snapshot text beyond the byte limit must be rejected");
        assert_eq!(
            error.downcast_ref::<WbsCellTargetError>(),
            Some(&WbsCellTargetError::InvalidInput)
        );
        assert_eq!(identity(1_048_576)?.row(), 1_048_576);
        for row in [0, 1_048_577] {
            let error = identity(row).expect_err("row outside workbook bounds must be rejected");
            assert_eq!(
                error.downcast_ref::<WbsCellTargetError>(),
                Some(&WbsCellTargetError::InvalidInput)
            );
        }
        for column in [0, 16_385] {
            assert_eq!(
                WbsCellIdentity::new(
                    42,
                    7,
                    "book".into(),
                    "sheet".into(),
                    5,
                    column,
                    context("row")?
                ),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        let boundary_anchor = WbsCellAnchor::new(-1_000_000, 1_000_000, 1, 32_768, 960)?;
        assert_eq!(boundary_anchor.x(), -1_000_000);
        assert_eq!(boundary_anchor.y(), 1_000_000);
        assert_eq!(boundary_anchor.width(), 1);
        assert_eq!(boundary_anchor.height(), 32_768);
        assert_eq!(boundary_anchor.dpi(), 960);
        for (width, dpi) in [(0, 96), (32_769, 96), (1, 47), (1, 961)] {
            assert_eq!(
                WbsCellAnchor::new(0, 0, width, 1, dpi),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn native_identity_requires_a_real_process_and_bounded_workbook_address() -> TestResult {
        let ctx = context("identity-boundary")?;
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
        let book = "b".repeat(128);
        let sheet = "s".repeat(128);
        let high = make(42, 7, &book, &sheet, 1_048_576, 16_384)?;
        assert_eq!(high.process_id(), 42);
        assert_eq!(high.process_started_at(), 7);
        assert_eq!(high.workbook_instance_id(), book);
        assert_eq!(high.worksheet_id(), sheet);
        assert_eq!((high.row(), high.column()), (1_048_576, 16_384));
        assert_eq!(high.context(), &ctx);
        let low = make(1, 1, "b", "s", 1, 1)?;
        assert_eq!((low.row(), low.column()), (1, 1));
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
        Ok(())
    }

    #[test]
    fn physical_anchor_preserves_both_monitor_origins_and_rejects_outside_bounds() -> TestResult {
        for expected in [
            (-1_000_000, 1_000_000, 1, 32_768, 48),
            (1_000_000, -1_000_000, 32_768, 1, 960),
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
            let target = snapshot("")?;
            let anchored = WbsCellTargetSnapshot::new(
                target.binding().clone(),
                target.identity().clone(),
                "".into(),
                Some(anchor),
            )?;
            assert_eq!(anchored.anchor(), Some(anchor));
            assert_eq!(target.anchor(), None);
            assert_eq!(target.ensure_unchanged(&anchored), Ok(()));
        }
        for (x, y, height) in [
            (-1_000_001, 0, 1),
            (1_000_001, 0, 1),
            (0, -1_000_001, 1),
            (0, 1_000_001, 1),
            (0, 0, 0),
            (0, 0, 32_769),
        ] {
            assert_eq!(
                WbsCellAnchor::new(x, y, 1, height, 96),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn same_cell_and_text_from_a_different_registration_are_still_stale() -> TestResult {
        let target = snapshot("Kim")?;
        let request = WbsCellApplyRequest::new("request-a".into(), target.clone(), "Kim".into())?;
        let valid = WbsBoundCellReadback::new(
            target.binding().clone(),
            target.identity().clone(),
            "Kim".into(),
        )?;
        assert_eq!(valid.binding(), target.binding());
        assert_eq!(valid.identity(), target.identity());
        assert_eq!(valid.verify_request(&request), Ok(()));
        for binding in [
            WbsCellBinding::new("binding-b".into(), 1)?,
            WbsCellBinding::new("binding-a".into(), 2)?,
        ] {
            let read = WbsBoundCellReadback::new(
                binding.clone(),
                target.identity().clone(),
                "Kim".into(),
            )?;
            assert_eq!(
                read.verify_request(&request),
                Err(WbsCellTargetError::StaleTarget)
            );
            let current =
                WbsCellTargetSnapshot::new(binding, target.identity().clone(), "Kim".into(), None)?;
            assert_eq!(
                target.ensure_unchanged(&current),
                Err(WbsCellTargetError::StaleTarget)
            );
        }
        Ok(())
    }

    #[test]
    fn observed_values_keep_exact_bytes_and_reject_controls_or_oversized_readback() -> TestResult {
        let target = snapshot("")?;
        for value in [
            "".to_owned(),
            " ".into(),
            "a".repeat(1_024),
            "한".repeat(341) + "a",
        ] {
            let read = WbsBoundCellReadback::new(
                target.binding().clone(),
                target.identity().clone(),
                value.clone(),
            )?;
            assert_eq!(read.value(), value);
            assert_eq!(snapshot(&value)?.before_value(), value);
        }
        for value in [
            "a".repeat(1_025),
            "한".repeat(342),
            "\0".into(),
            "\t".into(),
            "\n".into(),
            "\u{7f}".into(),
        ] {
            assert_eq!(
                WbsBoundCellReadback::new(
                    target.binding().clone(),
                    target.identity().clone(),
                    value.clone()
                ),
                Err(WbsCellTargetError::InvalidInput)
            );
            assert_eq!(
                WbsCellTargetSnapshot::new(
                    target.binding().clone(),
                    target.identity().clone(),
                    value,
                    None
                ),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn write_request_identity_is_bounded_independently_of_the_selected_text() -> TestResult {
        let target = snapshot("")?;
        let id = "r".repeat(128);
        let valid = WbsCellApplyRequest::new(id.clone(), target.clone(), "a".repeat(256))?;
        assert_eq!(valid.request_id(), id);
        assert_eq!(valid.target(), &target);
        assert_eq!(valid.assignee_text().len(), 256);
        for invalid in ["".to_owned(), "r".repeat(129), "request/path".into()] {
            assert_eq!(
                WbsCellApplyRequest::new(invalid, target.clone(), "Kim".into()),
                Err(WbsCellTargetError::InvalidInput)
            );
        }
        assert_eq!(
            WbsCellApplyRequest::new(id, target, "a".repeat(257)),
            Err(WbsCellTargetError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn debug_does_not_reveal_binding_document_or_assignee_text() -> TestResult {
        let target = snapshot("Private Assignee")?;
        let request = WbsCellApplyRequest::new(
            "secret-request".into(),
            target.clone(),
            "Private Candidate".into(),
        )?;
        let read = WbsBoundCellReadback::new(
            target.binding().clone(),
            target.identity().clone(),
            "Private Candidate".into(),
        )?;
        let output = format!(
            "{:?} {:?} {target:?} {request:?} {read:?}",
            target.binding(),
            target.identity()
        );
        for private in [
            "binding-a",
            "workbook-session",
            "sheet-session",
            "wbs-row-a",
            "Private Assignee",
            "Private Candidate",
            "secret-request",
        ] {
            assert!(!output.contains(private));
        }
        assert!(output.contains("REDACTED"));
        assert_ne!(
            WbsCellTargetError::CancelledBeforeStart,
            WbsCellTargetError::OutcomeUnknown
        );
        Ok(())
    }
}
