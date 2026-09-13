use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use futures::Stream;
use futures::channel::mpsc;
use futures::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;

use crate::catalog::CatalogSource;
use crate::dedupe::DedupeSet;
use crate::invoke::{SourceInvocation, WorkerEvent, run_source_operation};
use crate::metrics::EngineMetrics;
use crate::plan::{PlannedSource, StreamPlan, plan_sources};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: Query,
    pub sources: Vec<SourceId>,
    pub strict: bool,
}

impl SearchRequest {
    #[must_use]
    pub const fn all(query: Query) -> Self {
        Self {
            query,
            sources: Vec::new(),
            strict: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLimits {
    pub max_active_sources: usize,
    pub event_buffer: usize,
    pub max_records_per_source: usize,
    pub max_bytes_per_source: usize,
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self {
            max_active_sources: 4,
            event_buffer: 32,
            max_records_per_source: 1_000,
            max_bytes_per_source: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    Record(Record),
    SourceFailed(EngineDiagnostic),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDiagnostic {
    pub source: SourceId,
    pub class: ErrorClass,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

impl From<SourceError> for EngineDiagnostic {
    fn from(error: SourceError) -> Self {
        Self {
            source: error.source,
            class: error.class,
            message: error.message,
            retry_after_seconds: error.retry_after_seconds,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("source not found: {0}")]
    SourceNotFound(SourceId),
    #[error("duplicate source id: {0}")]
    DuplicateSourceId(SourceId),
    #[error("source does not support {operation}: {source_id}")]
    Unsupported {
        source_id: SourceId,
        operation: &'static str,
    },
    #[error("{0}")]
    Source(#[from] SourceError),
    #[error("strict search failed")]
    StrictSearchFailed(Vec<EngineDiagnostic>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    pub records: Vec<Record>,
    pub diagnostics: Vec<EngineDiagnostic>,
}

pub fn search_stream(
    sources: &[CatalogSource],
    request: SearchRequest,
    limits: EngineLimits,
    metrics: EngineMetrics,
) -> SearchDriver {
    SearchDriver::new(sources, StreamPlan::Search(request), limits, metrics)
}

pub fn follow_stream(
    sources: &[CatalogSource],
    target: Target,
    limits: EngineLimits,
    metrics: EngineMetrics,
) -> SearchDriver {
    SearchDriver::new(sources, StreamPlan::Follow(target), limits, metrics)
}

pub struct SearchDriver {
    initial: VecDeque<WorkerEvent>,
    pending: VecDeque<PlannedSource>,
    running: Vec<RunningSource>,
    token: CancellationToken,
    dedupe: DedupeSet,
    limits: EngineLimits,
    max_active: usize,
    strict: bool,
    stopping: bool,
    cursor: usize,
    metrics: EngineMetrics,
}

impl SearchDriver {
    fn new(
        sources: &[CatalogSource],
        plan: StreamPlan,
        limits: EngineLimits,
        metrics: EngineMetrics,
    ) -> Self {
        let (pending, diagnostics, strict) = plan_sources(sources, plan);
        let token = CancellationToken::new();
        let initial = diagnostics
            .into_iter()
            .map(WorkerEvent::Failed)
            .collect::<VecDeque<_>>();
        Self {
            initial,
            pending,
            running: Vec::new(),
            token,
            dedupe: DedupeSet::default(),
            max_active: limits.max_active_sources.max(1),
            limits,
            strict,
            stopping: false,
            cursor: 0,
            metrics,
        }
    }

    fn start_ready_sources(&mut self) {
        while !self.stopping && self.running.len() < self.max_active {
            let Some(source) = self.pending.pop_front() else {
                break;
            };
            let (tx, rx) = mpsc::channel(self.limits.event_buffer.max(1));
            let invocation = SourceInvocation {
                source: source.source,
                operation: source.operation,
                input: source.input,
                limits: self.limits,
                metrics: self.metrics.clone(),
            };
            let future = run_source_operation(invocation, tx, self.token.clone()).boxed();
            self.running.push(RunningSource {
                rx,
                future: Some(future),
            });
        }
    }

    fn map_event(&mut self, event: WorkerEvent) -> Option<EngineEvent> {
        match event {
            WorkerEvent::Record { source, record } => {
                if self.dedupe.insert(&record) {
                    self.metrics.record_emitted(&source);
                    Some(EngineEvent::Record(record))
                } else {
                    self.metrics.record_deduplicated(&source);
                    None
                }
            }
            WorkerEvent::Failed(diagnostic) => {
                if self.strict {
                    self.stop();
                }
                Some(EngineEvent::SourceFailed(diagnostic))
            }
        }
    }

    fn stop(&mut self) {
        self.stopping = true;
        self.pending.clear();
        self.running.clear();
        self.token.cancel();
    }

    fn poll_available_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<EngineEvent>> {
        if let Some(event) = self.next_initial_event() {
            return Poll::Ready(Some(event));
        }
        self.poll_ordered_source_event(cx)
    }

    fn next_initial_event(&mut self) -> Option<EngineEvent> {
        while let Some(event) = self.initial.pop_front() {
            if let Some(event) = self.map_event(event) {
                return Some(event);
            }
        }
        None
    }

    fn poll_ordered_source_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<EngineEvent>> {
        loop {
            if self.running.is_empty() {
                return Poll::Pending;
            }
            match self.poll_current_source(cx) {
                SourcePoll::Event(event) => return Poll::Ready(Some(*event)),
                SourcePoll::Advanced => {}
                SourcePoll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_workers(&mut self, cx: &mut Context<'_>) -> WorkerPoll {
        let mut progressed = false;
        for source in &mut self.running {
            if source.poll_future(cx).is_ready() {
                progressed = true;
            }
        }
        if progressed {
            WorkerPoll::Progress
        } else {
            WorkerPoll::Pending
        }
    }

    fn poll_current_source(&mut self, cx: &mut Context<'_>) -> SourcePoll {
        self.normalize_cursor();
        if self.running.is_empty() {
            return SourcePoll::Pending;
        }
        let index = self.cursor;
        match self.poll_source_receiver(index, cx) {
            Poll::Ready(Some(event)) => self.consume_source_event(event),
            Poll::Ready(None) if self.running[index].is_closed() => {
                self.remove_current_source();
                SourcePoll::Advanced
            }
            Poll::Ready(None) | Poll::Pending => SourcePoll::Pending,
        }
    }

    fn poll_source_receiver(
        &mut self,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Option<WorkerEvent>> {
        Pin::new(&mut self.running[index].rx).poll_next(cx)
    }

    fn consume_source_event(&mut self, event: WorkerEvent) -> SourcePoll {
        let event = self.map_event(event);
        self.advance_cursor();
        event.map_or(SourcePoll::Advanced, |event| {
            SourcePoll::Event(Box::new(event))
        })
    }

    const fn advance_cursor(&mut self) {
        if !self.running.is_empty() {
            self.cursor = (self.cursor + 1) % self.running.len();
        }
    }

    const fn normalize_cursor(&mut self) {
        if self.cursor >= self.running.len() {
            self.cursor = 0;
        }
    }

    fn remove_current_source(&mut self) {
        self.running.remove(self.cursor);
        self.normalize_cursor();
        self.start_ready_sources();
    }

    fn is_complete(&self) -> bool {
        self.initial.is_empty() && self.running.is_empty() && self.pending.is_empty()
    }
}

impl Drop for SearchDriver {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

enum WorkerPoll {
    Progress,
    Pending,
}

enum SourcePoll {
    Event(Box<EngineEvent>),
    Advanced,
    Pending,
}

struct RunningSource {
    rx: mpsc::Receiver<WorkerEvent>,
    future: Option<BoxFuture<'static, ()>>,
}

impl RunningSource {
    fn poll_future(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(future) = self.future.as_mut() else {
            return Poll::Pending;
        };
        match future.as_mut().poll(cx) {
            Poll::Ready(()) => {
                self.future = None;
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_closed(&self) -> bool {
        self.future.is_none()
    }
}

impl Stream for SearchDriver {
    type Item = EngineEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Poll::Ready(Some(event)) = self.poll_available_event(cx) {
                return Poll::Ready(Some(event));
            }
            self.start_ready_sources();
            let worker_poll = self.poll_workers(cx);
            if let Poll::Ready(Some(event)) = self.poll_available_event(cx) {
                return Poll::Ready(Some(event));
            }
            if matches!(worker_poll, WorkerPoll::Progress) {
                continue;
            }
            if self.is_complete() {
                return Poll::Ready(None);
            }
            return Poll::Pending;
        }
    }
}
