//! Batch files: many models run as one job (`openkeito batch`).

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::diagnostic::Diagnostic;
use crate::model::Model;

/// Value of the `format` field of a batch file.
pub const BATCH_FORMAT: &str = "openkeito-batch";
/// Current (and only) value of the `version` field of a batch file.
pub const BATCH_VERSION: &str = "0.1";

/// A list of cases run as one job. Each case runs on its own (in parallel
/// when several threads are allowed) and writes its own output directory,
/// named after the case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    /// Must be "openkeito-batch".
    pub format: String,
    /// Must be "0.1".
    pub version: String,
    /// Short name of the job.
    #[serde(default)]
    pub name: String,
    /// Free text.
    #[serde(default)]
    pub description: String,
    pub cases: Vec<BatchCase>,
}

/// One case of a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchCase {
    /// Unique name; also the name of the case's output directory. Letters,
    /// digits, '_', '-' and '.', not ending with '.'. Names that differ only
    /// in upper and lower case count as the same.
    pub name: String,
    /// The model: a path to a model file (relative to the batch file) or the
    /// model itself.
    pub model: ModelRef,
}

/// A model given by its file or written out.
///
/// A written-out model is kept as JSON and read like a model file when its
/// case runs, so that a mistake in it is reported for that case, with the
/// diagnostics of a model, and does not stop the other cases.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ModelRef {
    Path(String),
    Inline(#[schemars(with = "Model")] Box<RawValue>),
}

impl PartialEq for ModelRef {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ModelRef::Path(a), ModelRef::Path(b)) => a == b,
            (ModelRef::Inline(a), ModelRef::Inline(b)) => a.get() == b.get(),
            _ => false,
        }
    }
}

impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let s = raw.get().trim_start();
        if s.starts_with('"') {
            serde_json::from_str::<String>(s)
                .map(ModelRef::Path)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(ModelRef::Inline(raw))
        }
    }
}

/// Files that a batch writes into its output directory, next to the
/// directories of the cases.
const BATCH_FILES: [&str; 2] = ["batch-summary.json", "batch-metrics.csv"];

/// Why a case name cannot be used, or `None`. A case name is also the name
/// of a directory, on every operating system.
fn case_name_problem(s: &str) -> Option<&'static str> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    if s.is_empty() || !s.chars().all(allowed) {
        return Some("is not valid");
    }
    if s.ends_with('.') {
        // "." and "..", and names that Windows stores without the dot.
        return Some("ends with '.'");
    }
    let lower = s.to_ascii_lowercase();
    if BATCH_FILES.contains(&lower.as_str()) {
        return Some("is the name of a file of the batch itself");
    }
    // Windows device names, with or without an extension.
    let stem = lower.split('.').next().unwrap_or("");
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
    };
    if matches!(stem, "con" | "prn" | "aux" | "nul") || numbered("com") || numbered("lpt") {
        return Some("is a device name on Windows");
    }
    None
}

/// Checks the names of the cases of one batch: each must be usable as a
/// directory name and differ from the others, also when upper and lower case
/// are not told apart (as on Windows and macOS).
pub fn check_case_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<Diagnostic> {
    let mut d = Vec::new();
    let mut seen: HashMap<String, (usize, &str)> = HashMap::new();
    for (i, name) in names.into_iter().enumerate() {
        if let Some(problem) = case_name_problem(name) {
            d.push(Diagnostic::error(
                "E402",
                format!("cases[{i}].name"),
                format!("case name \"{name}\" {problem}"),
                "The name is also a directory name: use letters, digits, '_', '-' and '.' only, \
                 not ending with '.', and none of batch-summary.json, batch-metrics.csv or a \
                 Windows device name (con, nul, com1, ...).",
            ));
        } else if let Some((first, other)) = seen.get(&name.to_ascii_lowercase()) {
            d.push(Diagnostic::error(
                "E402",
                format!("cases[{i}].name"),
                if *other == name {
                    format!("case name \"{name}\" is already used by cases[{first}]")
                } else {
                    format!(
                        "case name \"{name}\" differs from \"{other}\" (cases[{first}]) only in \
                         upper and lower case"
                    )
                },
                "Give every case a different name; upper and lower case do not tell two \
                 directories apart on every operating system.",
            ));
        } else {
            seen.insert(name.to_ascii_lowercase(), (i, name));
        }
    }
    d
}

/// Checks the batch file itself (not the models of its cases).
pub fn validate_batch(batch: &Batch) -> Vec<Diagnostic> {
    let mut d = Vec::new();
    if batch.format != BATCH_FORMAT {
        d.push(Diagnostic::error(
            "E400",
            "format",
            format!("format is \"{}\"", batch.format),
            format!("Set \"format\": \"{BATCH_FORMAT}\"."),
        ));
    }
    if batch.version != BATCH_VERSION {
        d.push(Diagnostic::error(
            "E400",
            "version",
            format!("version \"{}\" is not supported", batch.version),
            format!("Set \"version\": \"{BATCH_VERSION}\"."),
        ));
    }
    if batch.cases.is_empty() {
        d.push(Diagnostic::error(
            "E401",
            "cases",
            "the batch has no cases",
            "Add cases as {\"name\": \"case1\", \"model\": \"case1.json\"}.",
        ));
    }
    for (i, c) in batch.cases.iter().enumerate() {
        if matches!(&c.model, ModelRef::Inline(v) if !v.get().trim_start().starts_with('{')) {
            d.push(Diagnostic::error(
                "E403",
                format!("cases[{i}].model"),
                format!(
                    "the model of case \"{}\" is neither a path nor a model",
                    c.name
                ),
                "Give the path of a model file as a string, or the model itself as a JSON object.",
            ));
        }
    }
    d.extend(check_case_names(
        batch.cases.iter().map(|c| c.name.as_str()),
    ));
    d
}
