//! Outbound server port for standalone WBS XLSX projection and receipts (#10358).

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::wbs_xlsx::{
    EffectiveWbsXlsxProjectionResolution, LocalWbsXlsxReceipt, UploadedWbsXlsxReceipt,
    WbsXlsxHandoff,
};

#[async_trait]
pub trait WbsXlsxClient: Send + Sync {
    async fn resolve_handoff(
        &self,
        assignment_receipt_id: &str,
    ) -> Result<WbsXlsxHandoff, CoreError>;

    async fn resolve_effective_projection(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<EffectiveWbsXlsxProjectionResolution, CoreError>;

    async fn append_local_receipt(
        &self,
        organization_id: &str,
        receipt: &LocalWbsXlsxReceipt,
    ) -> Result<UploadedWbsXlsxReceipt, CoreError>;
}
