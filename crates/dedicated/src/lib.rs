//! Dedicated server executable: configuration ([`settings`]), console
//! ([`console`]), map catalog ([`maps`]), the run loop ([`runner`]) and,
//! since lot 4.11, the hosted FPS match with [`fps_bot`] brains as virtual
//! clients.
//!
//! No graphics or audio crate is linked (checked by `tests/no_graphics.rs`).
#![forbid(unsafe_code)]

pub mod console;
pub mod fps_bot;
pub mod maps;
pub mod provider;
pub mod runner;
pub mod settings;

pub use console::{parse as parse_command, Command};
pub use runner::{run, Dedicated};
pub use settings::{Cli, Settings, SettingsError};
