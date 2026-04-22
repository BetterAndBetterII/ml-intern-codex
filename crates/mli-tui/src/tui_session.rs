//! Terminal session for the inline-viewport TUI.
//!
//! Enables raw mode + bracketed paste + keyboard enhancement flags, installs a
//! panic hook that restores the terminal, and wraps a [`CustomTerminal`] around
//! the current stdout. Deliberately *does not* enter the alternate screen —
//! the inline viewport and `insert_history_lines` rely on writing directly to
//! the shell scrollback.

use std::io::{self, Stdout, stdout};
use std::panic;
use std::sync::OnceLock;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;

use crate::custom_terminal::Terminal as CustomTerminal;

pub type Terminal = CustomTerminal<CrosstermBackend<Stdout>>;

pub fn enter() -> io::Result<Terminal> {
    install_panic_hook();

    enable_raw_mode()?;
    execute!(stdout(), EnableBracketedPaste)?;
    // Best effort: some terminals don't support these flags.
    let _ = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );
    let _ = execute!(stdout(), EnableFocusChange);

    let backend = CrosstermBackend::new(stdout());
    CustomTerminal::with_options(backend)
}

pub fn restore() -> io::Result<()> {
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(stdout(), DisableFocusChange);
    let _ = execute!(stdout(), DisableBracketedPaste);
    // Inline mode doesn't enter alt-screen, but be defensive on early exits.
    let _ = execute!(stdout(), LeaveAlternateScreen);
    disable_raw_mode()?;
    let _ = execute!(stdout(), crossterm::cursor::Show);
    Ok(())
}

fn install_panic_hook() {
    static HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
    HOOK_INSTALLED.get_or_init(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = restore();
            previous(info);
        }));
    });
}
