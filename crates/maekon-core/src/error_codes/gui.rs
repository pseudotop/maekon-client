//! GuiCode — GUI interaction 카테고리 에러 코드. `gui.*` 접두사.
//!
//! `GuiInteractionError`에서 사용.

define_code_enum! {
    /// GUI 카테고리 에러 코드.
    pub enum GuiCode {
        /// GUI 세션 토큰 유효하지 않음.
        Unauthorized => "gui.unauthorized",
        /// GUI 세션을 찾을 수 없음.
        NotFound => "gui.not_found",
        /// GUI 요청이 잘못됨.
        BadRequest => "gui.bad_request",
        /// GUI 요청 금지됨.
        Forbidden => "gui.forbidden",
        /// GUI 포커스 드리프트 감지됨.
        FocusDrift => "gui.focus_drift",
        /// GUI 티켓이 더 이상 유효하지 않음.
        TicketInvalid => "gui.ticket_invalid",
        /// GUI 런타임 사용 불가.
        Unavailable => "gui.unavailable",
        /// GUI 런타임 내부 오류.
        InternalError => "gui.internal_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = GuiCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in GuiCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in GuiCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
