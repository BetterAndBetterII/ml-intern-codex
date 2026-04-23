use ratatui::text::Line;
use std::path::Path;
use std::path::PathBuf;

use crate::markdown;

/// Newline-gated accumulator that renders markdown and commits only fully
/// completed logical lines.
pub struct MarkdownStreamCollector {
    buffer: String,
    committed_line_count: usize,
    width: Option<usize>,
    cwd: PathBuf,
}

impl MarkdownStreamCollector {
    /// Create a collector that renders markdown using `cwd` for local file-link display.
    ///
    /// The collector snapshots `cwd` into owned state because stream commits can happen long after
    /// construction. The same `cwd` should be reused for the entire stream lifecycle; mixing
    /// different working directories within one stream would make the same link render with
    /// different path prefixes across incremental commits.
    pub fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            buffer: String::new(),
            committed_line_count: 0,
            width,
            cwd: cwd.to_path_buf(),
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.committed_line_count = 0;
    }

    pub fn push_delta(&mut self, delta: &str) {
        tracing::trace!("push_delta: {delta:?}");
        self.buffer.push_str(delta);
    }

    /// Render the full buffer and return only the newly completed logical lines
    /// since the last commit. When the buffer does not end with a newline, the
    /// final rendered line is considered incomplete and is not emitted.
    pub fn commit_complete_lines(&mut self) -> Vec<Line<'static>> {
        let Some(committable_end) = last_committable_prefix_end(&self.buffer) else {
            return Vec::new();
        };
        let source = self.buffer[..committable_end].to_string();
        let rendered = self.render_markdown(&source);
        let mut complete_line_count = rendered.len();
        if complete_line_count > 0
            && crate::render::line_utils::is_blank_line_spaces_only(
                &rendered[complete_line_count - 1],
            )
        {
            complete_line_count -= 1;
        }

        if self.committed_line_count >= complete_line_count {
            return Vec::new();
        }

        let out_slice = &rendered[self.committed_line_count..complete_line_count];

        let out = out_slice.to_vec();
        self.committed_line_count = complete_line_count;
        out
    }

    /// Finalize the stream: emit all remaining lines beyond the last commit.
    /// If the buffer does not end with a newline, a temporary one is appended
    /// for rendering. Optionally unwraps ```markdown language fences in
    /// non-test builds.
    pub fn finalize_and_drain(&mut self) -> Vec<Line<'static>> {
        let raw_buffer = self.buffer.clone();
        let mut source: String = raw_buffer.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        tracing::debug!(
            raw_len = raw_buffer.len(),
            source_len = source.len(),
            "markdown finalize (raw length: {}, rendered length: {})",
            raw_buffer.len(),
            source.len()
        );
        tracing::trace!("markdown finalize (raw source):\n---\n{source}\n---");

        let rendered = self.render_markdown(&source);

        let out = if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        };

        // Reset collector state for next stream.
        self.clear();
        out
    }

    fn render_markdown(&self, source: &str) -> Vec<Line<'static>> {
        let mut rendered: Vec<Line<'static>> = Vec::new();
        markdown::append_markdown(source, self.width, Some(self.cwd.as_path()), &mut rendered);
        rendered
    }
}

fn last_committable_prefix_end(source: &str) -> Option<usize> {
    let mut cursor = 0usize;
    let mut last_safe = None;
    let mut fenced_code_block_marker: Option<&'static str> = None;

    while cursor < source.len() {
        let newline_offset = source[cursor..].find('\n')?;
        let line_end = cursor + newline_offset + 1;
        let line = &source[cursor..line_end];
        let trimmed_line = line.trim_end_matches('\n');
        let marker = trimmed_line.trim_start();

        let fence_marker = fenced_code_marker(marker);
        if let Some(fence) = fence_marker {
            if let Some(open_marker) = fenced_code_block_marker {
                if fence.starts_with(open_marker) {
                    fenced_code_block_marker = None;
                    last_safe = Some(line_end);
                }
            } else {
                fenced_code_block_marker = Some(if fence.starts_with("```") {
                    "```"
                } else {
                    "~~~"
                });
            }
        } else if fenced_code_block_marker.is_none() {
            if marker.trim().is_empty() {
                last_safe = Some(line_end);
            } else {
                let next_line = source[line_end..]
                    .split_once('\n')
                    .map(|(next, _)| next)
                    .unwrap_or(&source[line_end..]);
                if starts_new_markdown_block(next_line) {
                    last_safe = Some(line_end);
                }
            }
        }

        cursor = line_end;
    }

    last_safe
}

fn fenced_code_marker(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("```") {
        return Some(&line[..line.len() - rest.len()]);
    }
    if let Some(rest) = line.strip_prefix("~~~") {
        return Some(&line[..line.len() - rest.len()]);
    }
    None
}

fn starts_new_markdown_block(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    if matches!(trimmed.chars().next(), Some('>' | '#' | '-' | '*' | '+')) {
        if trimmed.starts_with('>') || trimmed.starts_with('#') {
            return true;
        }
        if trimmed.len() >= 2
            && matches!(trimmed.as_bytes()[0], b'-' | b'*' | b'+')
            && trimmed.as_bytes()[1].is_ascii_whitespace()
        {
            return true;
        }
    }
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return true;
    }

    let mut digits = 0usize;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
            continue;
        }
        if digits > 0 && matches!(ch, '.' | ')') {
            let rest = &trimmed[digits + 1..];
            return rest.chars().next().is_some_and(char::is_whitespace);
        }
        break;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_to_strings(lines: Vec<Line<'static>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn commit_complete_lines_waits_for_soft_break_list_item_to_finish() {
        let cwd = std::env::temp_dir();
        let mut collector = MarkdownStreamCollector::new(Some(80), &cwd);

        collector.push_delta("- memory\n");
        assert!(collector.commit_complete_lines().is_empty());

        collector.push_delta("schema\n\n");
        assert_eq!(
            lines_to_strings(collector.commit_complete_lines()),
            vec!["- memory schema".to_string()]
        );
    }

    #[test]
    fn commit_complete_lines_keeps_cjk_soft_breaks_in_same_list_item() {
        let cwd = std::env::temp_dir();
        let mut collector = MarkdownStreamCollector::new(Some(80), &cwd);

        collector.push_delta("- 得\n");
        assert!(collector.commit_complete_lines().is_empty());

        collector.push_delta("动\n\n");
        assert_eq!(
            lines_to_strings(collector.commit_complete_lines()),
            vec!["- 得 动".to_string()]
        );
    }

    #[test]
    fn commit_complete_lines_flushes_when_next_markdown_block_starts() {
        let cwd = std::env::temp_dir();
        let mut collector = MarkdownStreamCollector::new(Some(80), &cwd);

        collector.push_delta("- first item\n");
        assert!(collector.commit_complete_lines().is_empty());

        collector.push_delta("- second item\n");
        assert_eq!(
            lines_to_strings(collector.commit_complete_lines()),
            vec!["- first item".to_string()]
        );
    }
}
