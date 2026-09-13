use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use comsat_source::OperationKind;
use comsat_types::{ErrorClass, SourceId};
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceCatalogMetrics {
    pub sources: BTreeMap<String, SourceMetricsSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceMetricsSnapshot {
    pub search: OperationMetricsSnapshot,
    pub fetch: OperationMetricsSnapshot,
    pub follow: OperationMetricsSnapshot,
    pub records_emitted: u64,
    pub records_deduplicated: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OperationMetricsSnapshot {
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub rate_limits: u64,
    pub cancelled: u64,
}

#[derive(Clone, Default)]
pub struct EngineMetrics {
    inner: Arc<Mutex<BTreeMap<SourceId, SourceMetrics>>>,
}

impl EngineMetrics {
    pub fn new(sources: impl IntoIterator<Item = SourceId>) -> Self {
        let metrics = sources
            .into_iter()
            .map(|source| (source, SourceMetrics::default()))
            .collect();
        Self {
            inner: Arc::new(Mutex::new(metrics)),
        }
    }

    pub fn snapshot(&self) -> SourceCatalogMetrics {
        let sources = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(source, metrics)| (source.as_str().to_owned(), metrics.snapshot()))
            .collect();
        SourceCatalogMetrics { sources }
    }

    pub fn start(&self, source: SourceId, operation: OperationKind) -> OperationGuard {
        self.update(&source, |metrics| {
            metrics.operation_mut(operation).requests += 1;
        });
        OperationGuard {
            metrics: self.clone(),
            source,
            operation,
            completed: false,
        }
    }

    pub fn record_emitted(&self, source: &SourceId) {
        self.update(source, |metrics| {
            metrics.records_emitted = metrics.records_emitted.saturating_add(1);
        });
    }

    pub fn record_deduplicated(&self, source: &SourceId) {
        self.update(source, |metrics| {
            metrics.records_deduplicated = metrics.records_deduplicated.saturating_add(1);
        });
    }

    fn terminal(&self, source: &SourceId, operation: OperationKind, outcome: TerminalOutcome) {
        self.update(source, |metrics| {
            let operation = metrics.operation_mut(operation);
            match outcome {
                TerminalOutcome::Success => operation.successes += 1,
                TerminalOutcome::Cancelled => operation.cancelled += 1,
                TerminalOutcome::Failure { rate_limited } => {
                    operation.failures += 1;
                    if rate_limited {
                        operation.rate_limits += 1;
                    }
                }
            }
        });
    }

    fn update(&self, source: &SourceId, update: impl FnOnce(&mut SourceMetrics)) {
        let mut metrics = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(source_metrics) = metrics.get_mut(source) {
            update(source_metrics);
        }
    }
}

pub struct OperationGuard {
    metrics: EngineMetrics,
    source: SourceId,
    operation: OperationKind,
    completed: bool,
}

impl OperationGuard {
    pub fn success(mut self) {
        self.completed = true;
        self.metrics
            .terminal(&self.source, self.operation, TerminalOutcome::Success);
    }

    pub fn failure(mut self, class: ErrorClass) {
        self.completed = true;
        self.metrics.terminal(
            &self.source,
            self.operation,
            TerminalOutcome::Failure {
                rate_limited: class == ErrorClass::RateLimit,
            },
        );
    }

    pub fn cancelled(mut self) {
        self.completed = true;
        self.metrics
            .terminal(&self.source, self.operation, TerminalOutcome::Cancelled);
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.metrics
                .terminal(&self.source, self.operation, TerminalOutcome::Cancelled);
        }
    }
}

#[derive(Default)]
struct SourceMetrics {
    search: OperationMetricsSnapshot,
    fetch: OperationMetricsSnapshot,
    follow: OperationMetricsSnapshot,
    records_emitted: u64,
    records_deduplicated: u64,
}

impl SourceMetrics {
    const fn operation_mut(&mut self, operation: OperationKind) -> &mut OperationMetricsSnapshot {
        match operation {
            OperationKind::Search => &mut self.search,
            OperationKind::Fetch => &mut self.fetch,
            OperationKind::Follow => &mut self.follow,
        }
    }

    const fn snapshot(&self) -> SourceMetricsSnapshot {
        SourceMetricsSnapshot {
            search: self.search,
            fetch: self.fetch,
            follow: self.follow,
            records_emitted: self.records_emitted,
            records_deduplicated: self.records_deduplicated,
        }
    }
}

#[derive(Clone, Copy)]
enum TerminalOutcome {
    Success,
    Failure { rate_limited: bool },
    Cancelled,
}
