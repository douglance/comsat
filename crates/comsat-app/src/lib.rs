#![forbid(unsafe_code)]

mod app;
mod commands;
pub mod notifications;
#[cfg(feature = "serve")]
mod serve;

#[cfg(test)]
mod tests;

pub use app::{AppError, AppResult, ComsatApp, TargetStream, TargetStreamProvider};
pub use commands::build_cli;
#[cfg(feature = "serve")]
pub use serve::{DEFAULT_ADDRESS, NoServeHooks, ServeHooks, build_serving_cli};
