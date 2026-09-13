use comsat_engine::{EngineDiagnostic, EngineError};
use comsat_types::{ErrorClass, Record, SourceError};
use incurs::output::{CommandResult, StreamRecord};
use serde_json::{Value, json};

pub(super) fn source_error_value(diagnostic: &EngineDiagnostic) -> Value {
    json!({
        "type": "source_error",
        "diagnostic": diagnostic_value(diagnostic)
    })
}

pub(super) fn source_failures_stream_error(
    diagnostics: &[EngineDiagnostic],
    strict: bool,
) -> StreamRecord {
    let retryable = diagnostics
        .iter()
        .any(|diagnostic| retryable(diagnostic.class));
    stream_error(
        source_failures_code(diagnostics, strict),
        source_failures_message(diagnostics, strict),
        retryable,
        if strict { 3 } else { 1 },
    )
}

const fn source_failures_code(diagnostics: &[EngineDiagnostic], strict: bool) -> &'static str {
    if strict {
        return "partial_failure";
    }
    match diagnostics {
        [diagnostic] => error_code(diagnostic.class),
        _ => "source_failure",
    }
}

fn source_failures_message(diagnostics: &[EngineDiagnostic], strict: bool) -> String {
    let message = if strict {
        "one or more sources failed"
    } else {
        "all selected sources failed"
    };
    let details = diagnostics.iter().map(diagnostic_value).collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "message": message,
        "diagnostics": details,
    }))
    .unwrap_or_else(|_| message.to_string())
}

fn diagnostic_value(diagnostic: &EngineDiagnostic) -> Value {
    json!({
        "source": diagnostic.source,
        "class": diagnostic.class,
        "message": diagnostic.message,
        "retry_after_seconds": diagnostic.retry_after_seconds,
    })
}

pub(super) fn record_result(record: Record) -> CommandResult {
    match record_value(record) {
        Ok(data) => CommandResult::Ok {
            data,
            cta: None,
            exit_code: None,
        },
        Err(message) => command_error("protocol", message, false, 1),
    }
}

pub(super) fn record_value(record: Record) -> Result<Value, String> {
    record
        .validate()
        .map_err(|error| error.to_string())
        .and_then(|()| serde_json::to_value(record).map_err(|error| error.to_string()))
}

pub(super) fn command_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
    exit_code: i32,
) -> CommandResult {
    CommandResult::Error {
        code: code.into(),
        message: message.into(),
        retryable,
        exit_code: Some(exit_code),
        cta: None,
    }
}

pub(super) fn engine_error_result(error: EngineError) -> CommandResult {
    match error {
        EngineError::Source(error) => source_error_result(&error),
        EngineError::SourceNotFound(source) => {
            command_error("not_found", format!("source not found: {source}"), false, 1)
        }
        EngineError::Unsupported {
            source_id,
            operation,
        } => command_error(
            "unsupported",
            format!("source does not support {operation}: {source_id}"),
            false,
            1,
        ),
        EngineError::DuplicateSourceId(source) => command_error(
            "invalid_query",
            format!("duplicate source id: {source}"),
            false,
            2,
        ),
        EngineError::StrictSearchFailed(diagnostics) => {
            let retryable = diagnostics
                .iter()
                .any(|diagnostic| retryable(diagnostic.class));
            command_error(
                "partial_failure",
                "one or more sources failed",
                retryable,
                3,
            )
        }
    }
}

fn source_error_result(error: &SourceError) -> CommandResult {
    command_error(
        error_code(error.class),
        format!("{}: {}", error.source, error.message),
        retryable(error.class),
        1,
    )
}

pub(super) fn engine_error_record(error: EngineError) -> StreamRecord {
    match error {
        EngineError::Source(error) => StreamRecord::Error {
            code: error_code(error.class).to_string(),
            message: format!("{}: {}", error.source, error.message),
            retryable: retryable(error.class),
            exit_code: Some(1),
            cta: None,
        },
        other => StreamRecord::Error {
            code: engine_error_code(&other).to_string(),
            message: other.to_string(),
            retryable: false,
            exit_code: Some(engine_error_exit_code(&other)),
            cta: None,
        },
    }
}

const fn engine_error_code(error: &EngineError) -> &'static str {
    match error {
        EngineError::Source(error) => error_code(error.class),
        EngineError::SourceNotFound(_) => "not_found",
        EngineError::DuplicateSourceId(_) => "invalid_query",
        EngineError::Unsupported { .. } => "unsupported",
        EngineError::StrictSearchFailed(_) => "partial_failure",
    }
}

const fn engine_error_exit_code(error: &EngineError) -> i32 {
    match error {
        EngineError::DuplicateSourceId(_) => 2,
        EngineError::StrictSearchFailed(_) => 3,
        _ => 1,
    }
}

pub(super) fn stream_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
    exit_code: i32,
) -> StreamRecord {
    StreamRecord::Error {
        code: code.into(),
        message: message.into(),
        retryable,
        exit_code: Some(exit_code),
        cta: None,
    }
}

pub(super) const fn retryable(class: ErrorClass) -> bool {
    matches!(class, ErrorClass::RateLimit | ErrorClass::Timeout)
}

const fn error_code(class: ErrorClass) -> &'static str {
    if let Some(code) = access_error_code(class) {
        return code;
    }
    if let Some(code) = query_error_code(class) {
        return code;
    }
    if let Some(code) = upstream_error_code(class) {
        return code;
    }
    "internal"
}

const fn access_error_code(class: ErrorClass) -> Option<&'static str> {
    match class {
        ErrorClass::Authentication => Some("authentication"),
        ErrorClass::Authorization => Some("authorization"),
        _ => None,
    }
}

const fn query_error_code(class: ErrorClass) -> Option<&'static str> {
    match class {
        ErrorClass::InvalidQuery => Some("invalid_query"),
        ErrorClass::NotFound => Some("not_found"),
        ErrorClass::Unsupported => Some("unsupported"),
        _ => None,
    }
}

const fn upstream_error_code(class: ErrorClass) -> Option<&'static str> {
    match class {
        ErrorClass::RateLimit => Some("rate_limit"),
        ErrorClass::Upstream => Some("upstream"),
        ErrorClass::Timeout => Some("timeout"),
        ErrorClass::Cancelled => Some("cancelled"),
        ErrorClass::Protocol => Some("protocol"),
        _ => None,
    }
}
