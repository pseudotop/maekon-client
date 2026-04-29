use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use oneshim_core::models::gui::{
    GuiInteractionSession, GuiSessionEvent, GuiSessionState, HighlightRequest, HighlightTarget,
};
use oneshim_core::models::intent::ElementBounds;
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::focus_probe::FocusProbe;
use oneshim_core::ports::overlay_driver::OverlayDriver;

use super::crypto::new_capability_token;
use super::helpers::{build_candidates, is_expired, map_core_error};
use super::types::{
    GuiCreateSessionRequest, GuiCreateSessionResponse, GuiHighlightRequest, GuiInteractionError,
    StoredSession,
};
use super::{
    CLEANUP_INTERVAL_SECS, DEFAULT_MAX_CANDIDATES, DEFAULT_MIN_CONFIDENCE,
    DEFAULT_SESSION_TTL_SECS, GUI_EVENT_CHANNEL_CAPACITY, GUI_HMAC_SECRET_ENV,
};
use tracing::debug;

pub struct GuiInteractionService {
    pub(super) scene_finder: Arc<dyn ElementFinder>,
    pub(super) focus_probe: Arc<dyn FocusProbe>,
    pub(super) overlay_driver: Arc<dyn OverlayDriver>,
    pub(super) sessions: RwLock<HashMap<String, StoredSession>>,
    pub(super) event_tx: broadcast::Sender<GuiSessionEvent>,
    cleanup_started: AtomicBool,
    pub(super) hmac_secret: Option<Vec<u8>>,
}

impl GuiInteractionService {
    pub fn new(
        scene_finder: Arc<dyn ElementFinder>,
        focus_probe: Arc<dyn FocusProbe>,
        overlay_driver: Arc<dyn OverlayDriver>,
        hmac_secret: Option<String>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(GUI_EVENT_CHANNEL_CAPACITY);
        Self {
            scene_finder,
            focus_probe,
            overlay_driver,
            sessions: RwLock::new(HashMap::new()),
            event_tx,
            cleanup_started: AtomicBool::new(false),
            hmac_secret: hmac_secret
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(|value| value.into_bytes()),
        }
    }

    pub fn ensure_cleanup_task(self: &Arc<Self>) {
        if self.cleanup_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let weak = Arc::downgrade(self);
        // Guard: tokio::spawn panics if no runtime context exists (e.g., called
        // from Tauri setup before the async runtime is entered). Use try_spawn
        // pattern — if Handle::try_current() fails, skip the cleanup task.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("No tokio runtime — GUI cleanup task not started");
            self.cleanup_started.store(false, Ordering::SeqCst);
            return;
        };
        handle.spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let Some(service) = weak.upgrade() else {
                    break;
                };
                service.expire_sessions().await;
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<GuiSessionEvent> {
        self.event_tx.subscribe()
    }

    pub async fn subscribe_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<broadcast::Receiver<GuiSessionEvent>, GuiInteractionError> {
        self.assert_capability_token(session_id, capability_token)
            .await?;
        Ok(self.subscribe())
    }

    #[tracing::instrument(skip_all, fields(min_confidence = req.min_confidence))]
    pub async fn create_session(
        &self,
        req: GuiCreateSessionRequest,
    ) -> Result<GuiCreateSessionResponse, GuiInteractionError> {
        self.require_hmac_secret()?;

        let focus = self
            .focus_probe
            .current_focus()
            .await
            .map_err(map_core_error)?;
        let scene = self
            .scene_finder
            .analyze_scene(req.app_name.as_deref(), req.screen_id.as_deref())
            .await
            .map_err(map_core_error)?;

        let now = Utc::now();
        let max_candidates = req
            .max_candidates
            .unwrap_or(DEFAULT_MAX_CANDIDATES)
            .clamp(1, 1000);
        let min_confidence = req
            .min_confidence
            .unwrap_or(DEFAULT_MIN_CONFIDENCE)
            .clamp(0.0, 1.0);
        let ttl_secs = req
            .session_ttl_secs
            .map(|value| value as i64)
            .unwrap_or(DEFAULT_SESSION_TTL_SECS)
            .clamp(30, 3600);

        let candidates = build_candidates(&scene, min_confidence, max_candidates);
        if candidates.is_empty() {
            tracing::warn!(
                app = ?req.app_name,
                min_confidence,
                "GUI session creation failed — no eligible candidates"
            );
            return Err(GuiInteractionError::BadRequest {
                code: oneshim_core::error_codes::GuiCode::BadRequest,
                message: "No eligible GUI candidates found in scene".to_string(),
            });
        }

        let session_id = Uuid::new_v4().to_string();
        let session = GuiInteractionSession {
            schema_version: oneshim_core::models::gui::GUI_INTERACTION_SCHEMA_VERSION.to_string(),
            session_id: session_id.clone(),
            state: GuiSessionState::Proposed,
            scene,
            focus,
            candidates,
            selected_element_id: None,
            created_at: now,
            updated_at: now,
            expires_at: now + ChronoDuration::seconds(ttl_secs),
        };

        let capability_token = new_capability_token();

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(
                session_id.clone(),
                StoredSession {
                    session: session.clone(),
                    capability_token: capability_token.clone(),
                    overlay_handle_id: None,
                    confirmed_action: None,
                    used_ticket_nonces: HashSet::new(),
                },
            );
        }

        tracing::info!(
            session_id = %session.session_id,
            candidates = session.candidates.len(),
            ttl_secs = ttl_secs,
            app = %session.focus.app_name,
            "GUI session created"
        );

        self.publish_event(
            session_id,
            GuiSessionState::Proposed,
            "gui_session.proposed",
            None,
        );

        Ok(GuiCreateSessionResponse {
            session,
            capability_token,
        })
    }

    pub async fn get_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.assert_capability_token(session_id, capability_token)
            .await?;

        let mut sessions = self.sessions.write().await;
        let Some(stored) = sessions.get_mut(session_id) else {
            return Err(GuiInteractionError::NotFound {
                code: oneshim_core::error_codes::GuiCode::NotFound,
                name: session_id.to_string(),
            });
        };

        if is_expired(&stored.session.expires_at) {
            stored.session.state = GuiSessionState::Expired;
        }

        Ok(stored.session.clone())
    }

    #[tracing::instrument(skip_all, fields(session_id = %session_id))]
    pub async fn highlight_session(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiHighlightRequest,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.assert_capability_token(session_id, capability_token)
            .await?;

        let (scene_id, targets, previous_handle_id) = {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };

            if is_expired(&stored.session.expires_at) {
                stored.session.state = GuiSessionState::Expired;
                return Err(GuiInteractionError::TicketInvalid {
                    code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                    message: "Session already expired".to_string(),
                });
            }

            let candidate_ids: Option<HashSet<String>> = req.candidate_ids.as_ref().map(|ids| {
                ids.iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect()
            });

            let targets: Vec<HighlightTarget> = stored
                .session
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.eligible
                        && candidate_ids
                            .as_ref()
                            .map(|ids| ids.contains(&candidate.element.element_id))
                            .unwrap_or(true)
                })
                .map(|candidate| HighlightTarget {
                    candidate_id: candidate.element.element_id.clone(),
                    bbox_abs: ElementBounds {
                        x: candidate.element.bbox_abs.x,
                        y: candidate.element.bbox_abs.y,
                        width: candidate.element.bbox_abs.width,
                        height: candidate.element.bbox_abs.height,
                    },
                    color: "#22c55e".to_string(),
                    label: candidate.element.text_masked.clone(),
                })
                .collect();

            if targets.is_empty() {
                return Err(GuiInteractionError::BadRequest {
                    code: oneshim_core::error_codes::GuiCode::BadRequest,
                    message: "No highlight targets available".to_string(),
                });
            }

            (
                stored.session.scene.scene_id.clone(),
                targets,
                stored.overlay_handle_id.clone(),
            )
        };

        if let Some(handle_id) = previous_handle_id {
            if let Err(e) = self.overlay_driver.clear_highlights(&handle_id).await {
                debug!("clear_highlights failed: {e}");
            }
        }

        let handle = self
            .overlay_driver
            .show_highlights(HighlightRequest {
                session_id: session_id.to_string(),
                scene_id,
                targets,
            })
            .await
            .map_err(map_core_error)?;

        let updated = {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };
            stored.overlay_handle_id = Some(handle.handle_id);
            stored.session.state = GuiSessionState::Highlighted;
            stored.session.updated_at = Utc::now();
            stored.session.clone()
        };

        self.publish_event(
            session_id.to_string(),
            GuiSessionState::Highlighted,
            "gui_session.highlighted",
            None,
        );

        Ok(updated)
    }

    // confirm_candidate, prepare_execution, complete_execution → service_execution.rs

    pub async fn cancel_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.assert_capability_token(session_id, capability_token)
            .await?;

        let (session, overlay_handle_id) = {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };
            stored.session.state = GuiSessionState::Cancelled;
            stored.session.updated_at = Utc::now();
            (stored.session.clone(), stored.overlay_handle_id.take())
        };

        if let Some(handle_id) = overlay_handle_id {
            if let Err(e) = self.overlay_driver.clear_highlights(&handle_id).await {
                debug!("clear_highlights failed: {e}");
            }
        }

        tracing::info!(session_id, "GUI session cancelled");

        self.publish_event(
            session_id.to_string(),
            GuiSessionState::Cancelled,
            "gui_session.cancelled",
            None,
        );

        Ok(session)
    }

    pub(super) async fn assert_capability_token(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<(), GuiInteractionError> {
        let sessions = self.sessions.read().await;
        let Some(stored) = sessions.get(session_id) else {
            return Err(GuiInteractionError::NotFound {
                code: oneshim_core::error_codes::GuiCode::NotFound,
                name: session_id.to_string(),
            });
        };

        if stored.capability_token != capability_token.trim() {
            return Err(GuiInteractionError::Unauthorized {
                code: oneshim_core::error_codes::GuiCode::Unauthorized,
            });
        }

        Ok(())
    }

    pub(super) fn publish_event(
        &self,
        session_id: String,
        state: GuiSessionState,
        event_type: &str,
        message: Option<String>,
    ) {
        let _ = self.event_tx.send(GuiSessionEvent {
            schema_version: oneshim_core::models::gui::GUI_SESSION_EVENT_SCHEMA_VERSION.to_string(),
            event_type: event_type.to_string(),
            session_id,
            state,
            emitted_at: Utc::now(),
            message,
        });
    }

    pub(super) async fn expire_sessions(&self) {
        let (expired_ids, orphaned_overlay_ids) = {
            let mut sessions = self.sessions.write().await;
            let now = Utc::now();
            let expired: Vec<(String, Option<String>)> = sessions
                .iter()
                .filter(|(_, stored)| stored.session.expires_at <= now)
                .map(|(session_id, stored)| (session_id.clone(), stored.overlay_handle_id.clone()))
                .collect();

            let expired_ids: Vec<String> = expired.iter().map(|(id, _)| id.clone()).collect();
            let orphaned_overlay_ids: Vec<String> = expired
                .iter()
                .filter_map(|(_, overlay)| overlay.clone())
                .collect();

            for session_id in &expired_ids {
                sessions.remove(session_id);
            }

            (expired_ids, orphaned_overlay_ids)
        };

        for handle_id in orphaned_overlay_ids {
            if let Err(e) = self.overlay_driver.clear_highlights(&handle_id).await {
                debug!("clear_highlights failed: {e}");
            }
        }

        if !expired_ids.is_empty() {
            tracing::debug!(
                count = expired_ids.len(),
                "GUI sessions expired by TTL cleanup"
            );
        }

        for session_id in expired_ids {
            self.publish_event(
                session_id,
                GuiSessionState::Expired,
                "gui_session.expired",
                None,
            );
        }
    }

    pub(super) fn require_hmac_secret(&self) -> Result<&[u8], GuiInteractionError> {
        self.hmac_secret
            .as_deref()
            .ok_or_else(|| GuiInteractionError::Unavailable {
                code: oneshim_core::error_codes::GuiCode::Unavailable,
                message: format!("{GUI_HMAC_SECRET_ENV} is missing or empty"),
            })
    }
}
