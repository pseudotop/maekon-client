use super::should_exclude;

#[test]
fn exact_excluded_app_matches_windows_executable_suffix() {
    assert!(should_exclude(
        "Notepad.exe",
        "Untitled - Notepad",
        &["Notepad".to_string()],
        &[],
        &[],
        false,
    ));
    assert!(should_exclude(
        "notepad",
        "Untitled",
        &["NOTEPAD.EXE".to_string()],
        &[],
        &[],
        false,
    ));
}

#[test]
fn exact_excluded_app_does_not_become_a_substring_match() {
    assert!(!should_exclude(
        "NotepadPlus.exe",
        "Untitled",
        &["Notepad".to_string()],
        &[],
        &[],
        false,
    ));
}
