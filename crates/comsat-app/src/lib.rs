#![forbid(unsafe_code)]

mod app;
mod commands;
pub mod notifications;

#[cfg(test)]
mod tests;

pub use app::{AppError, AppResult, ComsatApp, TargetStream, TargetStreamProvider};
pub use commands::build_cli;
