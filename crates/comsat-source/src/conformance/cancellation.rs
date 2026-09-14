use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use comsat_types::{Query, Record, SourceError, Target};
use futures::Stream;

use crate::{RecordStream, SourceRunContext, SourceRuntime};

/// Counts source requests handed out and still held, so the conformance suite
/// can prove a cancelled tool call actually drops the pending source request
/// instead of only reporting a cancelled outcome.
#[derive(Debug, Default)]
pub(super) struct RequestLedger {
    opened: AtomicUsize,
    live: AtomicUsize,
}

impl RequestLedger {
    pub(super) fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }

    pub(super) fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    fn open(self: &Arc<Self>) -> RequestGuard {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        RequestGuard {
            ledger: Arc::clone(self),
        }
    }
}

/// Wraps a runtime so every source request is tracked by `ledger`.
pub(super) fn observe_runtime(
    inner: Arc<dyn SourceRuntime>,
    ledger: Arc<RequestLedger>,
) -> Arc<dyn SourceRuntime> {
    Arc::new(ObservedRuntime { inner, ledger })
}

struct ObservedRuntime {
    inner: Arc<dyn SourceRuntime>,
    ledger: Arc<RequestLedger>,
}

#[async_trait]
impl SourceRuntime for ObservedRuntime {
    async fn search(&self, query: Query, context: SourceRunContext) -> RecordStream {
        let guard = self.ledger.open();
        let inner = self.inner.search(query, context).await;
        Box::pin(ObservedStream {
            inner,
            _guard: guard,
        })
    }

    async fn fetch(
        &self,
        target: Target,
        context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        let _guard = self.ledger.open();
        self.inner.fetch(target, context).await
    }

    async fn follow(&self, target: Target, context: SourceRunContext) -> RecordStream {
        let guard = self.ledger.open();
        let inner = self.inner.follow(target, context).await;
        Box::pin(ObservedStream {
            inner,
            _guard: guard,
        })
    }
}

/// Released when the request future or its record stream is dropped.
struct RequestGuard {
    ledger: Arc<RequestLedger>,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.ledger.live.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ObservedStream {
    inner: RecordStream,
    _guard: RequestGuard,
}

impl Stream for ObservedStream {
    type Item = Result<Record, SourceError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(context)
    }
}
