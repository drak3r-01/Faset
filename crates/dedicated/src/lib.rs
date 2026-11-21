//! Dedicated server executable: configuration ([`settings`]), console
//! ([`console`]), map catalog ([`maps`]) and the run loop ([`runner`]).
//!
//! No graphics or audio crate is linked (checked by `tests/no_graphics.rs`).
#![forbid(unsafe_code)]

pub mod console;
pub mod maps;
pub mod runner;
pub mod settings;

pub use console::{parse as parse_command, Command};
pub use runner::{run, Dedicated};
pub use settings::{Cli, Settings, SettingsError};
