use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use comsat_source::OperationKind;
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt};
use incurs::tool::{ToolCallControl, ToolCallOptions, ToolCallOutcome, ToolEvent, ToolEventSink};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::catalog::{CatalogSource, SourceCatalog};
use crate::merge::{EngineDiagnostic, EngineError, EngineLimits};
use crate::metrics::EngineMetrics;

#[derive(Debug)]
pub enum WorkerEvent {
    Record { source: SourceId, record: Record },
    Failed(EngineDiagnostic),
}

#[derive(Clone)]
pub enum OperationInput {
    Query(Query),
    Target(Target),
}

impl OperationInput {
    pub(crate) fn arguments(&self) -> BTreeMap<String, Value> {
        match self {
            Self::Query(query) => query_arguments(query),
            Self::Target(target) => target_arguments(target),
        }
    }
}

pub struct SourceInvocation {
    pub source: CatalogSource,
    pub operation: OperationKind,
    pub input: OperationInput,
    pub limits: EngineLimits,
    pub metrics: EngineMetrics,
}

pub async fn run_source_operation(
    invocation: SourceInvocation,
    tx: mpsc::Sender<WorkerEvent>,
    parent_token: CancellationToken,
) {
    let SourceInvocation {
        source,
        operation,
        input,
        limits,
        metrics,
    } = invocation;
    let Some(tool) = source
        .descriptor
        .tool_name(operation)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    let guard = metrics.start(source.descriptor.id.clone(), operation);
    let token = parent_token.child_token();
    let sink = BudgetedSink::new(
        source.descriptor.id.clone(),
        limits,
        tx.clone(),
        token.clone(),
    );
    let options = stream_options(token, &sink);
    let outcome = source.catalog.call(&tool, input.arguments(), options).await;
    finish_stream_outcome(StreamFinish {
        source: source.descriptor.id,
        outcome,
        sink,
        guard,
        tx,
        parent_token,
    })
    .await;
}

struct StreamFinish {
    source: SourceId,
    outcome: ToolCallOutcome,
    sink: BudgetedSink,
    guard: crate::metrics::OperationGuard,
    tx: mpsc::Sender<WorkerEvent>,
    parent_token: CancellationToken,
}

async fn finish_stream_outcome(finish: StreamFinish) {
    let StreamFinish {
        source,
        outcome,
        sink,
        guard,
        tx,
        parent_token,
    } = finish;
    if let Some(error) = sink.budget_error() {
        guard.failure(error.class);
        send_event(tx, WorkerEvent::Failed(error.into()), &parent_token).await;
        return;
    }
    match outcome {
        ToolCallOutcome::Ok { data, .. } if !sink.has_records() => {
            sink.emit_final_data(data).await;
            if let Some(error) = sink.budget_error() {
                guard.failure(error.class);
                send_event(tx, WorkerEvent::Failed(error.into()), &parent_token).await;
            } else {
                guard.success();
            }
        }
        ToolCallOutcome::Error { code, message, .. } => {
            let class = error_class(&code);
            record_failure(guard, class);
            let error = SourceError::new(source, class, message);
            send_event(tx, WorkerEvent::Failed(error.into()), &parent_token).await;
        }
        ToolCallOutcome::Ok { .. } => {
            guard.success();
        }
    }
}

pub async fn fetch(
    catalog: &SourceCatalog,
    target: Target,
    limits: EngineLimits,
    metrics: EngineMetrics,
) -> Result<Record, EngineError> {
    let source_id = target.source().clone();
    let source = catalog
        .source(&source_id)
        .ok_or_else(|| EngineError::SourceNotFound(source_id.clone()))?;
    let guard = metrics.start(source_id.clone(), OperationKind::Fetch);
    let tool = source.descriptor.tool_name(OperationKind::Fetch);
    let Some(tool) = tool else {
        guard.failure(ErrorClass::Unsupported);
        return Err(EngineError::Unsupported {
            source_id: source_id.clone(),
            operation: OperationKind::Fetch.as_str(),
        });
    };
    let cancellation = CancellationToken::new();
    let drop_guard = cancellation.clone().drop_guard();
    let options = fetch_options(cancellation);
    let outcome = source
        .catalog
        .call(tool, target_arguments(&target), options)
        .await;
    let _cancellation = drop_guard.disarm();
    finish_fetch_outcome(source_id, outcome, &limits, guard)
}

fn finish_fetch_outcome(
    source_id: SourceId,
    outcome: ToolCallOutcome,
    limits: &EngineLimits,
    guard: crate::metrics::OperationGuard,
) -> Result<Record, EngineError> {
    match outcome {
        ToolCallOutcome::Ok { data, .. } => finish_fetch_record(source_id, data, limits, guard),
        ToolCallOutcome::Error { code, message, .. } => {
            let class = error_class(&code);
            record_failure(guard, class);
            Err(SourceError::new(source_id, class, message).into())
        }
    }
}

fn finish_fetch_record(
    source_id: SourceId,
    data: Value,
    limits: &EngineLimits,
    guard: crate::metrics::OperationGuard,
) -> Result<Record, EngineError> {
    match deserialize_record(source_id, data, limits) {
        Ok(record) => {
            guard.success();
            Ok(record)
        }
        Err(error) => {
            guard.failure(ErrorClass::Protocol);
            Err(error)
        }
    }
}

fn stream_options(token: CancellationToken, sink: &BudgetedSink) -> ToolCallOptions {
    ToolCallOptions {
        control: ToolCallControl {
            cancellation: token,
            events: Some(Arc::new(sink.clone())),
        },
        ..ToolCallOptions::isolated()
    }
}

fn fetch_options(cancellation: CancellationToken) -> ToolCallOptions {
    ToolCallOptions {
        control: ToolCallControl {
            cancellation,
            events: None,
        },
        ..ToolCallOptions::isolated()
    }
}

fn record_failure(guard: crate::metrics::OperationGuard, class: ErrorClass) {
    if class == ErrorClass::Cancelled {
        guard.cancelled();
    } else {
        guard.failure(class);
    }
}

#[derive(Clone)]
struct BudgetedSink {
    source: SourceId,
    state: Arc<Mutex<SinkState>>,
    tx: mpsc::Sender<WorkerEvent>,
    token: CancellationToken,
    limits: EngineLimits,
}

#[derive(Debug, Default)]
struct SinkState {
    records: usize,
    bytes: usize,
    budget_error: Option<SourceError>,
}

impl BudgetedSink {
    fn new(
        source: SourceId,
        limits: EngineLimits,
        tx: mpsc::Sender<WorkerEvent>,
        token: CancellationToken,
    ) -> Self {
        Self {
            source,
            state: Arc::new(Mutex::new(SinkState::default())),
            tx,
            token,
            limits,
        }
    }

    fn budget_error(&self) -> Option<SourceError> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .budget_error
            .clone()
    }

    fn has_records(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .records
            > 0
    }
}

#[async_trait]
impl ToolEventSink for BudgetedSink {
    async fn emit(&self, event: ToolEvent) {
        let ToolEvent::Chunk { data } = event else {
            return;
        };
        self.emit_record_value(data).await;
    }
}

async fn send_event(
    mut tx: mpsc::Sender<WorkerEvent>,
    event: WorkerEvent,
    token: &CancellationToken,
) {
    let send = tx.send(event).fuse();
    futures::pin_mut!(send);
    futures::select! {
        result = send => {
            if result.is_err() {
                token.cancel();
            }
        }
        () = token.cancelled().fuse() => {}
    }
}

impl BudgetedSink {
    async fn emit_final_data(&self, data: Value) {
        match data {
            Value::Array(records) => {
                for record in records {
                    self.emit_record_value(record).await;
                    if self.budget_error().is_some() {
                        return;
                    }
                }
            }
            value => self.emit_record_value(value).await,
        }
    }

    async fn emit_record_value(&self, data: Value) {
        if self.apply_budget(&data) {
            return;
        }
        match serde_json::from_value::<Record>(data) {
            Ok(record) if record.validate().is_ok() => {
                send_event(
                    self.tx.clone(),
                    WorkerEvent::Record {
                        source: self.source.clone(),
                        record,
                    },
                    &self.token,
                )
                .await;
            }
            Ok(_) | Err(_) => self.fail("source emitted invalid record data"),
        }
    }

    fn apply_budget(&self, data: &Value) -> bool {
        let bytes = serde_json::to_vec(data).map_or(usize::MAX, |value| value.len());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.records = state.records.saturating_add(1);
        state.bytes = state.bytes.saturating_add(bytes);
        if state.records > self.limits.max_records_per_source {
            state.budget_error = Some(budget_error(
                self.source.clone(),
                "source record budget exceeded",
            ));
        }
        if state.bytes > self.limits.max_bytes_per_source {
            state.budget_error = Some(budget_error(
                self.source.clone(),
                "source byte budget exceeded",
            ));
        }
        let exceeded = state.budget_error.is_some();
        drop(state);
        if exceeded {
            self.token.cancel();
        }
        exceeded
    }

    fn fail(&self, message: &'static str) {
        self.token.cancel();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.budget_error = Some(SourceError::new(
            self.source.clone(),
            ErrorClass::Protocol,
            message,
        ));
    }
}

fn query_arguments(query: &Query) -> BTreeMap<String, Value> {
    let value = serde_json::to_value(query).expect("query must serialize");
    object_arguments(&value)
}

pub fn target_arguments(target: &Target) -> BTreeMap<String, Value> {
    match target {
        Target::Record { record } => object_arguments(&json!({
            "type": "record",
            "record": record,
        })),
        Target::Url { source, url } => object_arguments(&json!({
            "type": "url",
            "source": source,
            "url": url,
        })),
        Target::Native { source, id } => object_arguments(&json!({
            "type": "native",
            "source": source,
            "id": id,
        })),
    }
}

fn object_arguments(value: &Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn deserialize_record(
    source: SourceId,
    data: Value,
    limits: &EngineLimits,
) -> Result<Record, EngineError> {
    let bytes = serde_json::to_vec(&data).map_or(usize::MAX, |value| value.len());
    if bytes > limits.max_bytes_per_source {
        // The source answered correctly; this runtime will not carry a record
        // that large. Saying `protocol` would blame the upstream for a limit
        // this deployment chose.
        return Err(SourceError::new(
            source,
            ErrorClass::Unsupported,
            "fetch byte budget exceeded",
        )
        .into());
    }
    let record: Record = serde_json::from_value(data).map_err(|error| {
        SourceError::new(source.clone(), ErrorClass::Protocol, error.to_string())
    })?;
    record
        .validate()
        .map_err(|error| SourceError::new(source, ErrorClass::Protocol, error.to_string()))?;
    Ok(record)
}

fn budget_error(source: SourceId, message: &'static str) -> SourceError {
    SourceError::new(source, ErrorClass::Unsupported, message)
}

fn error_class(code: &str) -> ErrorClass {
    let normalized = code.to_ascii_lowercase();
    if normalized == "validation_error" {
        return ErrorClass::InvalidQuery;
    }
    serde_json::from_value(Value::String(normalized)).unwrap_or(ErrorClass::Upstream)
}
