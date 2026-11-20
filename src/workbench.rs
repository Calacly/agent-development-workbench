use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

pub const EVENT_SCHEMA_VERSION: &str = "1.0";
const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Observed,
    Synthetic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Harness,
    Model,
    Mcp,
    Retrieval,
    Memory,
    Application,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    RunStarted,
    StepStarted,
    ModelRequest,
    ModelResponse,
    McpDiscovery,
    McpToolCall,
    McpToolResult,
    RetrievalQuery,
    RetrievalCandidates,
    RerankCompleted,
    MemoryUpdated,
    ApplicationSideEffect,
    StepCompleted,
    RunCompleted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceEvent {
    pub schema_version: String,
    pub event_id: String,
    pub operation_id: String,
    pub run_id: String,
    pub step_id: Option<String>,
    pub attempt_id: Option<String>,
    pub parent_operation_id: Option<String>,
    pub component: Component,
    pub kind: EventKind,
    pub source: EventSource,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub status: EventStatus,
    pub payload: Value,
    pub redacted_fields: Vec<String>,
}

impl TraceEvent {
    fn synthetic(
        event_id: &str,
        run_id: &str,
        step_id: Option<&str>,
        attempt_id: Option<&str>,
        parent_operation_id: Option<String>,
        component: Component,
        kind: EventKind,
        timestamp: DateTime<Utc>,
        duration_ms: Option<u64>,
        status: EventStatus,
        payload: Value,
    ) -> Self {
        let (payload, mut redacted_fields) = sanitize_metadata(payload);
        redacted_fields.sort();
        redacted_fields.dedup();

        Self {
            schema_version: EVENT_SCHEMA_VERSION.to_string(),
            event_id: event_id.to_string(),
            operation_id: format!("op-{event_id}"),
            run_id: run_id.to_string(),
            step_id: step_id.map(str::to_string),
            attempt_id: attempt_id.map(str::to_string),
            parent_operation_id,
            component,
            kind,
            source: EventSource::Synthetic,
            timestamp,
            duration_ms,
            status,
            payload,
            redacted_fields,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trace {
    pub trace_id: String,
    pub name: String,
    pub description: String,
    pub run_id: String,
    pub events: Vec<TraceEvent>,
}

impl Trace {
    pub fn validate(&self) -> Result<(), TraceValidationError> {
        if self.trace_id.trim().is_empty() {
            return Err(TraceValidationError::EmptyTraceField("trace_id"));
        }
        if self.run_id.trim().is_empty() {
            return Err(TraceValidationError::EmptyTraceField("run_id"));
        }
        if self.events.is_empty() {
            return Err(TraceValidationError::EmptyTrace);
        }
        if self.events.first().map(|event| event.kind) != Some(EventKind::RunStarted)
            || self.events.last().map(|event| event.kind) != Some(EventKind::RunCompleted)
        {
            return Err(TraceValidationError::InvalidBoundary);
        }

        let mut event_ids = HashSet::new();
        let mut operations = BTreeMap::new();
        let mut previous_timestamp = None;

        for event in &self.events {
            if event.schema_version != EVENT_SCHEMA_VERSION {
                return Err(TraceValidationError::UnsupportedSchema {
                    event_id: event.event_id.clone(),
                    version: event.schema_version.clone(),
                });
            }
            if event.event_id.trim().is_empty() {
                return Err(TraceValidationError::EmptyEventField {
                    event_id: event.event_id.clone(),
                    field: "event_id",
                });
            }
            if event.operation_id.trim().is_empty() {
                return Err(TraceValidationError::EmptyEventField {
                    event_id: event.event_id.clone(),
                    field: "operation_id",
                });
            }
            if event.run_id != self.run_id {
                return Err(TraceValidationError::RunMismatch(event.event_id.clone()));
            }
            if !event_ids.insert(event.event_id.clone()) {
                return Err(TraceValidationError::DuplicateEventId(
                    event.event_id.clone(),
                ));
            }
            if operations.contains_key(&event.operation_id) {
                return Err(TraceValidationError::DuplicateOperationId(
                    event.operation_id.clone(),
                ));
            }
            if previous_timestamp.is_some_and(|timestamp| event.timestamp < timestamp) {
                return Err(TraceValidationError::OutOfOrder(event.event_id.clone()));
            }

            let run_level = matches!(event.kind, EventKind::RunStarted | EventKind::RunCompleted);
            if !run_level && event.step_id.as_deref().is_none_or(str::is_empty) {
                return Err(TraceValidationError::EmptyEventField {
                    event_id: event.event_id.clone(),
                    field: "step_id",
                });
            }
            if !run_level && event.attempt_id.as_deref().is_none_or(str::is_empty) {
                return Err(TraceValidationError::EmptyEventField {
                    event_id: event.event_id.clone(),
                    field: "attempt_id",
                });
            }

            match &event.parent_operation_id {
                Some(parent) if !operations.contains_key(parent) => {
                    return Err(TraceValidationError::UnknownOrLateParent {
                        event_id: event.event_id.clone(),
                        parent_operation_id: parent.clone(),
                    });
                }
                None if event.kind != EventKind::RunStarted => {
                    return Err(TraceValidationError::MissingParent(event.event_id.clone()));
                }
                _ => {}
            }

            if event.status == EventStatus::Failed
                && (!has_nonempty_string(&event.payload, "error_code")
                    || !has_nonempty_string(&event.payload, "error_summary"))
            {
                return Err(TraceValidationError::MissingFailureMetadata(
                    event.event_id.clone(),
                ));
            }

            if let Some(path) = unsafe_payload_path(&event.payload, "$") {
                return Err(TraceValidationError::UnsafePayload {
                    event_id: event.event_id.clone(),
                    path,
                });
            }

            operations.insert(event.operation_id.clone(), event.timestamp);
            previous_timestamp = Some(event.timestamp);
        }

        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TraceValidationError {
    #[error("trace field {0} must not be empty")]
    EmptyTraceField(&'static str),
    #[error("trace must contain events")]
    EmptyTrace,
    #[error("trace must start with run_started and end with run_completed")]
    InvalidBoundary,
    #[error("event {event_id} uses unsupported schema version {version}")]
    UnsupportedSchema { event_id: String, version: String },
    #[error("event {event_id} field {field} must not be empty")]
    EmptyEventField {
        event_id: String,
        field: &'static str,
    },
    #[error("event {0} belongs to a different run")]
    RunMismatch(String),
    #[error("duplicate event id {0}")]
    DuplicateEventId(String),
    #[error("duplicate operation id {0}")]
    DuplicateOperationId(String),
    #[error("event {0} is not in timestamp order")]
    OutOfOrder(String),
    #[error("event {event_id} references missing or later parent {parent_operation_id}")]
    UnknownOrLateParent {
        event_id: String,
        parent_operation_id: String,
    },
    #[error("event {0} is missing a parent operation")]
    MissingParent(String),
    #[error("failed event {0} needs error_code and error_summary metadata")]
    MissingFailureMetadata(String),
    #[error("event {event_id} contains unsafe payload data at {path}")]
    UnsafePayload { event_id: String, path: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TimelineRow {
    pub event_id: String,
    pub operation_id: String,
    pub parent_operation_id: Option<String>,
    pub component: Component,
    pub kind: EventKind,
    pub source: EventSource,
    pub status: EventStatus,
    pub offset_ms: u64,
    pub duration_ms: u64,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LearnExplanation {
    pub grounded_in_event_id: String,
    pub explanation_type: &'static str,
    pub summary: String,
    pub significance: String,
    pub evidence_note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkbenchFixture {
    pub id: &'static str,
    pub label: &'static str,
    pub expected_outcome: &'static str,
    pub trace: Trace,
    pub timeline: Vec<TimelineRow>,
    pub explanations: Vec<LearnExplanation>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkbenchData {
    pub event_schema_version: &'static str,
    pub data_source: &'static str,
    pub fixtures: Vec<WorkbenchFixture>,
}

pub fn workbench_data() -> Result<WorkbenchData, TraceValidationError> {
    let fixtures = vec![
        fixture_view(
            "success",
            "Successful grounded answer",
            "The run presents an answer with two selected citations.",
            successful_trace(),
        )?,
        fixture_view(
            "retrieval-failure",
            "Retrieval timeout",
            "The failure remains correlated from retrieval through the stopped run.",
            failing_trace(),
        )?,
    ];

    Ok(WorkbenchData {
        event_schema_version: EVENT_SCHEMA_VERSION,
        data_source: "explicitly synthetic fixtures",
        fixtures,
    })
}

pub fn timeline_rows(trace: &Trace) -> Result<Vec<TimelineRow>, TraceValidationError> {
    trace.validate()?;

    let started_at = trace.events[0].timestamp;
    let mut depths = BTreeMap::new();
    let mut rows = Vec::with_capacity(trace.events.len());

    for event in &trace.events {
        let depth = event
            .parent_operation_id
            .as_ref()
            .and_then(|parent| depths.get(parent))
            .map_or(0, |parent_depth| parent_depth + 1);
        depths.insert(event.operation_id.clone(), depth);

        rows.push(TimelineRow {
            event_id: event.event_id.clone(),
            operation_id: event.operation_id.clone(),
            parent_operation_id: event.parent_operation_id.clone(),
            component: event.component,
            kind: event.kind,
            source: event.source,
            status: event.status,
            offset_ms: (event.timestamp - started_at).num_milliseconds().max(0) as u64,
            duration_ms: event.duration_ms.unwrap_or(1).max(1),
            depth,
        });
    }

    Ok(rows)
}

fn fixture_view(
    id: &'static str,
    label: &'static str,
    expected_outcome: &'static str,
    trace: Trace,
) -> Result<WorkbenchFixture, TraceValidationError> {
    let timeline = timeline_rows(&trace)?;
    let explanations = trace.events.iter().map(explain_event).collect();

    Ok(WorkbenchFixture {
        id,
        label,
        expected_outcome,
        trace,
        timeline,
        explanations,
    })
}

fn explain_event(event: &TraceEvent) -> LearnExplanation {
    let (summary, significance) = match event.kind {
        EventKind::RunStarted => (
            "The harness opened a correlated run.",
            "Every later operation can be connected to one top-level execution.",
        ),
        EventKind::StepStarted => (
            "The harness entered a bounded step.",
            "Step and attempt identifiers make retries and routing decisions distinguishable.",
        ),
        EventKind::ModelRequest => (
            "The harness invoked the model with sanitized metadata.",
            "The event records the call boundary without storing the private prompt or messages.",
        ),
        EventKind::ModelResponse => (
            "The model call returned a structured outcome.",
            "This shows observable output metadata, not hidden chain-of-thought.",
        ),
        EventKind::McpDiscovery => (
            "The MCP client discovered available capabilities.",
            "Discovery and execution are separate protocol responsibilities.",
        ),
        EventKind::McpToolCall => (
            "The harness invoked an MCP tool.",
            "Only argument names and counts are retained; values are removed before persistence.",
        ),
        EventKind::McpToolResult => (
            "The MCP client returned a tool outcome.",
            "The result remains linked to the request without persisting raw private content.",
        ),
        EventKind::RetrievalQuery => (
            "The retrieval adapter issued a vector query.",
            "Retrieval is an application operation around the model, not hidden model behavior.",
        ),
        EventKind::RetrievalCandidates => (
            "The retriever returned candidate metadata.",
            "Counts, score ranges, and errors make retrieval quality diagnosable without documents.",
        ),
        EventKind::RerankCompleted => (
            "The reranker selected context candidates.",
            "The selection boundary explains which retrieval stage changed the candidate set.",
        ),
        EventKind::MemoryUpdated => (
            "The harness changed memory state.",
            "Explicit memory events keep state mutation separate from model calls.",
        ),
        EventKind::ApplicationSideEffect => (
            "The application recorded an externally visible outcome.",
            "Side effects stay distinct from durable harness progress and can be marked skipped.",
        ),
        EventKind::StepCompleted => (
            "The harness closed the current step.",
            "The final step status connects subsystem failures to orchestration behavior.",
        ),
        EventKind::RunCompleted => (
            "The harness terminated the run.",
            "The terminal status shows whether the workflow succeeded or stopped after a failure.",
        ),
    };

    LearnExplanation {
        grounded_in_event_id: event.event_id.clone(),
        explanation_type: "derived",
        summary: summary.to_string(),
        significance: significance.to_string(),
        evidence_note: format!(
            "Derived from synthetic event {} ({:?}/{:?}); not hidden model reasoning.",
            event.event_id, event.component, event.kind
        ),
    }
}

fn has_nonempty_string(payload: &Value, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn sanitize_metadata(value: Value) -> (Value, Vec<String>) {
    let mut redacted_fields = Vec::new();
    let value = sanitize_value(value, "$", &mut redacted_fields);
    (value, redacted_fields)
}

fn sanitize_value(value: Value, path: &str, redacted_fields: &mut Vec<String>) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let child_path = format!("{path}.{key}");
                    if is_sensitive_key(&key) {
                        redacted_fields.push(child_path);
                        (key, Value::String(REDACTED.to_string()))
                    } else {
                        (key, sanitize_value(value, &child_path, redacted_fields))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    sanitize_value(value, &format!("{path}[{index}]"), redacted_fields)
                })
                .collect(),
        ),
        Value::String(value) if looks_like_secret(&value) => {
            redacted_fields.push(path.to_string());
            Value::String(REDACTED.to_string())
        }
        value => value,
    }
}

fn unsafe_payload_path(value: &Value, path: &str) -> Option<String> {
    match value {
        Value::Object(values) => values.iter().find_map(|(key, value)| {
            let child_path = format!("{path}.{key}");
            if is_sensitive_key(key) && value.as_str() != Some(REDACTED) {
                Some(child_path)
            } else {
                unsafe_payload_path(value, &child_path)
            }
        }),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .find_map(|(index, value)| unsafe_payload_path(value, &format!("{path}[{index}]"))),
        Value::String(value) if value != REDACTED && looks_like_secret(value) => {
            Some(path.to_string())
        }
        _ => None,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "apikey"
            | "authorization"
            | "bearertoken"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "password"
            | "secret"
            | "cookie"
            | "prompt"
            | "messages"
            | "document"
            | "documents"
            | "content"
            | "input"
            | "output"
            | "arguments"
            | "result"
    )
}

fn looks_like_secret(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("sk-")
        || lower.starts_with("gho_")
        || lower.starts_with("github_pat_")
        || value.starts_with("AIza")
}

struct SyntheticTraceBuilder {
    trace_id: &'static str,
    name: &'static str,
    description: &'static str,
    run_id: &'static str,
    step_id: &'static str,
    attempt_id: &'static str,
    base: DateTime<Utc>,
    events: Vec<TraceEvent>,
}

impl SyntheticTraceBuilder {
    fn new(
        trace_id: &'static str,
        name: &'static str,
        description: &'static str,
        run_id: &'static str,
    ) -> Self {
        Self {
            trace_id,
            name,
            description,
            run_id,
            step_id: "step-01",
            attempt_id: "attempt-01",
            base: Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap(),
            events: Vec::new(),
        }
    }

    fn push(
        &mut self,
        event_id: &str,
        parent_event_id: Option<&str>,
        component: Component,
        kind: EventKind,
        offset_ms: i64,
        duration_ms: Option<u64>,
        status: EventStatus,
        payload: Value,
    ) {
        let run_level = matches!(kind, EventKind::RunStarted | EventKind::RunCompleted);
        self.events.push(TraceEvent::synthetic(
            event_id,
            self.run_id,
            (!run_level).then_some(self.step_id),
            (!run_level).then_some(self.attempt_id),
            parent_event_id.map(|parent| format!("op-{parent}")),
            component,
            kind,
            self.base + Duration::milliseconds(offset_ms),
            duration_ms,
            status,
            payload,
        ));
    }

    fn finish(self) -> Trace {
        Trace {
            trace_id: self.trace_id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            run_id: self.run_id.to_string(),
            events: self.events,
        }
    }
}

fn successful_trace() -> Trace {
    let mut trace = SyntheticTraceBuilder::new(
        "trace-success-01",
        "Synthetic grounded answer",
        "A harness discovers an MCP search tool, retrieves and reranks candidates, then presents a cited answer.",
        "run-success-01",
    );
    trace.push(
        "run-start",
        None,
        Component::Harness,
        EventKind::RunStarted,
        0,
        Some(315),
        EventStatus::Running,
        json!({"scenario": "grounded_answer", "request_class": "knowledge_lookup"}),
    );
    trace.push(
        "step-start",
        Some("run-start"),
        Component::Harness,
        EventKind::StepStarted,
        5,
        Some(306),
        EventStatus::Running,
        json!({"route": "research_then_answer", "budgeted_rounds": 3}),
    );
    trace.push(
        "model-request-1",
        Some("step-start"),
        Component::Model,
        EventKind::ModelRequest,
        15,
        Some(80),
        EventStatus::Succeeded,
        json!({"model_family": "synthetic", "message_count": 1, "offered_tool_count": 1}),
    );
    trace.push(
        "model-response-1",
        Some("model-request-1"),
        Component::Model,
        EventKind::ModelResponse,
        95,
        Some(4),
        EventStatus::Succeeded,
        json!({"finish_reason": "tool_use", "selected_tool": "search_knowledge"}),
    );
    trace.push(
        "mcp-discovery",
        Some("step-start"),
        Component::Mcp,
        EventKind::McpDiscovery,
        105,
        Some(12),
        EventStatus::Succeeded,
        json!({"server_count": 1, "tool_count": 1, "resource_count": 0}),
    );
    trace.push(
        "mcp-tool-call",
        Some("step-start"),
        Component::Mcp,
        EventKind::McpToolCall,
        122,
        Some(92),
        EventStatus::Succeeded,
        json!({"tool_name": "search_knowledge", "argument_keys": ["query_category", "limit"]}),
    );
    trace.push(
        "retrieval-query",
        Some("mcp-tool-call"),
        Component::Retrieval,
        EventKind::RetrievalQuery,
        130,
        Some(35),
        EventStatus::Succeeded,
        json!({"index": "synthetic_documents", "top_k": 4, "filter_count": 1}),
    );
    trace.push(
        "retrieval-candidates",
        Some("retrieval-query"),
        Component::Retrieval,
        EventKind::RetrievalCandidates,
        166,
        Some(8),
        EventStatus::Succeeded,
        json!({"candidate_count": 4, "minimum_score": 0.61, "maximum_score": 0.92}),
    );
    trace.push(
        "rerank",
        Some("retrieval-candidates"),
        Component::Retrieval,
        EventKind::RerankCompleted,
        176,
        Some(22),
        EventStatus::Succeeded,
        json!({"method": "synthetic_cross_encoder", "input_count": 4, "selected_count": 2}),
    );
    trace.push(
        "mcp-tool-result",
        Some("mcp-tool-call"),
        Component::Mcp,
        EventKind::McpToolResult,
        201,
        Some(5),
        EventStatus::Succeeded,
        json!({"tool_name": "search_knowledge", "result_item_count": 2, "structured": true}),
    );
    trace.push(
        "memory-update",
        Some("step-start"),
        Component::Memory,
        EventKind::MemoryUpdated,
        210,
        Some(3),
        EventStatus::Succeeded,
        json!({"change": "selected_context_replaced", "item_count": 2}),
    );
    trace.push(
        "model-request-2",
        Some("step-start"),
        Component::Model,
        EventKind::ModelRequest,
        218,
        Some(73),
        EventStatus::Succeeded,
        json!({"model_family": "synthetic", "message_count": 3, "selected_context_count": 2}),
    );
    trace.push(
        "model-response-2",
        Some("model-request-2"),
        Component::Model,
        EventKind::ModelResponse,
        291,
        Some(4),
        EventStatus::Succeeded,
        json!({"finish_reason": "stop", "citation_count": 2}),
    );
    trace.push(
        "application-effect",
        Some("step-start"),
        Component::Application,
        EventKind::ApplicationSideEffect,
        300,
        Some(6),
        EventStatus::Succeeded,
        json!({"effect": "response_presented", "external": false}),
    );
    trace.push(
        "step-complete",
        Some("step-start"),
        Component::Harness,
        EventKind::StepCompleted,
        310,
        Some(1),
        EventStatus::Succeeded,
        json!({"outcome": "answer_with_citations"}),
    );
    trace.push(
        "run-complete",
        Some("run-start"),
        Component::Harness,
        EventKind::RunCompleted,
        315,
        Some(1),
        EventStatus::Succeeded,
        json!({"outcome": "success", "completed_steps": 1}),
    );
    trace.finish()
}

fn failing_trace() -> Trace {
    let mut trace = SyntheticTraceBuilder::new(
        "trace-failure-01",
        "Synthetic retrieval timeout",
        "A retrieval timeout propagates through the MCP tool result and causes the harness to stop without a side effect.",
        "run-failure-01",
    );
    trace.push(
        "run-start",
        None,
        Component::Harness,
        EventKind::RunStarted,
        0,
        Some(222),
        EventStatus::Running,
        json!({"scenario": "retrieval_timeout", "request_class": "knowledge_lookup"}),
    );
    trace.push(
        "step-start",
        Some("run-start"),
        Component::Harness,
        EventKind::StepStarted,
        5,
        Some(214),
        EventStatus::Running,
        json!({"route": "research_then_answer", "budgeted_rounds": 3}),
    );
    trace.push(
        "model-request",
        Some("step-start"),
        Component::Model,
        EventKind::ModelRequest,
        15,
        Some(70),
        EventStatus::Succeeded,
        json!({"model_family": "synthetic", "message_count": 1, "offered_tool_count": 1}),
    );
    trace.push(
        "model-response",
        Some("model-request"),
        Component::Model,
        EventKind::ModelResponse,
        85,
        Some(4),
        EventStatus::Succeeded,
        json!({"finish_reason": "tool_use", "selected_tool": "search_knowledge"}),
    );
    trace.push(
        "mcp-tool-call",
        Some("step-start"),
        Component::Mcp,
        EventKind::McpToolCall,
        95,
        Some(120),
        EventStatus::Failed,
        json!({
            "tool_name": "search_knowledge",
            "argument_keys": ["query_category", "limit"],
            "error_code": "TOOL_DEPENDENCY_FAILED",
            "error_summary": "The retrieval dependency timed out."
        }),
    );
    trace.push(
        "retrieval-query",
        Some("mcp-tool-call"),
        Component::Retrieval,
        EventKind::RetrievalQuery,
        101,
        Some(100),
        EventStatus::Failed,
        json!({
            "index": "synthetic_documents",
            "top_k": 4,
            "error_code": "VECTOR_TIMEOUT",
            "error_summary": "No candidates arrived before the 100 ms deadline."
        }),
    );
    trace.push(
        "retrieval-candidates",
        Some("retrieval-query"),
        Component::Retrieval,
        EventKind::RetrievalCandidates,
        202,
        Some(1),
        EventStatus::Failed,
        json!({
            "candidate_count": 0,
            "error_code": "VECTOR_TIMEOUT",
            "error_summary": "Candidate retrieval did not complete."
        }),
    );
    trace.push(
        "mcp-tool-result",
        Some("mcp-tool-call"),
        Component::Mcp,
        EventKind::McpToolResult,
        207,
        Some(4),
        EventStatus::Failed,
        json!({
            "tool_name": "search_knowledge",
            "result_item_count": 0,
            "error_code": "TOOL_DEPENDENCY_FAILED",
            "error_summary": "The tool returned a structured retrieval failure."
        }),
    );
    trace.push(
        "application-effect",
        Some("step-start"),
        Component::Application,
        EventKind::ApplicationSideEffect,
        214,
        Some(1),
        EventStatus::Skipped,
        json!({"effect": "response_presented", "reason": "retrieval_failure"}),
    );
    trace.push(
        "step-complete",
        Some("step-start"),
        Component::Harness,
        EventKind::StepCompleted,
        218,
        Some(1),
        EventStatus::Failed,
        json!({
            "outcome": "stopped",
            "error_code": "STEP_DEPENDENCY_FAILED",
            "error_summary": "The step stopped after the retrieval failure."
        }),
    );
    trace.push(
        "run-complete",
        Some("run-start"),
        Component::Harness,
        EventKind::RunCompleted,
        222,
        Some(1),
        EventStatus::Failed,
        json!({
            "outcome": "failed",
            "error_code": "RUN_FAILED",
            "error_summary": "The run ended without presenting an answer."
        }),
    );
    trace.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_fixtures_are_valid_and_correlated() {
        let data = workbench_data().unwrap();
        assert_eq!(data.fixtures.len(), 2);

        for fixture in data.fixtures {
            fixture.trace.validate().unwrap();
            assert_eq!(fixture.trace.events.len(), fixture.timeline.len());
            assert_eq!(fixture.trace.events.len(), fixture.explanations.len());
            assert!(fixture
                .explanations
                .iter()
                .all(|explanation| explanation.explanation_type == "derived"));
        }
    }

    #[test]
    fn sensitive_metadata_is_redacted_before_the_event_exists() {
        let event = TraceEvent::synthetic(
            "secret-event",
            "run-1",
            Some("step-1"),
            Some("attempt-1"),
            Some("op-parent".to_string()),
            Component::Model,
            EventKind::ModelRequest,
            Utc::now(),
            Some(1),
            EventStatus::Succeeded,
            json!({
                "api_key": "sk-private",
                "nested": {
                    "messages": ["private prompt"],
                    "safe_id": "doc-123",
                    "unexpected_value": "Bearer private-token"
                }
            }),
        );

        assert_eq!(event.payload["api_key"], REDACTED);
        assert_eq!(event.payload["nested"]["messages"], REDACTED);
        assert_eq!(event.payload["nested"]["unexpected_value"], REDACTED);
        assert_eq!(event.payload["nested"]["safe_id"], "doc-123");
        assert_eq!(
            event.redacted_fields,
            vec![
                "$.api_key",
                "$.nested.messages",
                "$.nested.unexpected_value"
            ]
        );
    }

    #[test]
    fn validation_rejects_out_of_order_and_late_parent_events() {
        let mut trace = successful_trace();
        trace.events[3].timestamp = trace.events[2].timestamp - Duration::milliseconds(1);
        assert!(matches!(
            trace.validate(),
            Err(TraceValidationError::OutOfOrder(_))
        ));

        let mut trace = successful_trace();
        trace.events[2].parent_operation_id = Some("op-not-yet-seen".to_string());
        assert!(matches!(
            trace.validate(),
            Err(TraceValidationError::UnknownOrLateParent { .. })
        ));
    }

    #[test]
    fn validation_rejects_duplicate_ids_and_unsafe_mutation() {
        let mut trace = successful_trace();
        trace.events[2].event_id = trace.events[1].event_id.clone();
        assert!(matches!(
            trace.validate(),
            Err(TraceValidationError::DuplicateEventId(_))
        ));

        let mut trace = successful_trace();
        trace.events[2].payload["authorization"] = Value::String("Bearer leaked".to_string());
        assert!(matches!(
            trace.validate(),
            Err(TraceValidationError::UnsafePayload { .. })
        ));
    }

    #[test]
    fn failed_events_require_diagnostic_metadata() {
        let mut trace = failing_trace();
        trace.events[5]
            .payload
            .as_object_mut()
            .unwrap()
            .remove("error_code");
        assert!(matches!(
            trace.validate(),
            Err(TraceValidationError::MissingFailureMetadata(_))
        ));
    }

    #[test]
    fn timeline_transform_preserves_lanes_offsets_and_depth() {
        let trace = successful_trace();
        let rows = timeline_rows(&trace).unwrap();
        let retrieval = rows
            .iter()
            .find(|row| row.event_id == "retrieval-query")
            .unwrap();
        let run = rows.iter().find(|row| row.event_id == "run-start").unwrap();

        assert_eq!(run.offset_ms, 0);
        assert_eq!(run.depth, 0);
        assert_eq!(retrieval.component, Component::Retrieval);
        assert_eq!(retrieval.offset_ms, 130);
        assert_eq!(retrieval.depth, 3);
        assert_eq!(
            retrieval.parent_operation_id.as_deref(),
            Some("op-mcp-tool-call")
        );
    }
}
