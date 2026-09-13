use std::pin::Pin;

use async_trait::async_trait;
use comsat_types::{Query, Record, SourceError, Target};
use futures::Stream;

pub type RecordStream = Pin<Box<dyn Stream<Item = Result<Record, SourceError>> + Send>>;

#[derive(Debug, Clone, Default)]
pub struct SourceRunContext {
    _private: (),
}

#[async_trait]
pub trait SourceRuntime: Send + Sync {
    async fn search(&self, query: Query, context: SourceRunContext) -> RecordStream;

    async fn fetch(&self, target: Target, context: SourceRunContext)
    -> Result<Record, SourceError>;

    async fn follow(&self, target: Target, context: SourceRunContext) -> RecordStream;
}
