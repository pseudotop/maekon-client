use axum::{
    extract::{Path, State},
    Json,
};
use oneshim_api_contracts::integration::{
    IntegrationDeviceAuthorizationCommandResult, IntegrationDeviceAuthorizationFlowRequest,
    IntegrationInboxActionResponse, IntegrationInboxDismissRequest,
    IntegrationInboxRefreshResponse, IntegrationInboxResponse, IntegrationStatus,
};

use crate::{
    error::ApiError,
    services::{
        integration_service::{
            IntegrationAuditQueryService, IntegrationAuthCommandService,
            IntegrationInboxCommandService, IntegrationStatusQueryService,
        },
        web_contexts::IntegrationWebContext,
    },
};

pub async fn get_status(State(context): State<IntegrationWebContext>) -> Json<IntegrationStatus> {
    Json(
        IntegrationStatusQueryService::new(context)
            .build_status()
            .await,
    )
}

pub async fn get_audit(
    State(context): State<IntegrationWebContext>,
) -> Json<oneshim_api_contracts::integration::IntegrationAuditLogResponse> {
    Json(
        IntegrationAuditQueryService::new(context)
            .build_audit_log(50)
            .await,
    )
}

pub async fn list_inbox(
    State(context): State<IntegrationWebContext>,
) -> Result<Json<IntegrationInboxResponse>, ApiError> {
    Ok(Json(
        IntegrationInboxCommandService::new(context)
            .list_inbox()
            .await?,
    ))
}

pub async fn refresh_inbox(
    State(context): State<IntegrationWebContext>,
) -> Result<Json<IntegrationInboxRefreshResponse>, ApiError> {
    Ok(Json(
        IntegrationInboxCommandService::new(context)
            .refresh_inbox()
            .await?,
    ))
}

pub async fn acknowledge_inbox_prompt(
    State(context): State<IntegrationWebContext>,
    Path(prompt_id): Path<String>,
) -> Result<Json<IntegrationInboxActionResponse>, ApiError> {
    Ok(Json(
        IntegrationInboxCommandService::new(context)
            .acknowledge_inbox_prompt(&prompt_id)
            .await?,
    ))
}

pub async fn dismiss_inbox_prompt(
    State(context): State<IntegrationWebContext>,
    Path(prompt_id): Path<String>,
    Json(request): Json<IntegrationInboxDismissRequest>,
) -> Result<Json<IntegrationInboxActionResponse>, ApiError> {
    Ok(Json(
        IntegrationInboxCommandService::new(context)
            .dismiss_inbox_prompt(&prompt_id, request)
            .await?,
    ))
}

pub async fn get_auth_status(
    State(context): State<IntegrationWebContext>,
) -> Result<Json<oneshim_core::models::integration::IntegrationAuthStatus>, ApiError> {
    Ok(Json(
        IntegrationAuthCommandService::new(context)
            .get_auth_status()
            .await?,
    ))
}

pub async fn start_device_authorization(
    State(context): State<IntegrationWebContext>,
) -> Result<Json<IntegrationDeviceAuthorizationCommandResult>, ApiError> {
    Ok(Json(
        IntegrationAuthCommandService::new(context)
            .start_device_authorization()
            .await?,
    ))
}

pub async fn poll_device_authorization(
    State(context): State<IntegrationWebContext>,
    Json(request): Json<IntegrationDeviceAuthorizationFlowRequest>,
) -> Result<Json<IntegrationDeviceAuthorizationCommandResult>, ApiError> {
    Ok(Json(
        IntegrationAuthCommandService::new(context)
            .poll_device_authorization(&request.flow_id)
            .await?,
    ))
}

pub async fn cancel_device_authorization(
    State(context): State<IntegrationWebContext>,
    Json(request): Json<IntegrationDeviceAuthorizationFlowRequest>,
) -> Result<Json<IntegrationDeviceAuthorizationCommandResult>, ApiError> {
    Ok(Json(
        IntegrationAuthCommandService::new(context)
            .cancel_device_authorization(&request.flow_id)
            .await?,
    ))
}

pub async fn reset_auth_state(
    State(context): State<IntegrationWebContext>,
) -> Result<Json<IntegrationDeviceAuthorizationCommandResult>, ApiError> {
    Ok(Json(
        IntegrationAuthCommandService::new(context)
            .reset_auth_state()
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::integration_service::{
        INTEGRATION_AUDIT_SCHEMA_VERSION, INTEGRATION_INBOX_ACTION_SCHEMA_VERSION,
        INTEGRATION_INBOX_SCHEMA_VERSION,
    };
    use crate::AppState;
    use async_trait::async_trait;
    use oneshim_api_contracts::integration::{
        IntegrationDeviceAuthorizationFlowRequest, IntegrationOutboundRuntimeStatus,
    };
    use oneshim_core::error::CoreError;
    use oneshim_core::models::integration::{
        IntegrationAckCursor, IntegrationAuthContext, IntegrationAuthProfileKind,
        IntegrationAuthScheme, IntegrationAuthStatus, IntegrationAuthStatusKind,
        IntegrationCapabilityScope, IntegrationDeviceAuthorizationFlow,
        IntegrationEgressDisposition, IntegrationEnvelope, IntegrationInboxItemStatus,
        IntegrationInsightAuditRecord, IntegrationPrivacyClassification,
        IntegrationRuntimeLaneTelemetry, IntegrationRuntimeTelemetry, IntegrationSessionState,
        IntegrationSessionStatus, IntegrationTransportKind, ProactivePrompt,
        ProactivePromptCategory, ProactivePromptPriority, PromptProvenance, StoredProactivePrompt,
    };
    use oneshim_core::ports::integration::{
        IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
        IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
    };
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::{broadcast, Mutex};

    struct TestSessionPort(Option<IntegrationSessionState>);

    #[async_trait]
    impl IntegrationSessionPort for TestSessionPort {
        async fn connect(
            &self,
            _requested_scopes: Vec<IntegrationCapabilityScope>,
        ) -> Result<IntegrationSessionState, CoreError> {
            self.current_session()
                .await?
                .ok_or_else(|| CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message: "no session".to_string(),
                })
        }

        async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
            Ok(self.0.clone())
        }

        async fn heartbeat(&self, _session_id: &str) -> Result<IntegrationSessionState, CoreError> {
            self.connect(Vec::new()).await
        }

        async fn store_ack_cursor(
            &self,
            _session_id: &str,
            _cursor: IntegrationAckCursor,
        ) -> Result<IntegrationSessionState, CoreError> {
            self.connect(Vec::new()).await
        }

        async fn disconnect(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct TestOutbox {
        pending_count: usize,
        last_ack_cursor: Option<IntegrationAckCursor>,
    }

    struct TestAuthPort {
        status: Arc<Mutex<IntegrationAuthStatus>>,
    }

    #[async_trait]
    impl IntegrationAuthPort for TestAuthPort {
        async fn resolve_session_auth(
            &self,
            _requested_scopes: &[IntegrationCapabilityScope],
            _resource_indicator: Option<&str>,
        ) -> Result<IntegrationAuthContext, CoreError> {
            Ok(IntegrationAuthContext {
                access_token: "integration-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: Some("https://integration.example.com".to_string()),
            })
        }

        async fn current_auth_status(&self) -> Result<IntegrationAuthStatus, CoreError> {
            Ok(self.status.lock().await.clone())
        }

        async fn start_device_authorization(
            &self,
            requested_scopes: &[IntegrationCapabilityScope],
            resource_indicator: Option<&str>,
        ) -> Result<IntegrationDeviceAuthorizationFlow, CoreError> {
            let flow = IntegrationDeviceAuthorizationFlow {
                flow_id: "flow-1".to_string(),
                user_code: "ABCD-EFGH".to_string(),
                verification_uri: "https://verify.example.com".to_string(),
                verification_uri_complete: Some(
                    "https://verify.example.com?user_code=ABCD-EFGH".to_string(),
                ),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                interval_secs: 5,
                requested_scopes: requested_scopes.to_vec(),
                resource_indicator: resource_indicator.map(str::to_string),
            };
            *self.status.lock().await = IntegrationAuthStatus {
                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                status: IntegrationAuthStatusKind::AwaitingUserAuthorization,
                interactive: true,
                authenticated: false,
                expires_at: None,
                resource_indicator: resource_indicator.map(str::to_string),
                pending_flow: Some(flow.clone()),
                message: Some("authorize the device".to_string()),
            };
            Ok(flow)
        }

        async fn poll_device_authorization(
            &self,
            flow_id: &str,
        ) -> Result<IntegrationAuthStatus, CoreError> {
            let mut status = self.status.lock().await;
            if status
                .pending_flow
                .as_ref()
                .map(|flow| flow.flow_id.as_str())
                != Some(flow_id)
            {
                return Err(CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_device_flow".to_string(),
                    id: flow_id.to_string(),
                });
            }
            status.status = IntegrationAuthStatusKind::Ready;
            status.authenticated = true;
            status.pending_flow = None;
            status.message = None;
            Ok(status.clone())
        }

        async fn cancel_device_authorization(&self, flow_id: &str) -> Result<(), CoreError> {
            let mut status = self.status.lock().await;
            if status
                .pending_flow
                .as_ref()
                .map(|flow| flow.flow_id.as_str())
                != Some(flow_id)
            {
                return Err(CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_device_flow".to_string(),
                    id: flow_id.to_string(),
                });
            }
            status.status = IntegrationAuthStatusKind::Unauthenticated;
            status.pending_flow = None;
            status.message = Some("device authorization cancelled".to_string());
            Ok(())
        }

        async fn reset_auth_state(&self) -> Result<(), CoreError> {
            let mut status = self.status.lock().await;
            status.status = IntegrationAuthStatusKind::Unauthenticated;
            status.authenticated = false;
            status.pending_flow = None;
            status.message = Some("integration auth state reset".to_string());
            Ok(())
        }
    }

    #[async_trait]
    impl IntegrationOutboxPort for TestOutbox {
        async fn enqueue_message(
            &self,
            _envelope: IntegrationEnvelope,
            _payload: oneshim_core::models::integration::IntegrationOutboundPayload,
        ) -> Result<String, CoreError> {
            Ok("queue-1".to_string())
        }

        async fn list_pending(
            &self,
            _limit: usize,
        ) -> Result<Vec<oneshim_core::models::integration::QueuedIntegrationEgressMessage>, CoreError>
        {
            Ok(Vec::new())
        }

        async fn pending_count(&self) -> Result<usize, CoreError> {
            Ok(self.pending_count)
        }

        async fn delete(&self, _queue_ids: &[String]) -> Result<(), CoreError> {
            Ok(())
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(self.last_ack_cursor.clone())
        }

        async fn store_ack_cursor(&self, _cursor: IntegrationAckCursor) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct TestInboxStore {
        pending_count: usize,
        last_ack_cursor: Option<IntegrationAckCursor>,
    }

    #[async_trait]
    impl IntegrationInboxStorePort for TestInboxStore {
        async fn upsert_prompts(
            &self,
            _prompts: Vec<StoredProactivePrompt>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            Ok(Vec::new())
        }

        async fn list_unpresented(
            &self,
            _limit: usize,
        ) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            Ok(Vec::new())
        }

        async fn pending_count(&self) -> Result<usize, CoreError> {
            Ok(self.pending_count)
        }

        async fn mark_presented(
            &self,
            _prompt_id: &str,
            _presented_at: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), CoreError> {
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
            Ok(self.last_ack_cursor.clone())
        }

        async fn store_ack_cursor(&self, _cursor: IntegrationAckCursor) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct TestInboxPort {
        prompts: Arc<Mutex<Vec<StoredProactivePrompt>>>,
    }

    #[async_trait]
    impl IntegrationInboxPort for TestInboxPort {
        async fn refresh(&self) -> Result<usize, CoreError> {
            Ok(self.prompts.lock().await.len())
        }

        async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            Ok(self.prompts.lock().await.clone())
        }

        async fn acknowledge(&self, prompt_id: &str) -> Result<(), CoreError> {
            let mut prompts = self.prompts.lock().await;
            let prompt = prompts
                .iter_mut()
                .find(|prompt| prompt.prompt.prompt_id == prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?;
            prompt.status = IntegrationInboxItemStatus::Acknowledged;
            Ok(())
        }

        async fn dismiss(&self, prompt_id: &str, reason: Option<String>) -> Result<(), CoreError> {
            let mut prompts = self.prompts.lock().await;
            let prompt = prompts
                .iter_mut()
                .find(|prompt| prompt.prompt.prompt_id == prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?;
            prompt.status = IntegrationInboxItemStatus::Dismissed;
            prompt.dismiss_reason = reason;
            Ok(())
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }
    }

    struct TestAuditPort(Vec<IntegrationInsightAuditRecord>);

    #[async_trait]
    impl IntegrationAuditPort for TestAuditPort {
        async fn record_insight_decision(
            &self,
            _record: IntegrationInsightAuditRecord,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn recent_insight_decisions(
            &self,
            _limit: usize,
        ) -> Result<Vec<IntegrationInsightAuditRecord>, CoreError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Clone)]
    struct TestTelemetryPort(IntegrationRuntimeTelemetry);

    #[async_trait]
    impl IntegrationRuntimeTelemetryPort for TestTelemetryPort {
        async fn snapshot(&self) -> Result<IntegrationRuntimeTelemetry, CoreError> {
            Ok(self.0.clone())
        }
    }

    fn test_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let inbox_prompts = Arc::new(Mutex::new(vec![StoredProactivePrompt {
            prompt: ProactivePrompt {
                prompt_id: "prompt-1".to_string(),
                category: ProactivePromptCategory::Reminder,
                title: "Review insight".to_string(),
                body: "A prompt arrived from integration.".to_string(),
                priority: ProactivePromptPriority::Medium,
                actions: Vec::new(),
                expires_at: None,
                provenance: PromptProvenance {
                    source_system: "integration-server".to_string(),
                    source_actor: Some("scheduler".to_string()),
                    correlation_id: Some("corr-1".to_string()),
                },
            },
            received_at: chrono::Utc::now(),
            status: IntegrationInboxItemStatus::Pending,
            status_updated_at: chrono::Utc::now(),
            presented_at: None,
            dismiss_reason: None,
        }]));
        let mut state = AppState::with_core(storage, event_tx);
        state.integration.runtime_status = Some(IntegrationOutboundRuntimeStatus {
            enabled: true,
            bootstrap_configured: true,
            auth_source_configured: true,
            auth_material_available: false,
            runtime_configured: true,
            resource_indicator_configured: true,
            auth_profile_kind:
                oneshim_core::models::integration::IntegrationAuthProfileKind::EnvToken,
            preferred_transports: vec![IntegrationTransportKind::WebSocket],
            supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
            outbox_pending_count: None,
            inbox_pending_count: None,
            outbox_ack_cursor: None,
            inbox_ack_cursor: None,
            auth_status: None,
            current_session: None,
            runtime_telemetry: None,
        });
        state.integration.auth = Some(Arc::new(TestAuthPort {
            status: Arc::new(Mutex::new(IntegrationAuthStatus {
                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                status: IntegrationAuthStatusKind::Unauthenticated,
                interactive: true,
                authenticated: false,
                expires_at: None,
                resource_indicator: Some("https://integration.example.com".to_string()),
                pending_flow: None,
                message: Some("authorize the device".to_string()),
            })),
        }) as Arc<dyn IntegrationAuthPort>);
        state.integration.session = Some(Arc::new(TestSessionPort(Some(IntegrationSessionState {
            session_id: "session-1".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: IntegrationTransportKind::WebSocket,
            auth_scheme: IntegrationAuthScheme::BearerToken,
            connected_at: None,
            last_heartbeat_at: None,
            requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            ack_cursors: Vec::new(),
        }))) as Arc<dyn IntegrationSessionPort>);
        state.integration.outbox = Some(Arc::new(TestOutbox {
            pending_count: 3,
            last_ack_cursor: Some(IntegrationAckCursor {
                stream_id: "insights".to_string(),
                cursor: "cursor-outbox".to_string(),
                acknowledged_at: chrono::Utc::now(),
            }),
        }) as Arc<dyn IntegrationOutboxPort>);
        state.integration.inbox = Some(Arc::new(TestInboxPort {
            prompts: inbox_prompts,
        }) as Arc<dyn IntegrationInboxPort>);
        state.integration.inbox_store = Some(Arc::new(TestInboxStore {
            pending_count: 2,
            last_ack_cursor: Some(IntegrationAckCursor {
                stream_id: "prompts".to_string(),
                cursor: "cursor-inbox".to_string(),
                acknowledged_at: chrono::Utc::now(),
            }),
        }) as Arc<dyn IntegrationInboxStorePort>);
        state.integration.audit =
            Some(Arc::new(TestAuditPort(vec![IntegrationInsightAuditRecord {
                record_id: "audit-1".to_string(),
                envelope_id: "env-1".to_string(),
                packet_id: "packet-1".to_string(),
                disposition: IntegrationEgressDisposition::Allow,
                reason: None,
                privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                capability_scope: IntegrationCapabilityScope::InsightWrite,
                occurred_at: chrono::Utc::now(),
            }])) as Arc<dyn IntegrationAuditPort>);
        state.integration.runtime_telemetry =
            Some(Arc::new(TestTelemetryPort(IntegrationRuntimeTelemetry {
                connect: IntegrationRuntimeLaneTelemetry {
                    consecutive_failures: 2,
                    last_success_at: None,
                    last_failure_at: Some(chrono::Utc::now()),
                    backoff_until: Some(chrono::Utc::now()),
                    last_error: Some("connect failed".to_string()),
                },
                heartbeat: IntegrationRuntimeLaneTelemetry::default(),
                egress: IntegrationRuntimeLaneTelemetry {
                    consecutive_failures: 0,
                    last_success_at: Some(chrono::Utc::now()),
                    last_failure_at: None,
                    backoff_until: None,
                    last_error: None,
                },
                inbox: IntegrationRuntimeLaneTelemetry::default(),
            })) as Arc<dyn IntegrationRuntimeTelemetryPort>);
        state
    }

    fn test_context() -> IntegrationWebContext {
        IntegrationWebContext::from_state(&test_state())
    }

    #[tokio::test]
    async fn get_status_merges_runtime_snapshot_and_current_session() {
        let response = get_status(State(test_context())).await.0;

        assert!(response.outbound_runtime.enabled);
        assert!(response.outbound_runtime.runtime_configured);
        assert_eq!(
            response
                .outbound_runtime
                .current_session
                .as_ref()
                .map(|session| session.status.clone()),
            Some(IntegrationSessionStatus::Connected)
        );
        assert_eq!(
            response
                .outbound_runtime
                .current_session
                .as_ref()
                .map(|session| session.granted_scopes.clone()),
            Some(vec!["insight:write".to_string()])
        );
        assert_eq!(response.outbound_runtime.outbox_pending_count, Some(3));
        assert_eq!(response.outbound_runtime.inbox_pending_count, Some(2));
        assert_eq!(
            response
                .outbound_runtime
                .outbox_ack_cursor
                .as_ref()
                .map(|cursor| cursor.stream_id.as_str()),
            Some("insights")
        );
        assert_eq!(
            response
                .outbound_runtime
                .inbox_ack_cursor
                .as_ref()
                .map(|cursor| cursor.stream_id.as_str()),
            Some("prompts")
        );
        assert_eq!(
            response
                .outbound_runtime
                .runtime_telemetry
                .as_ref()
                .map(|telemetry| telemetry.connect.consecutive_failures),
            Some(2)
        );
        assert_eq!(
            response
                .outbound_runtime
                .runtime_telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.connect.last_error.as_deref()),
            Some("connect failed")
        );
        assert!(response
            .outbound_runtime
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.egress.last_success_at)
            .is_some());
    }

    #[tokio::test]
    async fn get_audit_returns_recent_integration_records() {
        let response = get_audit(State(test_context())).await.0;

        assert_eq!(response.schema_version, INTEGRATION_AUDIT_SCHEMA_VERSION);
        assert_eq!(response.records.len(), 1);
        assert_eq!(response.records[0].record_id, "audit-1");
        assert_eq!(response.records[0].disposition, "allow");
        assert_eq!(
            response.records[0].privacy_classification,
            "derived_summary"
        );
    }

    #[tokio::test]
    async fn list_inbox_returns_pending_prompts() {
        let response = list_inbox(State(test_context())).await.unwrap().0;

        assert_eq!(response.schema_version, INTEGRATION_INBOX_SCHEMA_VERSION);
        assert_eq!(response.pending_count, 1);
        assert_eq!(response.prompts.len(), 1);
        assert_eq!(response.prompts[0].prompt_id, "prompt-1");
        assert_eq!(response.prompts[0].status, "pending");
    }

    #[tokio::test]
    async fn acknowledge_and_dismiss_inbox_prompt_return_action_status() {
        let ack_response =
            acknowledge_inbox_prompt(State(test_context()), Path("prompt-1".to_string()))
                .await
                .unwrap()
                .0;
        assert_eq!(
            ack_response.schema_version,
            INTEGRATION_INBOX_ACTION_SCHEMA_VERSION
        );
        assert_eq!(ack_response.status, "acknowledged");

        let dismiss_response = dismiss_inbox_prompt(
            State(test_context()),
            Path("prompt-1".to_string()),
            Json(IntegrationInboxDismissRequest {
                reason: Some("handled locally".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(dismiss_response.status, "dismissed");
    }

    #[tokio::test]
    async fn auth_handlers_roundtrip_device_authorization_flow() {
        let state = test_state();
        let context = IntegrationWebContext::from_state(&state);

        let auth_status = get_auth_status(State(context.clone())).await.unwrap().0;
        assert_eq!(
            auth_status.status,
            IntegrationAuthStatusKind::Unauthenticated
        );

        let start_response = start_device_authorization(State(context.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(
            start_response.auth_status.status,
            IntegrationAuthStatusKind::AwaitingUserAuthorization
        );
        assert!(start_response
            .flow
            .as_ref()
            .is_some_and(|flow| flow.requested_scopes.len() >= 4));

        let poll_response = poll_device_authorization(
            State(context),
            Json(IntegrationDeviceAuthorizationFlowRequest {
                flow_id: "flow-1".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            poll_response.auth_status.status,
            IntegrationAuthStatusKind::Ready
        );
    }

    #[tokio::test]
    async fn reset_auth_state_clears_pending_device_authorization_flow() {
        let state = test_state();
        let context = IntegrationWebContext::from_state(&state);

        let start_response = start_device_authorization(State(context.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(
            start_response.auth_status.status,
            IntegrationAuthStatusKind::AwaitingUserAuthorization
        );

        let reset_response = reset_auth_state(State(context)).await.unwrap().0;
        assert_eq!(
            reset_response.auth_status.status,
            IntegrationAuthStatusKind::Unauthenticated
        );
        assert!(reset_response.flow.is_none());
    }
}
