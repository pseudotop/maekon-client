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

/** User-created annotation attached to a captured frame (Rust `AnnotationType`). */
export type AnnotationType = 'Highlight' | 'Memo' | 'Arrow'

/**
 * Canonical `ExternalDataPolicy` wire tokens (Rust enum, PascalCase serde).
 * #9524: the server RESPONSE path only ever emits these three; fixtures and
 * form state must use them. The server REQUEST path additionally accepts
 * exactly one lowercase legacy alias ('disabled' → PiiFilterStrict) via
 * `settings_validation.rs` — that alias is a server-side compatibility
 * contract and deliberately NOT part of this type. Note the type only guards
 * annotated non-test src code; test/e2e/private fixtures sit outside the
 * tsc program and still rely on review.
 */
export type ExternalDataPolicy = 'PiiFilterStrict' | 'PiiFilterStandard' | 'AllowFiltered'

/**
 * #9535: canonical wire tokens for the sibling enum-backed fields, same
 * scope/limits as `ExternalDataPolicy` above (guards annotated non-test src;
 * request paths accept case-insensitive aliases server-side).
 * Rust: `SandboxProfile` / `OcrProviderType` / `LlmProviderType` — all
 * PascalCase in both serde and Display.
 */
export type SandboxProfile = 'Permissive' | 'Standard' | 'Strict'
export type OcrProvider = 'Local' | 'Remote'
export type LlmProvider = 'Local' | 'Remote'

export interface FrameAnnotation {
  annotation_id: string
  frame_id: number
  annotation_type: AnnotationType
  x: number
  y: number
  width: number
  height: number
  color: string | null
  text: string | null
  created_at: string
}

export interface CreateFrameAnnotationRequest {
  annotation_type: AnnotationType
  x: number
  y: number
  width?: number
  height?: number
  color?: string | null
  text?: string | null
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

export interface AnalysisSettings {
  enabled: boolean
  interval_secs: number
  min_confidence: number
  max_suggestions: number
  embedding_enabled: boolean
  /**
   * Whether the local LLM generates a natural-language daily-digest narrative
   * before embedding (maps to nested Rust `analysis.embedding.llm_summary_enabled`).
   * The narrative pipeline only runs when `enabled`, `embedding_enabled`, and this
   * flag are all true; the rule-based digest always runs regardless.
   */
  llm_summary_enabled: boolean
  gui_intelligence_enabled: boolean
  text_intelligence_enabled: boolean
  auto_tuner_enabled: boolean
  /**
   * #9629: master switch for the tiered-memory pipeline (maps to nested Rust
   * `analysis.tiered_memory.enabled`) — powers /day, /week, recalibration
   * segments, coaching goal progress, and habit streaks.
   */
  tiered_memory_enabled: boolean
  /**
   * #9629: flat contract field (maps to nested Rust
   * `analysis.tiered_memory.regime_detection_interval_hours`). Replaces the
   * phantom nested `tiered_memory` object the server never sent.
   */
  regime_detection_interval_hours: number
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
  /**
   * Non-failing observations (#9714). Separate from `errors` because `success`
   * is `errors.is_empty()` server-side: a relation pointing at a frame this
   * device no longer has is data-hygiene noise (#9721), not a failed restore.
   * Absent on older servers, hence optional.
   */
  notes?: string[]
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
      ocr_text?: string | null
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

export interface SuggestionDto {
  id: number
  suggestion_id: string
  suggestion_type: string
  source: string
  content: string
  priority: string
  confidence_score: number
  relevance_score: number
  is_actionable: boolean
  reasoning?: string | null
  shown_at?: string | null
  dismissed_at?: string | null
  acted_at?: string | null
  created_at: string
  expires_at?: string | null
}

export type SuggestionFeedbackAction = 'shown' | 'dismissed' | 'acted'

export interface AutomationSettings {
  enabled: boolean
}

export interface SandboxSettings {
  enabled: boolean
  profile: SandboxProfile
  allowed_read_paths: string[]
  allowed_write_paths: string[]
  allow_network: boolean
  max_memory_bytes: number
  max_cpu_time_ms: number
}

export interface AiProviderProfileConfig {
  access_mode: string
  ocr_provider: OcrProvider
  llm_provider: LlmProvider
  external_data_policy: ExternalDataPolicy
  bypass_pii_filter_for_external_ocr: boolean
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

export type ProviderCliReadiness =
  | 'not_detected'
  | 'auth_required'
  | 'auth_unsupported'
  | 'auth_stale'
  | 'interactive_required'
  | 'auth_unverified'
  | 'auth_ready'
  | 'invocation_ready'
  | 'runtime_unsupported'

export type ProviderCliVersionStatus = 'not_checked'

export type ProviderCliDependencyStatus = 'ready' | 'missing' | 'stale_process_env' | 'not_required'

export interface ProviderCliDiscoveryReport {
  candidate_name: string
  executable_path: string
  version_status: ProviderCliVersionStatus
  dependency_status: ProviderCliDependencyStatus
  status_reason: string | null
  env_refresh_required: boolean
}

export interface FeatureCapability {
  feature_id: string
  maturity: FeatureMaturity
  availability: FeatureAvailability
  provider_cli_readiness?: ProviderCliReadiness | null
  provider_cli_discovery?: ProviderCliDiscoveryReport | null
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
  /**
   * COMPILE-capability flag (#7600): true only when this binary was built with the `audio`
   * cargo feature. Optional on the TS side (existing test fixtures predate this field) — treat
   * a missing value as `false` (fail-closed): the shipped release build never compiles audio in.
   */
  audio_compiled?: boolean
  /**
   * PLATFORM-capability flag (#7678): true only when a local OCR engine (platform-native or
   * leptess/Tesseract) is actually compiled + usable on this platform. `false` on Linux in
   * every shipped build today. Optional on the TS side (existing test fixtures predate this
   * field) — treat a missing value as `false` (fail-closed).
   */
  ocr_available?: boolean
  /**
   * PLATFORM-capability flag (#7678): true only when real battery/power data is available
   * (macOS only today — Windows/Linux always report an empty default). Optional on the TS side
   * (existing test fixtures predate this field) — treat a missing value as `false` (fail-closed).
   */
  power_status_available?: boolean
  /**
   * PLATFORM-capability flag (#7678): true when active-window detection is expected to work
   * reliably (always on macOS/Windows; false on Linux under a Wayland session with no
   * dependable native path). Optional on the TS side — treat a missing value as `false`
   * (fail-closed). Read by the Monitoring tab's platform-capability matrix (#8686 AC3).
   */
  active_window_available?: boolean
  /**
   * PLATFORM/BUILD-capability flag (#8686 AC3): true when the out-of-process automation
   * sandbox can actually enforce isolation on this host + build. Optional on the TS side —
   * treat a missing value as `false` (fail-closed).
   */
  automation_sandbox_available?: boolean
  /**
   * Linux graphical session type (#8686 AC3): 'wayland' | 'x11' | 'unknown' on Linux,
   * null/absent elsewhere. Lets the permission matrix name the Wayland degradation
   * explicitly instead of a generic "manual check".
   */
  linux_session_type?: string | null
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
  microphone: DesktopPermissionEntry
  input_monitoring: DesktopPermissionEntry
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
  sandbox_profile: SandboxProfile
  ocr_provider: OcrProvider
  llm_provider: LlmProvider
  ocr_source: string
  llm_source: string
  ocr_fallback_reason: string | null
  llm_fallback_reason: string | null
  external_data_policy: ExternalDataPolicy
  pending_audit_entries: number
  /** Intent-hint confirmation policy; combine with sandbox fields for containment copy. */
  confirmation_policy?: string
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

/** Privacy-bounded row returned by `GET /api/audit/export`. */
export interface AuditExportEntry {
  entry_id: string
  timestamp: string
  command_id: string
  action_type: string
  status: string
  execution_time_ms?: number | null
}

/** A single hash-chain integrity break (#4834/ADR-072/#7600). */
export interface AuditChainBreak {
  seq: number
  reason: string
}

/**
 * Result of verifying the durable `audit_log` SHA-256 hash chain (#4834,
 * ADR-072, #7600 — `GET /api/audit/verify` / desktop `verify_audit_log` IPC).
 * A SHA-256-only chain is tamper-evident (detects accidental/partial
 * corruption, simple row edits, deletions, and reordering) but not
 * tamper-proof.
 */
export interface AuditChainReport {
  ok: boolean
  first_seq: number | null
  last_seq: number | null
  verified_count: number
  legacy_unchained_count: number
  first_break: AuditChainBreak | null
}

/**
 * One egress-ledger row (T1.2, #7910 — `GET /api/privacy/egress-ledger`). The
 * audit record of a single egress (or prevented-capture) event: the ledger's
 * own nine columns, which record *that* egress happened (byte counts,
 * destination sink, disposition) — never *what* was sent. A `capture_blocked`
 * row (destination `local.capture`, byte/recipient 0) is prevented-capture
 * evidence — a frame deliberately NOT captured, not an upload.
 */
export interface EgressLedgerEntry {
  record_id: string
  event_type: string
  event_id: string | null
  byte_count: number
  recipient_count: number
  destination: string
  disposition: string
  consent_state: string
  occurred_at: string
}

/** Response body for `GET /api/privacy/egress-ledger`. */
export interface EgressLedgerResponse {
  entries: EgressLedgerEntry[]
}

/**
 * One memory-graph claim node (T1.3, #7911 — `GET /api/memory/claims`). A
 * durable ADR-023 belief the agent has accumulated about the user, plus a cheap
 * evidence/provenance summary derived from its outbound edges. Timestamps are
 * epoch SECONDS (the frontend humanizes them). `evidence_segment_ids` are ids of
 * the captured segments supporting the belief — ids only, never content.
 */
export interface Claim {
  claim_id: string
  kind: string
  text: string
  source: string
  confidence: number
  status: string
  created_at: number
  updated_at: number
  evidence_count: number
  evidence_segment_ids: string[]
  supersedes_claim_ids: string[]
}

/** Response body for `GET /api/memory/claims`. */
export interface ClaimListResponse {
  claims: Claim[]
  /** Total matches before the `limit` truncation (for "N of M"). */
  total: number
}

/** Response body for `POST /api/memory/claims/{id}/retract`. */
export interface RetractClaimResponse {
  claim: Claim
  /** True when the claim was already retracted (idempotent no-op). */
  already_retracted: boolean
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
  sandbox_profile: SandboxProfile
  sandbox_enabled: boolean
  allow_network: boolean
  external_data_policy: ExternalDataPolicy
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
  sandbox_profile?: SandboxProfile | null
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

/**
 * #7600: `GET /api/semantic-search/capabilities`. `semantic_available` is `false`
 * whenever the vector/embedding pipeline is not wired up (e.g. `maekon-embedding`
 * compiled out of the shipped build) — `mode=semantic` would return HTTP 501 in
 * that case, and `mode=hybrid` silently degrades to keyword-only results.
 */
export interface SemanticSearchCapabilities {
  semantic_available: boolean
}

// ── Weekly Digest types ──────────────────────────────────────────

// NOTE (#5676): these mirror the Rust wire format in
// crates/maekon-core/src/models/weekly_digest.rs (no serde renames). The
// previous hand-written shape (total_minutes/category/deep_work_delta) had
// drifted from the wire — every renamed field deserialized as undefined.
// The OpenAPI contract types these endpoints as GenericObject, so
// openapi-sync cannot catch drift here; keep this in lockstep manually.
export interface ContentRanking {
  content_label: string
  total_mins: number
  /** WorkType enum — SCREAMING_SNAKE_CASE string (e.g. "ACTIVE_CODING"). */
  dominant_work_type: string
}

export interface WeekComparison {
  deep_work_delta_hours: number
  communication_delta_hours: number
  context_switch_delta: number
  trend_summary: string
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
  provider_cli: ProviderCliDiagnosticSummary[]
  recent_audit_entries: AuditEntry[]
  recent_policy_events: AuditEntry[]
}

export interface ProviderCliDiagnosticSummary {
  surface_id: string
  tool_id: string | null
  candidate_name: string | null
  executable_hint: string | null
  readiness: ProviderCliReadiness
  availability: FeatureAvailability
  dependency_status: ProviderCliDependencyStatus | null
  status_reason: string | null
  env_refresh_required: boolean
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

// ── Consent (GDPR) types ─────────────────────────────────────────────────────
// Mirrors the wire contract of maekon-core/src/consent.rs verbatim:
//   - ConsentStatus: #[serde(rename_all = "PascalCase")] → PascalCase strings.
//   - ConsentPermissions: the struct has no rename_all → fields serialize as snake_case.
//   - ConsentSnapshot: the DTO from src-tauri/src/commands/consent.rs.

/** Matches Rust's `ConsentStatus` enum serialized as PascalCase (serde rename_all). */
export type ConsentStatus = 'NotGranted' | 'Valid' | 'Expired' | 'UpdateRequired'

/**
 * Matches Rust's `ConsentPermissions` struct (snake_case fields, no rename_all).
 * 17 tiered boolean permissions; all default to false (fail-closed) on the Rust side.
 *
 * Tier numbering is NOT contiguous: Tiers 11/12 are name-reserved by ADR-032 for
 * its Modes B/C and have no field yet, so Tier 13 follows Tier 10 directly.
 */
export interface ConsentPermissions {
  // Tier 1
  screen_capture: boolean
  ocr_processing: boolean
  telemetry: boolean
  process_monitoring: boolean
  input_activity: boolean
  // Tier 2
  window_title_collection: boolean
  app_usage_analytics: boolean
  // Tier 3
  clipboard_monitoring: boolean
  file_access_monitoring: boolean
  // Tier 4: Tiered Memory
  activity_pattern_learning: boolean
  // Tier 5: Cross-Device Sync
  cross_device_sync: boolean
  // Tier 6: Text Intelligence
  full_text_extraction: boolean
  // Tier 7: Memory-Graph Enrichment
  memory_graph_enrichment: boolean
  // Tier 8: Audio/Voice
  microphone: boolean
  // Tier 9: Raw Off-Device OCR
  unredacted_external_ocr: boolean
  // Tier 10: Memory-Graph Retrieval Ranking (ADR-032 Mode A)
  memory_graph_retrieval_ranking: boolean
  /**
   * Tier 13: Memory Vault Mirror (ADR-033). Permits continuously mirroring
   * digests + Active claims to a local Markdown vault OUTSIDE SQLite. Dedicated
   * and never implied by a sibling permission; the separate
   * `custom_path_acknowledged` config gate (ADR-033 §3.3) additionally covers
   * custom-folder overwrite/sync risk.
   */
  memory_vault_mirror: boolean
}

/** Matches Rust's `ConsentSnapshot` DTO returned by the consent IPC commands. */
export interface ConsentSnapshot {
  status: ConsentStatus
  permissions: ConsentPermissions
}
