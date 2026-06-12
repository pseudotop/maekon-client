use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn is_locale_file(path: &Path) -> bool {
    path.to_string_lossy().contains("/src/i18n/locales/")
}

pub(crate) fn is_ignored(path: &Path, ignores: &[String]) -> bool {
    if ignores.is_empty() {
        return false;
    }
    let path_str = path.to_string_lossy();
    ignores.iter().any(|needle| path_str.contains(needle))
}

pub(crate) fn first_non_ascii(line: &str) -> Option<(usize, char)> {
    for (idx, ch) in line.char_indices() {
        if !ch.is_ascii() {
            return Some((idx + 1, ch));
        }
    }
    None
}

pub(crate) fn collect_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut extension_set: HashSet<&str> = HashSet::new();
    extension_set.extend(extensions.iter().copied());
    collect_files_recursive(root, &extension_set, &mut files);
    files
}

fn collect_files_recursive(root: &Path, extensions: &HashSet<&str>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if file_name == ".git"
            || file_name == "target"
            || file_name == "node_modules"
            || file_name == "dist"
        {
            continue;
        }

        if path.is_dir() {
            collect_files_recursive(&path, extensions, out);
            continue;
        }

        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if extensions.contains(ext) {
            out.push(path);
        }
    }
}
