//! Transcript-first TUI for ml-intern-codex.

pub mod app;
pub mod renderer;

// Rendering primitives ported from CodexPotter (Apache 2.0).
pub mod bottom_pane;
pub mod color;
pub mod custom_terminal;
pub mod history_cell;
pub mod inline_tui;
pub(crate) mod overlay;
pub mod human_time;
pub mod insert_history;
pub mod key_hint;
pub mod local_path;
pub mod markdown;
pub mod markdown_render;
pub mod markdown_stream;
pub mod render;
pub mod style;
pub mod terminal_cleanup;
pub mod terminal_palette;
pub mod tui_session;
pub mod token_format;
pub mod ui_colors;
pub mod ui_consts;
pub mod wrapping;

pub use app::*;
pub use renderer::*;
