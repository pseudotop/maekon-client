//! Durable local receipt spool for standalone WBS XLSX output (#10358).

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::wbs_xlsx::LocalWbsXlsxReceipt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingWbsXlsxReceipt {
    pub organization_id: String,
    pub receipt: LocalWbsXlsxReceipt,
}

#[async_trait]
pub trait WbsXlsxReceiptStore: Send + Sync {
    async fn append_pending(
        &self,
        organization_id: &str,
        receipt: &LocalWbsXlsxReceipt,
    ) -> Result<(), CoreError>;

    async fn list_pending(&self) -> Result<Vec<PendingWbsXlsxReceipt>, CoreError>;

    async fn mark_uploaded(&self, receipt_id: &str, uploaded_at: &str) -> Result<(), CoreError>;
}
