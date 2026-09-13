use std::sync::Arc;

use comsat_source::SourceDescriptor;
use comsat_types::{SourceId, Target};
use incurs::tool::ToolCatalog;

use crate::merge::{EngineError, EngineLimits, SearchRequest, follow_stream, search_stream};
use crate::metrics::{EngineMetrics, SourceCatalogMetrics};
use crate::plan::validate_unique_sources;

#[derive(Clone)]
pub struct CatalogSource {
    pub descriptor: SourceDescriptor,
    pub catalog: Arc<ToolCatalog>,
}

impl CatalogSource {
    #[must_use]
    pub fn new(descriptor: SourceDescriptor, catalog: ToolCatalog) -> Self {
        Self {
            descriptor,
            catalog: Arc::new(catalog),
        }
    }
}

#[derive(Clone, Default)]
pub struct SourceCatalog {
    sources: Vec<CatalogSource>,
    metrics: EngineMetrics,
}

impl SourceCatalog {
    pub fn try_new(sources: Vec<CatalogSource>) -> Result<Self, EngineError> {
        validate_unique_sources(&sources)?;
        let source_ids = sources
            .iter()
            .map(|source| source.descriptor.id.clone())
            .collect::<Vec<_>>();
        Ok(Self {
            sources,
            metrics: EngineMetrics::new(source_ids),
        })
    }

    #[must_use]
    pub fn new(sources: Vec<CatalogSource>) -> Self {
        Self::try_new(sources).expect("source catalog must not contain duplicate source ids")
    }

    #[must_use]
    pub fn sources(&self) -> &[CatalogSource] {
        &self.sources
    }

    pub fn source(&self, source: &SourceId) -> Option<&CatalogSource> {
        self.sources
            .iter()
            .find(|entry| entry.descriptor.id == *source)
    }

    pub fn metrics(&self) -> SourceCatalogMetrics {
        self.metrics.snapshot()
    }

    pub fn search_stream(
        &self,
        request: SearchRequest,
        limits: EngineLimits,
    ) -> impl futures::Stream<Item = crate::EngineEvent> + 'static {
        search_stream(&self.sources, request, limits, self.metrics.clone())
    }

    pub fn follow_stream(
        &self,
        target: Target,
        limits: EngineLimits,
    ) -> impl futures::Stream<Item = crate::EngineEvent> + 'static {
        follow_stream(&self.sources, target, limits, self.metrics.clone())
    }

    pub async fn collect_search(
        &self,
        request: SearchRequest,
        limits: EngineLimits,
    ) -> crate::SearchOutcome {
        use futures::StreamExt;

        let mut stream = Box::pin(self.search_stream(request, limits));
        let mut records = Vec::new();
        let mut diagnostics = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                crate::EngineEvent::Record(record) => records.push(record),
                crate::EngineEvent::SourceFailed(diagnostic) => diagnostics.push(diagnostic),
            }
        }
        crate::SearchOutcome {
            records,
            diagnostics,
        }
    }

    pub async fn collect_follow(
        &self,
        target: Target,
        limits: EngineLimits,
    ) -> crate::SearchOutcome {
        use futures::StreamExt;

        let mut stream = Box::pin(self.follow_stream(target, limits));
        let mut records = Vec::new();
        let mut diagnostics = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                crate::EngineEvent::Record(record) => records.push(record),
                crate::EngineEvent::SourceFailed(diagnostic) => diagnostics.push(diagnostic),
            }
        }
        crate::SearchOutcome {
            records,
            diagnostics,
        }
    }

    pub async fn fetch(
        &self,
        target: Target,
        limits: EngineLimits,
    ) -> Result<comsat_types::Record, EngineError> {
        crate::invoke::fetch(self, target, limits, self.metrics.clone()).await
    }
}
