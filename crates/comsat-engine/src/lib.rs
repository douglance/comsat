#![forbid(unsafe_code)]

mod catalog;
#[cfg(test)]
mod catalog_selection_tests;
#[cfg(test)]
mod catalog_tests;
mod dedupe;
mod invoke;
mod merge;
mod metrics;
mod plan;

pub use catalog::{CatalogSource, SourceCatalog};
pub use dedupe::DedupeSet;
pub use merge::{
    EngineDiagnostic, EngineError, EngineEvent, EngineLimits, SearchOutcome, SearchRequest,
};
pub use metrics::{OperationMetricsSnapshot, SourceCatalogMetrics, SourceMetricsSnapshot};
