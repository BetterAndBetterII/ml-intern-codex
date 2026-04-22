//! Normalize local filesystem paths into the display-stable text form used by the TUI.
//!
//! This is purely lexical. It does not touch the filesystem, expand `~`, or collapse `.` / `..`.
//! The goal is to keep user-visible path text stable across platforms and avoid leaking Windows
//! verbatim path prefixes such as `\\?\`.

use std::path::Path;

/// Normalize a local filesystem path into the transcript/history text form used by the TUI.
///
/// On Windows, canonicalized paths can include verbatim prefixes like `\\?\C:\...` or
/// `\\?\UNC\server\share\...`. This strips those prefixes and rewrites separators to forward
/// slashes so markdown links and transcript path rendering stay user-friendly.
pub fn normalize_local_path(path: &Path) -> String {
    normalize_local_path_text(&path.to_string_lossy())
}

/// Normalize local-path text into the transcript/history text form used by the TUI.
///
/// UNC-style paths are rendered with a stable `//server/share/...` prefix, and Windows verbatim
/// prefixes are removed after separator normalization so mixed `\` / `/` inputs normalize through
/// one lexical path.
pub fn normalize_local_path_text(path_text: &str) -> String {
    let normalized = strip_windows_verbatim_prefix(path_text.replace('\\', "/"));
    if let Some(rest) = normalized.strip_prefix("//") {
        format!("//{}", rest.trim_start_matches('/'))
    } else {
        normalized
    }
}

fn strip_windows_verbatim_prefix(path_text: String) -> String {
    if let Some(rest) = path_text.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = path_text.strip_prefix("/?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = path_text.strip_prefix("//?/") {
        return rest.to_string();
    }
    if let Some(rest) = path_text.strip_prefix("/?/") {
        return rest.to_string();
    }
    path_text
}

#[cfg(test)]
mod tests {
    use super::normalize_local_path_text;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalize_local_path_text_strips_supported_windows_verbatim_prefixes() {
        for (input, expected) in [
            (
                r"\\?\C:\Users\me\repo\file.txt",
                "C:/Users/me/repo/file.txt",
            ),
            (r"\\?\UNC\server\share\file.txt", "//server/share/file.txt"),
            (r"\?\C:\Users\me\repo\file.txt", "C:/Users/me/repo/file.txt"),
            (r"\?\UNC\server\share\file.txt", "//server/share/file.txt"),
            (r"\?/C:/Users/me/repo/file.txt", "C:/Users/me/repo/file.txt"),
            (r"\?/UNC/server/share/file.txt", "//server/share/file.txt"),
        ] {
            assert_eq!(normalize_local_path_text(input), expected, "input: {input}");
        }
    }

    #[test]
    fn normalize_local_path_text_keeps_unc_roots_stable() {
        assert_eq!(
            normalize_local_path_text(r"\\server\share\file.txt"),
            "//server/share/file.txt"
        );
    }
}
