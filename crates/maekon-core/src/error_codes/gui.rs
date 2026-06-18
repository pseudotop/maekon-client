//! GuiCode — GUI interaction category error codes. `gui.*` prefix.
//!
//! Used by `GuiInteractionError`.

define_code_enum! {
    /// GUI category error codes.
    pub enum GuiCode {
        /// GUI session token is invalid.
        Unauthorized => "gui.unauthorized",
        /// GUI session not found.
        NotFound => "gui.not_found",
        /// GUI request is malformed.
        BadRequest => "gui.bad_request",
        /// GUI request forbidden.
        Forbidden => "gui.forbidden",
        /// GUI focus drift detected.
        FocusDrift => "gui.focus_drift",
        /// GUI ticket is no longer valid.
        TicketInvalid => "gui.ticket_invalid",
        /// GUI runtime unavailable.
        Unavailable => "gui.unavailable",
        /// GUI runtime internal error.
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
