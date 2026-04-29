//! Coaching template data — static message templates for coaching profiles.
//!
//! This module provides the `CoachingTemplate` struct and the built-in `TEMPLATES`
//! slice containing 108 templates (54 English + 54 Korean).
//!
//! The data lives in `oneshim-core` because it is pure static domain data
//! referenced by both `oneshim-analysis` (template selection logic) and
//! `oneshim-web` (playbook listing endpoint). Keeping it here avoids a
//! forbidden cross-adapter dependency (web -> analysis).

use crate::config::CoachingTone;
use crate::models::coaching::CoachingProfile;

/// A coaching message template with variable placeholders.
///
/// Placeholders use `{variable_name}` syntax, resolved at runtime by the
/// template selection engine in `oneshim-analysis`.
#[derive(Debug, Clone)]
pub struct CoachingTemplate {
    pub profile: CoachingProfile,
    pub trigger_type: &'static str,
    pub tone: CoachingTone,
    pub locale: &'static str,
    pub text: &'static str,
}

// ── 108 built-in templates (54 English + 54 Korean) ─────────────────────

pub const TEMPLATES: &[CoachingTemplate] = &[
    // ── FocusGuard x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "You've switched from {regime} - {context_switches} switches in 30 min.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Heads up: you've moved away from {regime}. Need to switch back?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{context_switches} context switches today. Your average is {comparison}.",
    },
    // ── FocusGuard x RegimeDrift ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Drift detected in {regime}. Refocus on your current task.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Looks like your attention drifted from {regime}. Want to refocus?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Attention drift in {regime}: {context_switches} app switches detected.",
    },
    // ── TimeAware x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "{duration} in {regime}. Consider wrapping up.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You've been in {regime} for {duration} — longer than usual. A break might help.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{duration} in {regime}. Your average session is {comparison}.",
    },
    // ── TimeAware x GoalThreshold ──
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "{goal_progress}% of your {regime} goal reached ({goal_minutes} min target).",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Nice progress! {goal_progress}% toward your {regime} goal.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime} goal: {goal_progress}% complete. {remaining_minutes} min remaining.",
    },
    // ── DeepWorkCoach x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Deep work for {duration}. Take a 5-minute break.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Nice focus session! {duration} in deep work. A short break might help.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{duration} deep work session. Average is {comparison}. Break recommended.",
    },
    // ── DeepWorkCoach x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Leaving deep work after {duration}. Save your progress.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Transitioning out of deep work. Great session! Duration: {duration}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Deep work ended after {duration}. Today's total: {goal_minutes} min.",
    },
    // ── ContextRestore x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Welcome back. Your last context was {previous_context} in {app_name}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Back from break! You were working on {previous_context}. Ready to continue?",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Returning from idle. Previous: {previous_context} ({app_name}), {duration} ago.",
    },
    // ── GoalTracker x GoalThreshold (25%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "25% of {regime} goal done. {remaining_minutes} min to go.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You're a quarter of the way to your {regime} goal! Keep it up.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: 25% complete ({goal_progress} min / {goal_minutes} min target).",
    },
    // ── GoalTracker x GoalThreshold (50%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Halfway to your {regime} goal. {remaining_minutes} min left.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Great progress! You're halfway to your {regime} goal.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: 50% complete ({goal_progress} min / {goal_minutes} min).",
    },
    // ── GoalTracker x GoalThreshold (75%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Almost there — 75% of your {regime} goal. Push through!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You're 75% toward your {regime} target. Wonderful pace!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: 75% complete ({goal_progress} min / {goal_minutes} min). {remaining_minutes} min remaining.",
    },
    // ── GoalTracker x GoalThreshold (100%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "{regime} goal reached! {goal_minutes} min target complete.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Congratulations! You've hit your {regime} goal for today.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: 100% complete — {goal_minutes} min target achieved.",
    },
    // ── GoalTracker x GoalThreshold (over 100%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Over target! {goal_progress}% of {regime} goal ({goal_minutes} min).",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You've exceeded your {regime} target — {goal_progress}%. Well done!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: {goal_progress}% of {goal_minutes} min target. {goal_progress} min recorded.",
    },
    // ── Additional variant templates (14+ to reach 50+) ──
    // FocusGuard x RegimeOverstay
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Still in {regime} after {duration}. Consider a change of pace.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You've been focused on {regime} for {duration}. Everything okay?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: {duration} elapsed. Typical session: {comparison}.",
    },
    // ContextRestore x RegimeDrift
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Context drifting. Recall: you were in {previous_context}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Seems like you've drifted. Your earlier context was {previous_context}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Drift from {previous_context}. {context_switches} switches in this session.",
    },
    // DeepWorkCoach x RegimeDrift
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Drift in deep work detected. Close {app_name} and refocus.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Your deep work flow was interrupted. Want to get back on track?",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Deep work drift: {context_switches} switches. Average uninterrupted: {comparison}.",
    },
    // TimeAware x RegimeTransition
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Switching from {regime} after {duration}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Regime change from {regime}. You spent {duration} there.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime} session ended: {duration}. Average: {comparison}.",
    },
    // GoalTracker x RegimeOverstay
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "{regime} overstay: {duration}. Goal is {goal_minutes} min today.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "Long {regime} session ({duration}). You've logged {goal_progress} min of your {goal_minutes} min goal.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "{regime}: {duration} session. Daily total: {goal_progress}/{goal_minutes} min ({goal_progress}%).",
    },
    // FocusGuard x GoalThreshold
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "en",
        text: "Focus goal: {goal_progress}% of {goal_minutes} min. Stay on task.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "en",
        text: "You're {goal_progress}% toward your focus goal. Keep going!",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "en",
        text: "Focus time: {goal_progress}% of {goal_minutes} min target. {remaining_minutes} min left.",
    },
    // ════════════════════════════════════════════════════════════════════
    // ── Korean templates (locale: "ko") ─────────────────────────────────
    // ════════════════════════════════════════════════════════════════════
    // ── FocusGuard x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime}에서 전환됨 — 30분간 {context_switches}회 전환.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "알림: {regime}에서 벗어났습니다. 돌아가실 건가요?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "오늘 컨텍스트 전환 {context_switches}회. 평균은 {comparison}입니다.",
    },
    // ── FocusGuard x RegimeDrift ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime}에서 주의 분산 감지. 현재 작업에 다시 집중하세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime}에서 주의가 분산된 것 같습니다. 다시 집중해 볼까요?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime} 주의 분산: 앱 전환 {context_switches}회 감지.",
    },
    // ── TimeAware x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime}에서 {duration} 경과. 마무리를 고려해 보세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime}에서 {duration} 동안 작업 중입니다 — 평소보다 깁니다. 휴식이 도움될 수 있어요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}에서 {duration} 경과. 평균 세션은 {comparison}입니다.",
    },
    // ── TimeAware x GoalThreshold ──
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime} 목표의 {goal_progress}% 달성 ({goal_minutes}분 목표).",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "좋은 진행이에요! {regime} 목표의 {goal_progress}% 달성 중.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime} 목표: {goal_progress}% 완료. {remaining_minutes}분 남음.",
    },
    // ── DeepWorkCoach x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "딥워크 {duration} 경과. 5분간 휴식을 취하세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "훌륭한 집중 세션! 딥워크 {duration}. 짧은 휴식이 도움될 수 있어요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "딥워크 세션 {duration}. 평균은 {comparison}. 휴식 권장.",
    },
    // ── DeepWorkCoach x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{duration} 후 딥워크 종료. 진행 상황을 저장하세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "딥워크에서 전환 중. 훌륭한 세션이었습니다! 소요: {duration}.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "딥워크 {duration} 후 종료. 오늘 합계: {goal_minutes}분.",
    },
    // ── ContextRestore x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "돌아오셨습니다. 마지막 컨텍스트는 {app_name}의 {previous_context}였습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "휴식에서 복귀! {previous_context} 작업 중이었습니다. 계속하시겠어요?",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "유휴에서 복귀. 이전: {previous_context} ({app_name}), {duration} 전.",
    },
    // ── GoalTracker x GoalThreshold (25%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime} 목표의 25% 완료. {remaining_minutes}분 남았습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime} 목표의 1/4 지점에 도달했습니다! 계속 화이팅!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: 25% 완료 ({goal_progress}분 / {goal_minutes}분 목표).",
    },
    // ── GoalTracker x GoalThreshold (50%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime} 목표의 절반 도달. {remaining_minutes}분 남음.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "좋은 진행이에요! {regime} 목표의 절반에 도달했습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: 50% 완료 ({goal_progress}분 / {goal_minutes}분).",
    },
    // ── GoalTracker x GoalThreshold (75%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "거의 다 왔습니다 — {regime} 목표의 75%. 끝까지 힘내세요!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime} 목표의 75%에 도달했습니다. 훌륭한 페이스!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: 75% 완료 ({goal_progress}분 / {goal_minutes}분). {remaining_minutes}분 남음.",
    },
    // ── GoalTracker x GoalThreshold (100%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime} 목표 달성! {goal_minutes}분 목표 완료.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "축하합니다! 오늘의 {regime} 목표를 달성했습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: 100% 완료 — {goal_minutes}분 목표 달성.",
    },
    // ── GoalTracker x GoalThreshold (over 100%) ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "목표 초과! {regime} 목표의 {goal_progress}% ({goal_minutes}분).",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime} 목표를 초과 달성했습니다 — {goal_progress}%. 잘 하셨어요!",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: {goal_minutes}분 목표의 {goal_progress}%. {goal_progress}분 기록.",
    },
    // ── FocusGuard x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{duration} 후에도 {regime} 계속 중. 전환을 고려해 보세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime}에서 {duration} 동안 집중하셨습니다. 괜찮으신가요?",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: {duration} 경과. 일반적인 세션: {comparison}.",
    },
    // ── ContextRestore x RegimeDrift ──
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "컨텍스트가 분산되고 있습니다. 참고: {previous_context} 중이었습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "주의가 분산된 것 같습니다. 이전 컨텍스트는 {previous_context}였습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::ContextRestore,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{previous_context}에서 분산. 이 세션에서 {context_switches}회 전환.",
    },
    // ── DeepWorkCoach x RegimeDrift ──
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "딥워크 중 주의 분산 감지. {app_name}을 닫고 다시 집중하세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "딥워크 흐름이 중단되었습니다. 다시 돌아가시겠어요?",
    },
    CoachingTemplate {
        profile: CoachingProfile::DeepWorkCoach,
        trigger_type: "RegimeDrift",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "딥워크 분산: {context_switches}회 전환. 평균 무중단 시간: {comparison}.",
    },
    // ── TimeAware x RegimeTransition ──
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{duration} 후 {regime}에서 전환.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "{regime}에서 체제 변경. {duration} 동안 머물렀습니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::TimeAware,
        trigger_type: "RegimeTransition",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime} 세션 종료: {duration}. 평균: {comparison}.",
    },
    // ── GoalTracker x RegimeOverstay ──
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "{regime} 초과 체류: {duration}. 오늘 목표는 {goal_minutes}분입니다.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "긴 {regime} 세션 ({duration}). {goal_minutes}분 목표 중 {goal_progress}분 기록.",
    },
    CoachingTemplate {
        profile: CoachingProfile::GoalTracker,
        trigger_type: "RegimeOverstay",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "{regime}: {duration} 세션. 일일 합계: {goal_progress}/{goal_minutes}분 ({goal_progress}%).",
    },
    // ── FocusGuard x GoalThreshold ──
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Direct,
        locale: "ko",
        text: "집중 목표: {goal_minutes}분의 {goal_progress}%. 작업에 집중하세요.",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::Gentle,
        locale: "ko",
        text: "집중 목표의 {goal_progress}% 달성 중. 계속 화이팅!",
    },
    CoachingTemplate {
        profile: CoachingProfile::FocusGuard,
        trigger_type: "GoalThreshold",
        tone: CoachingTone::DataDriven,
        locale: "ko",
        text: "집중 시간: {goal_minutes}분 목표의 {goal_progress}%. {remaining_minutes}분 남음.",
    },
];
