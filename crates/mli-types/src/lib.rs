//! Shared domain types for ml-intern-codex.

use std::time::SystemTime;

use chrono::{DateTime, Utc};

pub mod artifacts;
pub mod ids;
pub mod skills;
pub mod threads;
pub mod ui;

pub use artifacts::*;
pub use ids::*;
pub use skills::*;
pub use threads::*;
pub use ui::*;

pub fn utc_now() -> DateTime<Utc> {
    DateTime::<Utc>::from(SystemTime::now())
}
