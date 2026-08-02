//! Core library for Agent Governor.

pub mod config;
pub mod doctor;
pub mod error;
pub mod hook;
pub mod install;
pub mod scheduler;
pub mod shell;
pub mod status;
pub mod supervisor;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
