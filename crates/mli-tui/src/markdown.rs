use ratatui::text::Line;
use std::path::Path;

/// Render markdown into `lines` while resolving local file-link display relative to `cwd`.
///
/// Callers that already know the session working directory should pass it here so streamed and
/// non-streamed rendering show the same relative path text even if the process cwd differs.
pub fn append_markdown(
    markdown_source: &str,
    width: Option<usize>,
    cwd: Option<&Path>,
    lines: &mut Vec<Line<'static>>,
) {
    let rendered = crate::markdown_render::render_markdown_text_with_width_and_cwd(
        markdown_source,
        width,
        cwd,
    );
    crate::render::line_utils::push_owned_lines(&rendered.lines, lines);
}

