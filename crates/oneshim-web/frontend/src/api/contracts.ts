// Generated from api/client.ts declarations. Keep this file type-only.
// Source of truth for frontend API transport contracts.
export interface DailySummary {
  date: string
  total_active_secs: number
  total_idle_secs: number
  top_apps: AppUsage[]
  cpu_avg: number
  memory_avg_percent: number
  frames_captured: number
  events_logged: number
}

export interface AppUsage {
  name: string
  duration_secs: number
  event_count: number
  frame_count: number
}

export interface SystemMetrics {
  timestamp: string
  cpu_usage: number
  memory_used: number
  memory_total: number
  memory_percent: number
  disk_used: number
  disk_total: number
  network_upload: number
  network_download: number
}

export interface HourlyMetrics {
  hour: string
  cpu_avg: number
  cpu_max: number
  memory_avg: number
  memory_max: number
  sample_count: number
}

export interface ProcessSnapshot {
  timestamp: string
  processes: ProcessEntry[]
}

export interface ProcessEntry {
  pid: number
  name: string
  cpu_usage: number
  memory_bytes: number
}

export interface Frame {
  id: number
  timestamp: string
  trigger_type: string
  app_name: string
  window_title: string
  importance: number
  resolution: string
  file_path: string | null
  ocr_text: string | null
  image_url: string | null
  tag_ids: number[]
}

export interface IdlePeriod {
  start_time: string
  end_time: string | null
  duration_secs: number | null
}

export interface Session {
  session_id: string
  started_at: string
  ended_at: string | null
  total_events: number
  total_frames: number
  total_idle_secs: number
  active_duration_secs: number | null
}

export interface StorageStats {
  db_size_bytes: number
  frames_size_bytes: number
  total_size_bytes: number
  frame_count: number
  event_count: number
  metric_count: number
  oldest_data_date: string | null
  newest_data_date: string | null
}

export interface NotificationSettings {
  enabled: boolean
  idle_notification: boolean
  idle_notification_mins: number
  long_session_notification: boolean
  long_session_mins: number
  high_usage_notification: boolean
  high_usage_threshold: number
}

export interface TelemetrySettings {
  enabled: boolean
  crash_reports: boolean
  usage_analytics: boolean
  performance_metrics: boolean
}

export interface MonitorControlSettings {
  process_monitoring: boolean
  input_activity: boolean
  privacy_mode: boolean
}

export interface PrivacySettings {
  excluded_apps: string[]
  excluded_app_patterns: string[]
  excluded_title_patterns: string[]
  auto_exclude_sensitive: boolean
  pii_filter_level: string
}

export interface ScheduleSettings {
  active_hours_enabled: boolean
  active_start_hour: number
  active_end_hour: number
  active_days: string[]
  pause_on_screen_lock: boolean
  pause_on_battery_saver: boolean
}

export type UpdateChannel = 'stable' | 'pre_release' | 'nightly'

export interface UpdateSettings {
  enabled: boolean
  check_interval_hours: number
  /** Update channel: stable, pre_release, or nightly */
  channel: UpdateChannel
  /** @deprecated Use channel instead */
  include_prerelease: boolean
  auto_install: boolean
}

export interface AppSettings {
  retention_days: number
  max_storage_mb: number
  web_port: number
  allow_external: boolean
  capture_enabled: boolean
  idle_threshold_secs: number
  metrics_interval_secs: number
  process_interval_secs: number
  notification: NotificationSettings
  update: UpdateSettings
  telemetry: TelemetrySettings
  monitor: MonitorControlSettings
  privacy: PrivacySettings
  schedule: ScheduleSettings
  automation: AutomationSettings
  sandbox: SandboxSettings
  ai_provider: AiProviderSettings
  ai_session: AiSessionSettings
  suggestion: SuggestionSettings
  indicator: IndicatorSettings
  analysis: AnalysisSettings
  network: NetworkSettings
  coaching: CoachingSettings
  integration: IntegrationSettings
  sync: SyncSettings
  audio: AudioSettings
  focus_auto: FocusAutoSettings
}

export interface AudioSettings {
  enabled: boolean
  whisper_model_path: string
  language: string
  max_recording_secs: number
  model_size: string
  stt_provider: string
  cloud_api_key: string
  cloud_stt_endpoint: string
  cloud_timeout_secs: number
  mic_input_mode: string
  vad_threshold: number
  vad_silence_ms: number
  vad_min_speech_ms: number
}

export interface FocusAutoSettings {
  enabled: boolean
  duration_minutes: number
  trigger_apps: string[]
  trigger_schedules: FocusScheduleSettings[]
  cooldown_secs: number
}

export interface FocusScheduleSettings {
  start: string
  end: string
  days: string[]
}

export interface AiSessionSettings {
  max_concurrent_sessions: number
  idle_timeout_secs: number
  session_timeout_secs: number
  max_retries: number
  max_history_turns: number
  health_check_interval_secs: number
}

export interface SuggestionSettings {
  enabled: boolean
}

export interface IndicatorSettings {
  show_border: boolean
  show_panel: boolean
  border_opacity: number
}

export interface TieredMemorySettings {
  regime_detection_interval_hours: number
}

export interface AnalysisSettings {
  enabled: boolean
  interval_secs: number
  min_confidence: number
  max_suggestions: number
  embedding_enabled: boolean
  gui_intelligence_enabled: boolean
  text_intelligence_enabled: boolean
  auto_tuner_enabled: boolean
  tiered_memory?: TieredMemorySettings
}

export interface NetworkSettings {
  server_base_url: string
  request_timeout_ms: number
  grpc_enabled: boolean
  grpc_endpoint: string
  tls_enabled: boolean
}

export interface TimeRange {
  start: string
  end: string
}

export interface ProfileConfig {
  enabled: boolean
  min_interval_secs: number
}

export interface CoachingSettings {
  enabled: boolean
  tone: 'Direct' | 'Gentle' | 'DataDriven'
  quiet_hours: TimeRange[]
  profiles: Record<string, ProfileConfig>
  regime_goals: Record<string, number>
  locale: string
  overlay_mode: string
}

export interface IntegrationSettings {
  enabled: boolean
  auth_profile_kind: string
  request_timeout_secs: number
  sync_interval_secs: number
}

export interface SyncSettings {
  enabled: boolean
  transport: string
  interval_secs: number
  device_name: string
  lan_advertise: boolean
  compression_enabled: boolean
}

export type UpdatePhase =
  | 'Idle'
  | 'Checking'
  | 'PendingApproval'
  | 'Downloading'
  | 'ReadyToInstall'
  | 'Installing'
  | 'Updated'
  | 'Deferred'
  | 'Error'
  /** Phase 4 D11: automatic rollback completed after repeated startup failures. */
  | 'RolledBack'

/** Phase 4 D11: rollback escalation reason (snake_case to match Rust serde). */
export type RollbackReason = 'repeated_startup_failure'

export interface RollbackInfo {
  from_version: string
  /** RFC3339 UTC timestamp of the rolled-from release (if known). */
  from_published_at?: string | null
  to_version: string
  /** RFC3339 UTC timestamp of the rolled-to release (if known). */
  to_published_at?: string | null
  reason: RollbackReason
  /** RFC3339 UTC timestamp at which the rollback completed. */
  rolled_back_at: string
}

export interface DownloadProgress {
  bytes_downloaded: number
  total_bytes: number
  percent: number
}

export interface PendingUpdateInfo {
  current_version: string
  latest_version: string
  release_url: string
  release_name: string | null
  published_at: string | null
  download_url: string
  release_notes?: string | null
  download_size_bytes?: number | null
}

export interface UpdateStatus {
  enabled: boolean
  auto_install: boolean
  phase: UpdatePhase
  message: string | null
  pending: PendingUpdateInfo | null
  download_progress: DownloadProgress | null
  /** Phase 4 D11: populated when phase === 'RolledBack'. */
  rollback?: RollbackInfo | null
  revision: number
  updated_at: string
}

export interface UpdateActionResponse {
  accepted: boolean
  status: UpdateStatus
}

export type UpdateAction = 'Approve' | 'Defer' | 'CheckNow'

export interface IntegrationAckCursorSummary {
  stream_id: string
  cursor: string
  acknowledged_at: string
}

export interface IntegrationSessionSummary {
  status: string
  transport_kind: string
  auth_scheme: string
  connected_at?: string | null
  last_heartbeat_at?: string | null
  requested_scopes: string[]
  granted_scopes: string[]
}

export interface IntegrationDeviceAuthorizationFlow {
  flow_id: string
  user_code: string
  verification_uri: string
  verification_uri_complete?: string | null
  expires_at: string
  interval_secs: number
  requested_scopes: string[]
  resource_indicator?: string | null
}

export interface IntegrationAuthStatus {
  profile_kind: string
  status: string
  interactive: boolean
  authenticated: boolean
  expires_at?: string | null
  resource_indicator?: string | null
  pending_flow?: IntegrationDeviceAuthorizationFlow | null
  message?: string | null
}

export interface IntegrationRuntimeLaneTelemetry {
  consecutive_failures: number
  last_success_at?: string | null
  last_failure_at?: string | null
  backoff_until?: string | null
  last_error?: string | null
}

export interface IntegrationRuntimeTelemetry {
  connect: IntegrationRuntimeLaneTelemetry
  heartbeat: IntegrationRuntimeLaneTelemetry
  egress: IntegrationRuntimeLaneTelemetry
  inbox: IntegrationRuntimeLaneTelemetry
}

export interface IntegrationOutboundRuntimeStatus {
  enabled: boolean
  bootstrap_configured: boolean
  auth_source_configured: boolean
  auth_material_available: boolean
  runtime_configured: boolean
  resource_indicator_configured: boolean
  auth_profile_kind: string
  preferred_transports: string[]
  supported_auth_schemes: string[]
  outbox_pending_count?: number | null
  inbox_pending_count?: number | null
  outbox_ack_cursor?: IntegrationAckCursorSummary | null
  inbox_ack_cursor?: IntegrationAckCursorSummary | null
  auth_status?: IntegrationAuthStatus | null
  current_session?: IntegrationSessionSummary | null
  runtime_telemetry?: IntegrationRuntimeTelemetry | null
}

export interface IntegrationStatus {
  schema_version: string
  external_access_enabled: boolean
  automation_controller_configured: boolean
  ai_runtime_status?: Record<string, unknown> | null
  outbound_runtime: IntegrationOutboundRuntimeStatus
}

export interface IntegrationAuditRecordSummary {
  record_id: string
  envelope_id: string
  packet_id: string
  disposition: string
  reason?: string | null
  privacy_classification: string
  capability_scope: string
  occurred_at: string
}

export interface IntegrationAuditLogResponse {
  schema_version: string
  records: IntegrationAuditRecordSummary[]
}

export interface IntegrationInboxPromptSummary {
  prompt_id: string
  category: string
  priority: string
  title: string
  body: string
  status: string
  received_at: string
  status_updated_at: string
  presented_at?: string | null
  expires_at?: string | null
  source_system: string
  source_actor?: string | null
  correlation_id?: string | null
  dismiss_reason?: string | null
}

export interface IntegrationInboxResponse {
  schema_version: string
  prompts: IntegrationInboxPromptSummary[]
  pending_count: number
}

export interface IntegrationInboxRefreshResponse {
  schema_version: string
  fetched_count: number
}

export interface IntegrationInboxActionResponse {
  schema_version: string
  prompt_id: string
  status: string
}

export interface IntegrationInboxDismissRequest {
  reason?: string | null
}

export interface IntegrationDeviceAuthorizationCommandResult {
  auth_status: IntegrationAuthStatus
  flow?: IntegrationDeviceAuthorizationFlow | null
}

export interface IntegrationDeviceAuthorizationFlowRequest {
  flow_id: string
}

export interface PaginationMeta {
  total: number
  offset: number
  limit: number
  has_more: boolean
}

export interface PaginatedResponse<T> {
  data: T[]
  pagination: PaginationMeta
}

export interface Event {
  event_id: string
  event_type: string
  timestamp: string
  app_name: string | null
  window_title: string | null
  data: Record<string, unknown>
}

export interface ProviderModelsRequest {
  provider_type: string
  api_key: string
  endpoint?: string | null
  surface?: string | null
  surface_id?: string | null
  use_saved_secret?: boolean
}

export type ProviderModelSupportStatus = 'supported' | 'unsupported' | 'unknown'

export interface ProviderDiscoveredModel {
  id: string
  display_name?: string | null
  llm_support?: ProviderModelSupportStatus | null
  supports_ocr?: boolean | null
  ocr_support?: ProviderModelSupportStatus | null
  image_input_support?: ProviderModelSupportStatus | null
  structured_output_support?: ProviderModelSupportStatus | null
  capability_source?: string | null
}

export interface ProviderModelsResponse {
  models: string[]
  model_details?: ProviderDiscoveredModel[]
  notice?: string | null
}

export interface ProviderSurfaceSupports {
  llm: boolean
  ocr: boolean
  model_catalog: boolean
  context_bridge: boolean
}

export interface SurfaceDefaultModels {
  llm_models: string[]
  ocr_models: string[]
}

export interface ProviderKnownModelCapabilities {
  llm: boolean
  ocr: boolean
  image_input: boolean
}

export interface ProviderKnownModelSpec {
  id: string
  display_name?: string | null
  aliases: string[]
  id_prefixes: string[]
  capabilities: ProviderKnownModelCapabilities
  notes: string[]
}

export interface SubprocessTransportSpec {
  tool_id: string
  executable_candidates: string[]
  auth_probe_command: string[]
  auth_probe_mode: string
  invocation_mode: string
  model_flag?: string | null
  json_output_supported: boolean
}

export interface ProviderSurfaceSpec {
  surface_id: string
  vendor_id: string
  provider_type: string
  display_name: string
  execution_kind: string
  placement_kind: string
  credential_kind: string
  stability: string
  preferred_for_product_auth: boolean
  related_surface_ids?: string[]
  catalog_strategy: string
  supports: ProviderSurfaceSupports
  llm_capabilities?: {
    structured_output: boolean
  } | null
  ocr_capabilities?: {
    strategy: string
    supports_geometry: boolean
    supports_confidence: boolean
    requires_image_input_model: boolean
    requires_structured_output_model: boolean
  } | null
  default_models: SurfaceDefaultModels
  capability_rules?: {
    llm: {
      default_support: string
      allow_patterns: string[]
      deny_patterns: string[]
    }
    ocr: {
      default_support: string
      allow_patterns: string[]
      deny_patterns: string[]
    }
    image_input: {
      default_support: string
      allow_patterns: string[]
      deny_patterns: string[]
    }
    structured_output: {
      default_support: string
      allow_patterns: string[]
      deny_patterns: string[]
    }
  } | null
  unknown_model_policy?: {
    llm: 'allow' | 'warn' | 'reject'
    ocr: 'allow' | 'warn' | 'reject'
  } | null
  known_models: ProviderKnownModelSpec[]
  parameter_profiles: {
    llm: {
      supported: string[]
      unsupported: string[]
      notes: string[]
    }
    ocr: {
      supported: string[]
      unsupported: string[]
      notes: string[]
    }
  }
  llm_transport?: {
    method: string
    url: string
    auth_scheme: string
    request_shape: string
  } | null
  ocr_transport?: {
    method: string
    url: string
    auth_scheme: string
    request_shape: string
  } | null
  model_catalog_transport?: {
    method: string
    url: string
    auth_scheme: string
    response_shape: string
    llm_supported: boolean
    ocr_supported: boolean
    ocr_notice?: string | null
  } | null
  availability_probe?: {
    method: string
    url: string
    auth_scheme: string
  } | null
  subprocess_transport?: SubprocessTransportSpec | null
  references: string[]
}

export interface ProviderVendorSpec {
  vendor_id: string
  provider_type: string
  aliases: string[]
  display_name: string
}

export interface ProviderSurfaceCatalog {
  version: number
  updated_at: string
  vendors: ProviderVendorSpec[]
  surfaces: ProviderSurfaceSpec[]
}

export interface DeleteRangeRequest {
  from: string
  to: string
  data_types?: string[]
}

export interface DeleteResult {
  success: boolean
  events_deleted: number
  frames_deleted: number
  metrics_deleted: number
  process_snapshots_deleted: number
  idle_periods_deleted: number
  message: string
}

export interface SearchTagInfo {
  id: number
  name: string
  color: string
}

export interface SearchResult {
  result_type: 'frame' | 'event'
  id: string
  timestamp: string
  app_name: string | null
  window_title: string | null
  matched_text: string | null
  image_url: string | null
  importance: number | null
  tags?: SearchTagInfo[]
}

export interface SearchResponse {
  query: string
  total: number
  offset: number
  limit: number
  results: SearchResult[]
}

export interface SearchParams {
  query: string
  searchType?: 'all' | 'frames' | 'events'
  tagIds?: number[]
  limit?: number
  offset?: number
}

export interface HeatmapCell {
  day: number // 0=Mon, 6=Sun
  hour: number // 0-23
  value: number
}

export interface HeatmapResponse {
  from_date: string
  to_date: string
  cells: HeatmapCell[]
  max_value: number
}

export type ExportFormat = 'json' | 'csv'

export type ExportDataType = 'metrics' | 'events' | 'frames'

export interface Tag {
  id: number
  name: string
  color: string
  created_at: string
}

export interface CreateTagRequest {
  name: string
  color?: string
}

export interface UpdateTagRequest {
  name: string
  color: string
}

export type ReportPeriod = 'week' | 'month' | 'custom'

export interface ReportDailyStat {
  date: string
  active_secs: number
  idle_secs: number
  captures: number
  events: number
  cpu_avg: number
  memory_avg: number
}

export interface ReportAppStat {
  name: string
  duration_secs: number
  events: number
  captures: number
  percentage: number
}

export interface ReportHourlyActivity {
  hour: number
  activity: number
}

export interface ReportProductivity {
  score: number
  active_ratio: number
  peak_hour: number
  top_app: string
  trend: number
}

export interface ReportResponse {
  title: string
  from_date: string
  to_date: string
  days: number
  total_active_secs: number
  total_idle_secs: number
  total_captures: number
  total_events: number
  avg_cpu: number
  avg_memory: number
  daily_stats: ReportDailyStat[]
  app_stats: ReportAppStat[]
  hourly_activity: ReportHourlyActivity[]
  productivity: ReportProductivity
}

export interface ReportParams {
  period: ReportPeriod
  from?: string
  to?: string
}

export interface BackupMetadata {
  version: string
  created_at: string
  app_version: string
  includes: {
    settings: boolean
    tags: boolean
    events: boolean
    frames: boolean
  }
}

export interface SettingsBackup {
  capture_enabled: boolean
  capture_interval_secs: number
  idle_threshold_secs: number
  metrics_interval_secs: number
  web_port: number
  notification_enabled: boolean
  idle_notification_mins: number
  long_session_notification_mins: number
  high_usage_threshold_percent: number
}

export interface TagBackup {
  id: number
  name: string
  color: string
  created_at: string
}

export interface FrameTagBackup {
  frame_id: number
  tag_id: number
  created_at: string
}

export interface EventBackup {
  event_id: string
  event_type: string
  timestamp: string
  app_name: string | null
  window_title: string | null
}

export interface FrameBackup {
  id: number
  timestamp: string
  trigger_type: string
  app_name: string
  window_title: string
  importance: number
  width: number
  height: number
  ocr_text: string | null
}

export interface BackupArchive {
  metadata: BackupMetadata
  settings?: SettingsBackup
  tags?: TagBackup[]
  frame_tags?: FrameTagBackup[]
  events?: EventBackup[]
  frames?: FrameBackup[]
}

export interface BackupParams {
  include_settings?: boolean
  include_tags?: boolean
  include_events?: boolean
  include_frames?: boolean
}

export interface RestoreResult {
  success: boolean
  restored: {
    settings: boolean
    tags: number
    frame_tags: number
    events: number
    frames: number
  }
  errors: string[]
}

export interface TimelineSessionInfo {
  start: string
  end: string
  duration_secs: number
  total_events: number
  total_frames: number
  total_idle_secs: number
}

export type TimelineItem =
  | { type: 'Event'; id: string; timestamp: string; event_type: string; app_name?: string; window_title?: string }
  | {
      type: 'Frame'
      id: number
      timestamp: string
      app_name: string
      window_title: string
      importance: number
      image_url: string
    }
  | { type: 'IdlePeriod'; start: string; end: string; duration_secs: number }

export interface AppSegment {
  app_name: string
  start: string
  end: string
  color: string
}

export interface TimelineResponse {
  session: TimelineSessionInfo
  items: TimelineItem[]
  segments: AppSegment[]
}

export interface TimelineParams {
  from?: string
  to?: string
  max_events?: number
  max_frames?: number
}

export interface FocusMetrics {
  date: string
  total_active_secs: number
  deep_work_secs: number
  communication_secs: number
  context_switches: number
  interruption_count: number
  avg_focus_duration_secs: number
  max_focus_duration_secs: number
  focus_score: number
}

export interface FocusMetricsResponse {
  today: FocusMetrics
  history: FocusMetrics[]
}

export interface WorkSession {
  id: number
  started_at: string
  ended_at: string | null
  primary_app: string
  category: string
  state: string
  interruption_count: number
  deep_work_secs: number
  duration_secs: number
}

export interface Interruption {
  id: number
  interrupted_at: string
  from_app: string
  from_category: string
  to_app: string
  to_category: string
  resumed_at: string | null
  resumed_to_app: string | null
  duration_secs: number | null
}

export interface LocalSuggestion {
  id: number
  suggestion_type: string
  payload: Record<string, unknown>
  created_at: string
  shown_at: string | null
  dismissed_at: string | null
  acted_at: string | null
}

export type SuggestionFeedbackAction = 'shown' | 'dismissed' | 'acted'

export interface AutomationSettings {
  enabled: boolean
}

export interface SandboxSettings {
  enabled: boolean
  profile: string
  allowed_read_paths: string[]
  allowed_write_paths: string[]
  allow_network: boolean
  max_memory_bytes: number
  max_cpu_time_ms: number
}

export interface AiProviderProfileConfig {
  access_mode: string
  ocr_provider: string
  llm_provider: string
  external_data_policy: string
  allow_unredacted_external_ocr: boolean
  ocr_validation: OcrValidationSettings
  scene_action_override: SceneActionOverrideSettings
  scene_intelligence: SceneIntelligenceSettings
  fallback_to_local: boolean
  ocr_api: ExternalApiSettings | null
  llm_api: ExternalApiSettings | null
}

export interface SavedAiProviderProfile {
  profile_id: string
  name: string
  ai_provider: AiProviderProfileConfig
  updated_at?: string | null
}

export interface AiProviderSettings extends AiProviderProfileConfig {
  active_profile_id?: string | null
  saved_profiles?: SavedAiProviderProfile[]
}

export interface SceneActionOverrideSettings {
  enabled: boolean
  reason: string
  approved_by: string
  expires_at: string | null
}

export interface OcrValidationSettings {
  enabled: boolean
  min_confidence: number
  max_invalid_ratio: number
}

export interface SceneIntelligenceSettings {
  enabled: boolean
  overlay_enabled: boolean
  allow_action_execution: boolean
  min_confidence: number
  max_elements: number
  calibration_enabled: boolean
  calibration_min_elements: number
  calibration_min_avg_confidence: number
}

export interface ExternalApiSettings {
  endpoint: string
  api_key_masked: string
  model: string | null
  provider_type: string
  surface_id?: string | null
  timeout_secs: number
  auth_mode: string
  backend_kind: string
  has_secret: boolean
  can_edit_secret: boolean
  secret_display_hint: string | null
  projection_enabled: boolean
}

// ── OAuth types ──────────────────────────────────────────────

export interface OAuthFlowHandle {
  flow_id: string
  auth_url: string
}

export type OAuthFlowStatus =
  | { status: 'pending' }
  | { status: 'completed' }
  | { status: 'failed'; error: string }
  | { status: 'cancelled' }

export interface OAuthConnectionStatus {
  provider_id: string
  connected: boolean
  expires_at: string | null
  scopes: string[]
  api_base_url: string | null
  has_refresh_token?: boolean
}

export interface SecretBackendCapabilities {
  os_secret_store_available: boolean
  oauth_available: boolean
  oauth_provider_ids: string[]
  default_backend_kind: string
  byok_backend_kind: string
  fallback_backend_kind: string
}

export type FeatureMaturity = 'stable' | 'beta' | 'experimental' | 'deprecated'

export type FeatureAvailability = 'available' | 'unavailable' | 'partially_available'

export interface FeatureCapability {
  feature_id: string
  maturity: FeatureMaturity
  availability: FeatureAvailability
  preferred: boolean
  requires: string[]
  status_reason: string | null
  status_copy_key: string | null
  setup_copy_key: string | null
  setup_docs_url: string | null
  configuration_env_vars: string[]
}

export interface FeatureCapabilitySnapshot {
  features: FeatureCapability[]
}

export type DesktopPermissionState = 'granted' | 'needs_attention' | 'not_required' | 'unavailable'

export interface DesktopPermissionEntry {
  state: DesktopPermissionState
  status_reason: string | null
}

export interface DesktopPermissionSnapshot {
  platform: string
  accessibility: DesktopPermissionEntry
  screen_capture: DesktopPermissionEntry
  notifications: DesktopPermissionEntry
}

export interface ProviderEndpointProbeResult {
  surface_id: string
  endpoint_kind: string
  endpoint: string
  availability: FeatureAvailability
  status_reason: string | null
  status_copy_key: string | null
}

export interface AutomationStatus {
  enabled: boolean
  sandbox_enabled: boolean
  sandbox_profile: string
  ocr_provider: string
  llm_provider: string
  ocr_source: string
  llm_source: string
  ocr_fallback_reason: string | null
  llm_fallback_reason: string | null
  external_data_policy: string
  pending_audit_entries: number
}

export interface AuditEntry {
  schema_version: string
  entry_id: string
  timestamp: string
  session_id: string
  command_id: string
  action_type: string
  status: string
  details: string | null
  elapsed_ms: number | null
}

export interface AutomationStats {
  total_executions: number
  successful: number
  failed: number
  denied: number
  timeout: number
  avg_elapsed_ms: number
  success_rate: number
  blocked_rate: number
  p95_elapsed_ms: number
  timing_samples: number
}

export interface PoliciesInfo {
  automation_enabled: boolean
  sandbox_profile: string
  sandbox_enabled: boolean
  allow_network: boolean
  external_data_policy: string
  scene_action_override_enabled: boolean
  scene_action_override_active: boolean
  scene_action_override_reason: string | null
  scene_action_override_approved_by: string | null
  scene_action_override_expires_at: string | null
  scene_action_override_issue: string | null
}

export interface AutomationContracts {
  audit_schema_version: string
  scene_schema_version: string
  scene_action_schema_version: string
}

export interface ExecutionPolicyConfig {
  policy_id: string
  process_name: string
  process_hash?: string | null
  allowed_args: string[]
  requires_sudo: boolean
  max_execution_time_ms: number
  audit_level: string
  sandbox_profile?: string | null
  allowed_paths: string[]
  allow_network?: boolean | null
  require_signed_token: boolean
  confirmation: string
}

export type IntentDefinition = Record<string, unknown>

export interface WorkflowPreset {
  id: string
  name: string
  description: string
  category: string
  steps: WorkflowStep[]
  builtin: boolean
  platform: string | null
  ai_profile_id?: string | null
}

export interface WorkflowStep {
  name: string
  intent: IntentDefinition
  delay_ms: number
  stop_on_failure: boolean
}

export interface PresetRunResult {
  preset_id: string
  success: boolean
  message: string
  steps_executed?: number
  total_steps?: number
  total_elapsed_ms?: number
}

export interface ExecuteIntentHintRequest {
  command_id?: string
  session_id: string
  intent_hint: string
}

export interface ExecuteIntentHintResponse {
  command_id: string
  session_id: string
  planned_intent: IntentDefinition
  result: {
    success: boolean
    element: unknown | null
    verification: unknown | null
    retry_count: number
    elapsed_ms: number
    error: string | null
  }
}

export type SceneActionType = 'click' | 'type_text'

export interface ExecuteSceneActionRequest {
  command_id?: string
  session_id: string
  frame_id?: number
  scene_id?: string
  element_id: string
  action_type: SceneActionType
  bbox_abs: UiSceneBounds
  role?: string | null
  label?: string | null
  text?: string | null
  allow_sensitive_input?: boolean
}

export interface ExecuteSceneActionResponse {
  schema_version: string
  command_id: string
  session_id: string
  frame_id?: number
  scene_id?: string
  element_id: string
  applied_privacy_policy: string
  scene_action_override_active: boolean
  scene_action_override_expires_at?: string | null
  executed_intents: IntentDefinition[]
  result: {
    success: boolean
    element: unknown | null
    verification: unknown | null
    retry_count: number
    elapsed_ms: number
    error: string | null
  }
}

export interface UiSceneBounds {
  x: number
  y: number
  width: number
  height: number
}

export interface UiSceneElement {
  element_id: string
  bbox_abs: UiSceneBounds
  bbox_norm: UiSceneBounds
  label: string
  role: string | null
  intent: string | null
  state: string | null
  confidence: number
  text_masked: string | null
  parent_id: string | null
}

export interface UiScene {
  schema_version: string
  scene_id: string
  app_name: string | null
  screen_id: string | null
  captured_at: string
  screen_width: number
  screen_height: number
  elements: UiSceneElement[]
}

// ── Recalibration types ──────────────────────────────────────

export type UserOverrideAction =
  | { type: 'MARK_AS_NOISE' }
  | { type: 'REASSIGN_REGIME'; target_regime_id: string }
  | { type: 'MARK_AS_PERSONAL_TIME'; from: string; to: string }

export interface RegimeOverride {
  override_id: string
  segment_id: string
  original_regime_id: string | null
  user_action: UserOverrideAction
  created_at: string
}

export interface CreateOverrideRequest {
  segment_id: string
  original_regime_id?: string
  action: UserOverrideAction
}

export interface ListOverridesQuery {
  from?: string
  to?: string
}

export interface SceneCalibrationReport {
  schema_version: string
  scene_id: string
  total_elements: number
  considered_elements: number
  avg_confidence: number
  min_confidence: number
  min_required_elements: number
  min_required_avg_confidence: number
  passed: boolean
  reasons: string[]
}

// Pomodoro timer
export type PomodoroStatus = 'running' | 'on_break' | 'completed' | 'cancelled'

export interface PomodoroSession {
  id: string
  started_at: string
  duration_minutes: number
  break_minutes: number
  status: PomodoroStatus
  remaining_secs: number
  completed_at: string | null
}

export interface StartPomodoroRequest {
  duration_minutes?: number
  break_minutes?: number
}

// GUI activity intelligence — click-position heatmap point (50x50 grid bin)
export interface GuiHeatmapPoint {
  x: number
  y: number
  count: number
}

// GUI interaction hourly heatmap cell
export interface GuiHeatmapCell {
  hour: string
  count: number
}

// ── Dashboard Day types ──────────────────────────────────────

export interface DailyDigestHighlight {
  highlight_type: string
  text: string
  segment_id?: string
}

export interface DailyDigestInsight {
  narrative: string
  highlights: DailyDigestHighlight[]
}

export interface DailyDigestContentSummary {
  content: string
  work_type: string
  mins: number
}

export interface DailyDigestSegment {
  segment_id: string
  start_time: string
  end_time: string
  duration_mins: number
  regime_label: string
  regime_color: string
  regime_id?: string
  dominant_app: string
  content_summary: DailyDigestContentSummary[]
  annotation?: { highlight_type: string; text: string }
}

export interface DailyDigestComparison {
  deep_work_delta: number
  communication_delta: number
  context_switch_delta: number
}

export interface DailyDigestStatistics {
  deep_work_hours: number
  communication_hours: number
  meeting_hours: number
  context_switches: number
  longest_focus_mins: number
  longest_focus_content: string
  regime_distribution: Record<string, number>
  comparison?: DailyDigestComparison
}

export interface DailyDigestResponse {
  date: string
  insight: DailyDigestInsight | null
  timeline: DailyDigestSegment[]
  statistics: DailyDigestStatistics
}

// ── GUI V2 Session types ─────────────────────────────────────────

export interface GuiCreateSessionRequest {
  app_name?: string
  screen_id?: string
  min_confidence?: number
  max_candidates?: number
  session_ttl_secs?: number
}

export interface GuiHighlightRequest {
  candidate_ids?: string[]
}

export interface GuiActionRequest {
  action_type: 'click' | 'type_text'
  text?: string
}

export interface GuiConfirmRequest {
  candidate_id: string
  action: GuiActionRequest
  ticket_ttl_secs?: number
}

export interface GuiExecutionTicket {
  ticket_id: string
  session_id: string
  candidate_id: string
  action: GuiActionRequest
  issued_at: string
  expires_at: string
}

export interface GuiExecutionRequest {
  ticket: GuiExecutionTicket
}

export interface GuiInteractionSession {
  session_id: string
  state: string
  scene: UiScene
  focus: GuiFocusInfo
  candidates: GuiCandidate[]
  created_at: string
  updated_at: string
  expires_at: string
}

export interface GuiFocusInfo {
  app_name: string
  window_title: string
  pid: number
  captured_at: string
  focus_hash: string
}

export interface GuiCandidate {
  candidate_id: string
  element: UiSceneElement
  highlighted: boolean
}

export interface GuiCreateSessionResponse {
  schema_version: string
  session: GuiInteractionSession
  capability_token: string
}

export interface GuiSessionResponse {
  schema_version: string
  session: GuiInteractionSession
}

export interface GuiConfirmResponse {
  schema_version: string
  ticket: GuiExecutionTicket
}

export interface GuiExecutionOutcome {
  session: GuiInteractionSession
  succeeded: boolean
  detail: string | null
  steps_completed: number
  total_steps: number
}

export interface IntentResult {
  success: boolean
  element: unknown | null
  verification: unknown | null
  retry_count: number
  elapsed_ms: number
  error: string | null
}

export interface GuiExecuteResponse {
  schema_version: string
  command_id: string
  ticket: GuiExecutionTicket
  result: IntentResult
  outcome: GuiExecutionOutcome
}

// ── Semantic Search types ────────────────────────────────────────

export interface SemanticSearchResult {
  segment_id: string
  content_type: string
  content_label: string | null
  original_text: string
  score: number
  similarity: number
  time_decay: number
  timestamp: string
  segment_start: string | null
  segment_end: string | null
  duration_secs: number | null
  llm_summary: string | null
  dominant_category: string | null
  regime_label: string | null
}

// ── Weekly Digest types ──────────────────────────────────────────

export interface ContentRanking {
  content_label: string
  total_minutes: number
  category: string
}

export interface WeekComparison {
  deep_work_delta: number
  communication_delta: number
  context_switch_delta: number
}

export interface WeeklyDigest {
  week_start: string
  week_end: string
  total_tracked_hours: number
  regime_breakdown: Record<string, number>
  category_breakdown: Record<string, number>
  top_content: ContentRanking[]
  deep_work_hours: number
  communication_hours: number
  context_switches_total: number
  longest_deep_work_segment_mins: number
  comparison: WeekComparison | null
  llm_narrative: string | null
}

// ── Onboarding types ─────────────────────────────────────────────

export interface QuickstartStep {
  order: number
  title: string
  action: string
  expected_outcome: string
}

export interface OnboardingQuickstartResponse {
  schema_version: string
  generated_at: string
  target_mode: string
  dashboard_url: string
  checklist: QuickstartStep[]
  recommended_presets: WorkflowPreset[]
  verification_commands: string[]
}

// ── Support Diagnostics types ────────────────────────────────────

export interface DiagnosticsHealth {
  storage_ok: boolean
  storage_error: string | null
  frames_dir_configured: boolean
  frames_dir_path: string | null
  frames_dir_exists: boolean | null
  config_manager_configured: boolean
  automation_controller_configured: boolean
  update_control_configured: boolean
}

export interface DiagnosticsBundleResponse {
  schema_version: string
  generated_at: string
  health: DiagnosticsHealth
  settings_snapshot: AppSettings
  storage_stats: StorageStats | null
  recent_audit_entries: AuditEntry[]
  recent_policy_events: AuditEntry[]
}

// ── Coaching Stats types ────────────────────────────────────────

export interface CoachingStatsToday {
  nudges_count: number
  current_regime: string | null
  regime_minutes_today: number
}

// --- Bug Report ---

export interface BugReportBundle {
  bug_id: string
  diagnostics: DiagnosticsBundleResponse
  system: SystemInfo
  connection: ConnectionStatus
  runtime_logs: RuntimeLogSnapshot | null
  pii_filter_level: string
}

export interface SystemInfo {
  app_version: string
  os_name: string
  os_version: string
  arch: string
  runtime: string
  cpu_count: number
  memory_total_mb: number
  memory_available_mb: number
  uptime_seconds: number
}

export interface ConnectionStatus {
  server_reachable: boolean
  last_sync_at: string | null
  grpc_enabled: boolean
  websocket_connected: boolean
}

export interface RuntimeLogSnapshot {
  generated_at: string
  log_dir: string
  log_file: string | null
  line_count: number
  recent_text: string
}

// ── Playbook Library types ────────────────────────────────────

export interface CoachingTemplateDto {
  profile: string
  trigger_type: string
  tone: string
  locale: string
  text: string
}

export interface CoachingTemplateListDto {
  templates: CoachingTemplateDto[]
}

export interface PresetSummaryDto {
  id: string
  name: string
  description: string
  category: string
  step_count: number
  builtin: boolean
}

export interface PresetSummaryListDto {
  presets: PresetSummaryDto[]
}

// ── Tracking Schedule ───────────────────────────────────────────────────────

/** Matches Rust's `Weekday` enum serialized as short names (ADR-001 serde). */
export type Weekday = 'Mon' | 'Tue' | 'Wed' | 'Thu' | 'Fri' | 'Sat' | 'Sun'

export interface TrackingWindow {
  start: string
  end: string
  /** Days of week this window is active. Empty = never active (matches backend). */
  days_of_week: Weekday[]
  label?: string
}

export interface TrackingScheduleConfig {
  enabled: boolean
  windows: TrackingWindow[]
  timezone?: string
}

export interface TrackingScheduleStatus {
  active_now: boolean
  ends_at: string | null
  next_starts_at: string | null
  label: string
}
