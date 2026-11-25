//! Model format of OpenKeito: data structures, JSON loading, validation and
//! the component catalog.
//!
//! A model is one JSON document. Every physical quantity carries its unit in
//! the field name (`resistance_ohm`, `time_step_s`), unknown fields are
//! errors, and numbers must be valid JSON numbers, so a value such as
//! `1.1141-6` (missing exponent letter) is rejected instead of being read as
//! `1.1141`.

mod batch;
mod catalog;
mod check;
mod components;
mod controls;
mod diagnostic;
mod model;
mod read;
mod schedule;
mod topology;
mod validate;

pub use batch::{
    check_case_names, validate_batch, Batch, BatchCase, ModelRef, BATCH_FORMAT, BATCH_VERSION,
};
pub use catalog::{component_catalog, ComponentInfo, ParameterInfo};
use diagnostic::DocumentKind;
pub use diagnostic::{Diagnostic, Severity, Subject};
pub use model::*;
pub use schedule::{
    parse_date_time, validate_schedule, Interpolation, Schedule, ScheduledQuantity, Series,
    SCHEDULE_FORMAT, SCHEDULE_VERSION,
};
pub use topology::{
    topology, BusTopology, ComponentTopology, ControlTopology, Mark, NodeTopology, SignalEnd,
    SignalTopology, SignalUse, TerminalTopology, Topology,
};
pub use validate::{source_frequency, step_count, validate};

/// A successfully loaded model and the warnings found while validating it.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub model: Model,
    pub warnings: Vec<Diagnostic>,
}

/// The text without a leading byte order mark.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// The `format` field of a JSON document, to tell models, batch files and
/// schedules apart. `None` when the text is not a JSON object with one.
pub fn document_format(text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(strip_bom(text))
        .ok()?
        .get("format")?
        .as_str()
        .map(str::to_owned)
}

/// Parses a JSON document of one kind and checks it. Returns the document
/// and its warnings, or every error found (not only the first). A leading
/// byte order mark is ignored.
fn load_document<T: serde::de::DeserializeOwned>(
    text: &str,
    kind: DocumentKind,
    check: impl Fn(&T) -> Vec<Diagnostic>,
) -> Result<(T, Vec<Diagnostic>), Vec<Diagnostic>> {
    let text = strip_bom(text);
    let document: T = match serde_json::from_str(text) {
        Ok(d) => d,
        Err(e) => return Err(vec![diagnostic::from_parse_error(text, &e, kind)]),
    };
    let diagnostics = check(&document);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics);
    }
    Ok((document, diagnostics))
}

/// Parses and validates a model. Returns every error found, not only the first.
pub fn load_str(text: &str) -> Result<Loaded, Vec<Diagnostic>> {
    let model = read::read_model(text)?;
    let diagnostics = validate(&model);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics);
    }
    Ok(Loaded {
        model,
        warnings: diagnostics,
    })
}

/// Parses a model and validates it. Unlike [`load_str`], a model that can be
/// read comes back also when validation finds errors in it, with every
/// error and warning: for what describes a model that is not valid yet (its
/// connections, for a diagram). The error is the one of a text that is not a
/// model (E001).
pub fn parse_str(text: &str) -> Result<(Model, Vec<Diagnostic>), Vec<Diagnostic>> {
    let model = read::read_model(text)?;
    let diagnostics = validate(&model);
    Ok((model, diagnostics))
}

/// JSON Schema of the model format, for editors and AI agents.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Model)).expect("schema serializes")
}

/// Parses and checks a batch file (not the models of its cases).
pub fn load_batch_str(text: &str) -> Result<Batch, Vec<Diagnostic>> {
    load_document(text, DocumentKind::Batch, validate_batch).map(|(b, _)| b)
}

/// JSON Schema of the batch file format.
pub fn batch_json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Batch)).expect("schema serializes")
}

/// Parses and checks a schedule file.
pub fn load_schedule_str(text: &str) -> Result<Schedule, Vec<Diagnostic>> {
    load_document(text, DocumentKind::Schedule, validate_schedule).map(|(s, _)| s)
}

/// JSON Schema of the schedule file format.
pub fn schedule_json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Schedule)).expect("schema serializes")
}
