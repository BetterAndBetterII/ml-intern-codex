use std::io;
use std::io::Write;

use ratatui::backend::Backend;

/// Clears the current inline viewport so the shell prompt is clean after the TUI exits.
pub fn clear_inline_viewport_for_exit<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    terminal.clear()?;
    ratatui::backend::Backend::flush(terminal.backend_mut())
}
