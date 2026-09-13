use std::collections::{BTreeMap, BTreeSet, VecDeque};

use comsat_source::{OperationKind, SourceDescriptor};
use comsat_types::{ErrorClass, SourceId, Target};

use crate::catalog::CatalogSource;
use crate::invoke::OperationInput;
use crate::merge::{EngineDiagnostic, EngineError, SearchRequest};

#[derive(Clone)]
pub enum StreamPlan {
    Search(SearchRequest),
    Follow(Target),
}

#[derive(Clone)]
pub struct PlannedSource {
    pub source: CatalogSource,
    pub operation: OperationKind,
    pub input: OperationInput,
}

pub fn plan_sources(
    sources: &[CatalogSource],
    plan: StreamPlan,
) -> (VecDeque<PlannedSource>, Vec<EngineDiagnostic>, bool) {
    match plan {
        StreamPlan::Search(request) => plan_search(sources, &request),
        StreamPlan::Follow(target) => plan_follow(sources, target),
    }
}

fn plan_search(
    sources: &[CatalogSource],
    request: &SearchRequest,
) -> (VecDeque<PlannedSource>, Vec<EngineDiagnostic>, bool) {
    let by_id = sources_by_id(sources);
    let diagnostics = missing_source_diagnostics(&by_id, &request.sources);
    let pending = selected_source_ids(&by_id, &request.sources)
        .into_iter()
        .filter_map(|id| by_id.get(id.as_str()).cloned())
        .filter(|source| supports(&source.descriptor, OperationKind::Search))
        .map(|source| planned_search(source, request))
        .collect();
    (pending, diagnostics, request.strict)
}

fn plan_follow(
    sources: &[CatalogSource],
    target: Target,
) -> (VecDeque<PlannedSource>, Vec<EngineDiagnostic>, bool) {
    let source_id = target.source().clone();
    let by_id = sources_by_id(sources);
    let Some(source) = by_id.get(source_id.as_str()).cloned() else {
        return (VecDeque::new(), vec![source_not_found(source_id)], false);
    };
    if !supports(&source.descriptor, OperationKind::Follow) {
        return unsupported_follow(source_id);
    }
    (
        VecDeque::from([planned_follow(source, target)]),
        Vec::new(),
        false,
    )
}

fn planned_search(source: CatalogSource, request: &SearchRequest) -> PlannedSource {
    PlannedSource {
        source,
        operation: OperationKind::Search,
        input: OperationInput::Query(request.query.clone()),
    }
}

const fn planned_follow(source: CatalogSource, target: Target) -> PlannedSource {
    PlannedSource {
        source,
        operation: OperationKind::Follow,
        input: OperationInput::Target(target),
    }
}

fn unsupported_follow(
    source_id: SourceId,
) -> (VecDeque<PlannedSource>, Vec<EngineDiagnostic>, bool) {
    (
        VecDeque::new(),
        vec![unsupported(source_id, OperationKind::Follow)],
        false,
    )
}

fn sources_by_id(sources: &[CatalogSource]) -> BTreeMap<String, CatalogSource> {
    sources
        .iter()
        .map(|source| (source.descriptor.id.as_str().to_owned(), source.clone()))
        .collect()
}

fn selected_source_ids(
    by_id: &BTreeMap<String, CatalogSource>,
    selected: &[SourceId],
) -> Vec<SourceId> {
    let mut ids = if selected.is_empty() {
        by_id
            .values()
            .map(|source| source.descriptor.id.clone())
            .collect()
    } else {
        selected.to_vec()
    };
    ids.sort();
    ids.dedup();
    ids
}

fn missing_source_diagnostics(
    by_id: &BTreeMap<String, CatalogSource>,
    selected: &[SourceId],
) -> Vec<EngineDiagnostic> {
    let mut missing = selected
        .iter()
        .filter(|source| !by_id.contains_key(source.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing.into_iter().map(source_not_found).collect()
}

pub fn validate_unique_sources(sources: &[CatalogSource]) -> Result<(), EngineError> {
    let mut seen = BTreeSet::new();
    for source in sources {
        if !seen.insert(source.descriptor.id.clone()) {
            return Err(EngineError::DuplicateSourceId(source.descriptor.id.clone()));
        }
    }
    Ok(())
}

const fn supports(descriptor: &SourceDescriptor, operation: OperationKind) -> bool {
    descriptor.supports(operation)
}

fn source_not_found(source: SourceId) -> EngineDiagnostic {
    EngineDiagnostic {
        source,
        class: ErrorClass::NotFound,
        message: "source is not registered".to_string(),
        retry_after_seconds: None,
    }
}

fn unsupported(source: SourceId, operation: OperationKind) -> EngineDiagnostic {
    EngineDiagnostic {
        source,
        class: ErrorClass::Unsupported,
        message: format!("source does not support {}", operation.as_str()),
        retry_after_seconds: None,
    }
}
