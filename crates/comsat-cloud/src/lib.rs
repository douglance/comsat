#![forbid(unsafe_code)]

pub mod auth;

#[cfg(any(target_arch = "wasm32", test))]
mod landing;

#[cfg(any(target_arch = "wasm32", test))]
mod queue_policy;
#[cfg(any(target_arch = "wasm32", test))]
mod tenant_rotation;

#[cfg(target_arch = "wasm32")]
mod cloud_config;
#[cfg(target_arch = "wasm32")]
mod cloud_http;
#[cfg(target_arch = "wasm32")]
mod cloud_notifications;
#[cfg(target_arch = "wasm32")]
mod d1_rows;
#[cfg(target_arch = "wasm32")]
mod d1_store;
#[cfg(target_arch = "wasm32")]
mod runtime;
#[cfg(target_arch = "wasm32")]
mod runtime_http;
#[cfg(target_arch = "wasm32")]
mod runtime_watch;
#[cfg(target_arch = "wasm32")]
mod source_graph;
#[cfg(target_arch = "wasm32")]
mod telemetry;
#[cfg(target_arch = "wasm32")]
mod worker_entry;

#[cfg(target_arch = "wasm32")]
pub use d1_store::D1Store;
#[cfg(target_arch = "wasm32")]
pub use runtime::{WatchClaimMessage, route_request};
