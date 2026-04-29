use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    ProactivePromptCategory, ProactivePromptPriority, StoredProactivePrompt,
};
use oneshim_core::ports::integration::{IntegrationInboxStorePort, IntegrationPromptPresenterPort};
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::watch;
use tracing::{debug, warn};

const INTEGRATION_PROMPT_EVENT: &str = "integration-proactive-prompt";

#[derive(Debug, Clone, Serialize)]
struct IntegrationPromptEventPayload {
    prompt_id: String,
    category: String,
    priority: String,
    title: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<DateTime<Utc>>,
}

fn category_label(category: &ProactivePromptCategory) -> &'static str {
    match category {
        ProactivePromptCategory::Insight => "insight",
        ProactivePromptCategory::Task => "task",
        ProactivePromptCategory::Reminder => "reminder",
        ProactivePromptCategory::Escalation => "escalation",
    }
}

fn priority_label(priority: &ProactivePromptPriority) -> &'static str {
    match priority {
        ProactivePromptPriority::Low => "low",
        ProactivePromptPriority::Medium => "medium",
        ProactivePromptPriority::High => "high",
        ProactivePromptPriority::Critical => "critical",
    }
}

fn prompt_payload(prompt: &StoredProactivePrompt) -> IntegrationPromptEventPayload {
    IntegrationPromptEventPayload {
        prompt_id: prompt.prompt.prompt_id.clone(),
        category: category_label(&prompt.prompt.category).to_string(),
        priority: priority_label(&prompt.prompt.priority).to_string(),
        title: prompt.prompt.title.clone(),
        body: prompt.prompt.body.clone(),
        source_system: Some(prompt.prompt.provenance.source_system.clone()),
        source_actor: prompt.prompt.provenance.source_actor.clone(),
        expires_at: prompt.prompt.expires_at,
    }
}

pub struct TauriIntegrationPromptPresenter {
    app_handle: tauri::AppHandle,
}

impl TauriIntegrationPromptPresenter {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

#[async_trait]
impl IntegrationPromptPresenterPort for TauriIntegrationPromptPresenter {
    async fn present_prompt(&self, prompt: &StoredProactivePrompt) -> Result<(), CoreError> {
        let payload = prompt_payload(prompt);
        self.app_handle
            .emit(INTEGRATION_PROMPT_EVENT, &payload)
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("emit integration prompt event: {err}"),
            })?;

        if let Err(err) = tauri_plugin_notification::NotificationExt::notification(&self.app_handle)
            .builder()
            .title(&prompt.prompt.title)
            .body(&prompt.prompt.body)
            .show()
        {
            debug!(error = %err, "integration prompt notification delivery failed");
        }

        Ok(())
    }
}

pub struct IntegrationInboxDeliveryCoordinator {
    inbox_store: Arc<dyn IntegrationInboxStorePort>,
    presenter: Arc<dyn IntegrationPromptPresenterPort>,
    max_batch_size: usize,
}

impl IntegrationInboxDeliveryCoordinator {
    pub fn new(
        inbox_store: Arc<dyn IntegrationInboxStorePort>,
        presenter: Arc<dyn IntegrationPromptPresenterPort>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            inbox_store,
            presenter,
            max_batch_size: max_batch_size.max(1),
        }
    }

    pub async fn deliver_pending(&self) -> Result<usize, CoreError> {
        let prompts = self
            .inbox_store
            .list_unpresented(self.max_batch_size)
            .await?;
        let mut delivered = 0usize;

        for prompt in prompts {
            self.presenter.present_prompt(&prompt).await?;
            self.inbox_store
                .mark_presented(&prompt.prompt.prompt_id, Utc::now())
                .await?;
            delivered += 1;
        }

        Ok(delivered)
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationInboxDeliveryLoopProfile {
    pub delivery_interval: Duration,
}

impl Default for IntegrationInboxDeliveryLoopProfile {
    fn default() -> Self {
        Self {
            delivery_interval: Duration::from_secs(15),
        }
    }
}

#[derive(Clone)]
pub struct IntegrationInboxDeliveryLoop {
    delivery: Arc<IntegrationInboxDeliveryCoordinator>,
    profile: IntegrationInboxDeliveryLoopProfile,
}

impl IntegrationInboxDeliveryLoop {
    pub fn new(
        delivery: Arc<IntegrationInboxDeliveryCoordinator>,
        profile: IntegrationInboxDeliveryLoopProfile,
    ) -> Self {
        Self { delivery, profile }
    }

    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let mut delivery_interval = tokio::time::interval(self.profile.delivery_interval);
        delivery_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = delivery_interval.tick() => {
                    if let Err(error) = self.delivery.deliver_pending().await {
                        warn!(error = %error, "integration prompt delivery cycle failed");
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use chrono::Duration as ChronoDuration;
    use tokio::sync::Mutex;

    use super::*;
    use oneshim_core::models::integration::{
        IntegrationAckCursor, IntegrationInboxItemStatus, ProactivePrompt, PromptProvenance,
    };

    struct MockInboxStore {
        prompts: Arc<Mutex<BTreeMap<String, StoredProactivePrompt>>>,
    }

    #[async_trait]
    impl IntegrationInboxStorePort for MockInboxStore {
        async fn upsert_prompts(
            &self,
            prompts: Vec<StoredProactivePrompt>,
        ) -> Result<(), CoreError> {
            let mut guard = self.prompts.lock().await;
            for prompt in prompts {
                guard.insert(prompt.prompt.prompt_id.clone(), prompt);
            }
            Ok(())
        }

        async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            Ok(self.prompts.lock().await.values().cloned().collect())
        }

        async fn list_unpresented(
            &self,
            limit: usize,
        ) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            let mut prompts: Vec<_> = self
                .prompts
                .lock()
                .await
                .values()
                .filter(|prompt| {
                    prompt.status == IntegrationInboxItemStatus::Pending
                        && prompt.presented_at.is_none()
                })
                .cloned()
                .collect();
            prompts.sort_by_key(|prompt| prompt.received_at);
            prompts.truncate(limit);
            Ok(prompts)
        }

        async fn pending_count(&self) -> Result<usize, CoreError> {
            Ok(self.prompts.lock().await.len())
        }

        async fn mark_presented(
            &self,
            prompt_id: &str,
            presented_at: DateTime<Utc>,
        ) -> Result<(), CoreError> {
            let mut guard = self.prompts.lock().await;
            guard
                .get_mut(prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?
                .presented_at = Some(presented_at);
            Ok(())
        }

        async fn update_status(
            &self,
            _prompt_id: &str,
            _status: IntegrationInboxItemStatus,
            _reason: Option<String>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn expire_stale(&self) -> Result<usize, CoreError> {
            Ok(0)
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }

        async fn store_ack_cursor(&self, _cursor: IntegrationAckCursor) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct MockPresenter {
        prompt_ids: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl IntegrationPromptPresenterPort for MockPresenter {
        async fn present_prompt(&self, prompt: &StoredProactivePrompt) -> Result<(), CoreError> {
            self.prompt_ids
                .lock()
                .await
                .push(prompt.prompt.prompt_id.clone());
            Ok(())
        }
    }

    fn sample_prompt(prompt_id: &str) -> StoredProactivePrompt {
        StoredProactivePrompt {
            prompt: ProactivePrompt {
                prompt_id: prompt_id.to_string(),
                category: ProactivePromptCategory::Reminder,
                title: format!("Prompt {prompt_id}"),
                body: "Review the latest insight".to_string(),
                priority: ProactivePromptPriority::Medium,
                actions: Vec::new(),
                expires_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                provenance: PromptProvenance {
                    source_system: "integration-server".to_string(),
                    source_actor: Some("scheduler".to_string()),
                    correlation_id: Some(format!("corr-{prompt_id}")),
                },
            },
            received_at: Utc::now(),
            status: IntegrationInboxItemStatus::Pending,
            status_updated_at: Utc::now(),
            presented_at: None,
            dismiss_reason: None,
        }
    }

    #[tokio::test]
    async fn delivery_coordinator_marks_presented_after_successful_delivery() {
        let store = Arc::new(MockInboxStore {
            prompts: Arc::new(Mutex::new(BTreeMap::from([
                ("prompt-1".to_string(), sample_prompt("prompt-1")),
                ("prompt-2".to_string(), sample_prompt("prompt-2")),
            ]))),
        });
        let presenter = Arc::new(MockPresenter {
            prompt_ids: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator =
            IntegrationInboxDeliveryCoordinator::new(store.clone(), presenter.clone(), 10);

        let delivered = coordinator.deliver_pending().await.unwrap();

        assert_eq!(delivered, 2);
        assert_eq!(
            presenter.prompt_ids.lock().await.clone(),
            vec!["prompt-1".to_string(), "prompt-2".to_string()]
        );
        assert!(store
            .prompts
            .lock()
            .await
            .values()
            .all(|prompt| prompt.presented_at.is_some()));
    }

    #[tokio::test]
    async fn delivery_coordinator_uses_received_at_order_for_limited_batches() {
        let mut newer = sample_prompt("prompt-9");
        let mut older = sample_prompt("prompt-1");
        older.received_at = Utc::now() - ChronoDuration::minutes(5);
        newer.received_at = Utc::now();

        let store = Arc::new(MockInboxStore {
            prompts: Arc::new(Mutex::new(BTreeMap::from([
                ("prompt-9".to_string(), newer),
                ("prompt-1".to_string(), older),
            ]))),
        });
        let presenter = Arc::new(MockPresenter {
            prompt_ids: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator =
            IntegrationInboxDeliveryCoordinator::new(store.clone(), presenter.clone(), 1);

        let delivered = coordinator.deliver_pending().await.unwrap();

        assert_eq!(delivered, 1);
        assert_eq!(
            presenter.prompt_ids.lock().await.clone(),
            vec!["prompt-1".to_string()]
        );
    }
}
