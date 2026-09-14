#![forbid(unsafe_code)]

mod command;
mod conformance;
mod descriptor;
mod http;
mod runtime;

pub use command::{SourceCommand, source_commands};
pub use conformance::{
    CancellationFixture, ConformanceError, ConformanceFixtures, ConformanceReport,
    ConformanceSuite, SourceErrorFixture,
};
pub use descriptor::{OperationKind, SourceCommandNames, SourceDescriptor, SourceProfile};
pub use http::HttpClient;
pub use runtime::{RecordStream, SourceRunContext, SourceRuntime};

pub use comsat_types::{ErrorClass, SourceError};

pub type SourceResult<T> = Result<T, SourceError>;
