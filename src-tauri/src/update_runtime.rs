use maekon_core::config::UpdateConfig;
use maekon_web::update_control::{UpdateAction, UpdateControl};
use tokio::runtime::Handle;

use crate::update_coordinator;

pub(crate) struct UpdateRuntimeBundle {
    pub(crate) update_control: UpdateControl,
    pub(crate) update_action_tx: tokio::sync::mpsc::UnboundedSender<UpdateAction>,
}

pub(crate) struct UpdateRuntimeBuilder<'a> {
    config: &'a UpdateConfig,
    runtime_handle: &'a Handle,
}

impl<'a> UpdateRuntimeBuilder<'a> {
    pub(crate) fn new(config: &'a UpdateConfig, runtime_handle: &'a Handle) -> Self {
        Self {
            config,
            runtime_handle,
        }
    }

    pub(crate) fn build_and_spawn(&self) -> UpdateRuntimeBundle {
        let runtime_auto_update = self.config.auto_install;
        let (update_action_tx, update_action_rx) =
            tokio::sync::mpsc::unbounded_channel::<UpdateAction>();
        let update_control = UpdateControl::new(
            update_action_tx.clone(),
            update_coordinator::initial_status(self.config, runtime_auto_update),
        );

        if self.config.enabled {
            let update_config = self.config.clone();
            let update_state = update_control.state.clone();
            let update_status_tx = Some(update_control.event_tx.clone());
            self.runtime_handle.spawn(async move {
                update_coordinator::run_update_coordinator(
                    update_config,
                    update_state,
                    update_action_rx,
                    update_status_tx,
                    runtime_auto_update,
                )
                .await;
            });
        }

        UpdateRuntimeBundle {
            update_control,
            update_action_tx,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn update_runtime_uses_coordinator_as_single_startup_status_writer() {
        let source = include_str!("update_runtime.rs");
        let timeout_call = concat!("tokio::time", "::timeout");
        let direct_probe_call = concat!("check_for", "_updates(),");

        assert!(
            !source.contains(timeout_call) && !source.contains(direct_probe_call),
            "UpdateRuntimeBuilder must not run its own update probe; the coordinator is the single UpdateStatus writer"
        );
    }
}
