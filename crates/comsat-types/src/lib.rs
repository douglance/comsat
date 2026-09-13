//! Canonical information values. No invocation, transport, or persistence dependencies.
#![forbid(unsafe_code)]

mod error;
mod identity;
mod query;
mod record;
mod target;

pub use error::{ErrorClass, SourceError, ValidationError};
pub use identity::{RecordId, SourceId};
pub use query::{Query, validate_timestamp};
pub use record::Record;
pub use target::Target;
