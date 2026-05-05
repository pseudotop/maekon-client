mod commands;
mod queries;

pub(crate) const INTEGRATION_STATUS_SCHEMA_VERSION: &str = "integration.status.v1";
pub(crate) const INTEGRATION_AUDIT_SCHEMA_VERSION: &str = "integration.audit.v1";
pub(crate) const INTEGRATION_INBOX_SCHEMA_VERSION: &str = "integration.inbox.v1";
pub(crate) const INTEGRATION_INBOX_ACTION_SCHEMA_VERSION: &str = "integration.inbox-action.v1";

pub use commands::{IntegrationAuthCommandService, IntegrationInboxCommandService};
pub use queries::{IntegrationAuditQueryService, IntegrationStatusQueryService};
