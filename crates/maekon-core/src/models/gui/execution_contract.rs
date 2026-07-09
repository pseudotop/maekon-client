//! GUI real-input execution contract — declares which sandbox worker
//! execution kinds exist and enforces the invariants tying each kind to an
//! `GuiInputExecutionMode` / `GuiExecutionVerificationMode` pair.
//!
//! Split from `models/gui.rs` (issue #7721 F4). Pure move — no behavior change.

use serde::{Deserialize, Serialize};

use super::readiness::{GuiExecutionVerificationMode, GuiInputExecutionMode};

pub const GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION: &str = "automation.gui.real_input_execution.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSandboxWorkerExecutionKind {
    DryRunLogging,
    ExecutesRealInput,
    DelegatesToNativeInjector,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiSandboxWorkerExecutionContract {
    pub worker_id: String,
    pub execution_kind: GuiSandboxWorkerExecutionKind,
    pub input_execution_mode: GuiInputExecutionMode,
    pub verification_mode: GuiExecutionVerificationMode,
    pub requires_policy_approval: bool,
    pub emits_audit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiRealInputExecutionContract {
    pub schema_version: String,
    pub sandbox_worker_contracts: Vec<GuiSandboxWorkerExecutionContract>,
}

impl GuiRealInputExecutionContract {
    pub fn validate_contract_coverage(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION}"
            ));
        }

        for required in [
            GuiSandboxWorkerExecutionKind::DryRunLogging,
            GuiSandboxWorkerExecutionKind::ExecutesRealInput,
            GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector,
            GuiSandboxWorkerExecutionKind::Unsupported,
        ] {
            if !self
                .sandbox_worker_contracts
                .iter()
                .any(|contract| contract.execution_kind == required)
            {
                errors.push(format!("missing sandbox worker contract {required:?}"));
            }
        }

        for contract in &self.sandbox_worker_contracts {
            if contract.worker_id.trim().is_empty() {
                errors.push("sandbox worker contract must include worker_id".to_string());
            }
            match contract.execution_kind {
                GuiSandboxWorkerExecutionKind::DryRunLogging => {
                    if contract.input_execution_mode != GuiInputExecutionMode::DryRunWorker {
                        errors.push(format!(
                            "{} dry-run worker must report dry_run_worker mode",
                            contract.worker_id
                        ));
                    }
                    if contract.verification_mode
                        == GuiExecutionVerificationMode::ObservableStateChange
                    {
                        errors.push(format!(
                            "{} dry-run worker must not claim observable state change",
                            contract.worker_id
                        ));
                    }
                }
                GuiSandboxWorkerExecutionKind::ExecutesRealInput
                | GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector => {
                    if contract.input_execution_mode != GuiInputExecutionMode::SandboxedRealInput {
                        errors.push(format!(
                            "{} real sandbox worker must report sandboxed_real_input mode",
                            contract.worker_id
                        ));
                    }
                    if contract.verification_mode
                        != GuiExecutionVerificationMode::ObservableStateChange
                    {
                        errors.push(format!(
                            "{} real sandbox worker must prove observable state change",
                            contract.worker_id
                        ));
                    }
                    if !contract.requires_policy_approval || !contract.emits_audit {
                        errors.push(format!(
                            "{} real sandbox worker must require policy approval and audit",
                            contract.worker_id
                        ));
                    }
                }
                GuiSandboxWorkerExecutionKind::Unsupported => {
                    if contract.input_execution_mode != GuiInputExecutionMode::Unsupported {
                        errors.push(format!(
                            "{} unsupported worker must report unsupported mode",
                            contract.worker_id
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_real_input_execution_contract_fixture_distinguishes_dispatch_modes() {
        let contract: GuiRealInputExecutionContract = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-real-input-execution.v1.json"
        ))
        .unwrap();

        contract.validate_contract_coverage().unwrap();

        let dry_run = contract
            .sandbox_worker_contracts
            .iter()
            .find(|contract| {
                contract.execution_kind == GuiSandboxWorkerExecutionKind::DryRunLogging
            })
            .unwrap();
        assert_eq!(
            dry_run.input_execution_mode,
            GuiInputExecutionMode::DryRunWorker
        );
        assert_eq!(
            dry_run.verification_mode,
            GuiExecutionVerificationMode::CommandAccepted
        );

        let delegated_real_input = contract
            .sandbox_worker_contracts
            .iter()
            .find(|contract| {
                contract.execution_kind == GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector
            })
            .unwrap();
        assert_eq!(
            delegated_real_input.input_execution_mode,
            GuiInputExecutionMode::SandboxedRealInput
        );
        assert_eq!(
            delegated_real_input.verification_mode,
            GuiExecutionVerificationMode::ObservableStateChange
        );
    }
}
