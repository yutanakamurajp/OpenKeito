//! Running models and writing their results: the work behind the
//! `openkeito` command, shared with the Python package.
//!
//! Every entry point returns the JSON document that the command prints (the
//! summary) and an exit code: 0 success, 1 the simulation failed, 2 the model
//! or the request is invalid.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use openkeito_core::clock::Stopwatch;
use openkeito_core::{FrequencyScanner, Simulator};
use openkeito_model::{Diagnostic, Loaded};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const EXIT_FAILED: u8 = 1;
pub const EXIT_INVALID: u8 = 2;

/// Switching events listed in summary.json; events.csv has all of them.
const SUMMARY_EVENTS: usize = 100;

pub fn request_error(path: impl Into<String>, message: String, hint: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error("E300", path, message, hint)]
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Parses and validates a model given as text. Returns the loaded model and
/// the SHA-256 of the text.
pub fn load_text(text: &str) -> Result<(Loaded, String), Vec<Diagnostic>> {
    match openkeito_model::document_format(text).as_deref() {
        Some(openkeito_model::BATCH_FORMAT) => {
            return Err(request_error(
                "format",
                "this is a batch file (format \"openkeito-batch\"), not a model".to_string(),
                "Run batch files with `openkeito batch <file> --out <dir>` (or \
                 `openkeito.run_batch` in Python).",
            ));
        }
        Some(openkeito_model::SCHEDULE_FORMAT) => {
            return Err(request_error(
                "format",
                "this is a schedule file (format \"openkeito-schedule\"), not a model".to_string(),
                "Validate schedule files with `openkeito validate <file>`, or convert them to \
                 models with the Python `openkeito.schedule` / `openkeito.transition` tools.",
            ));
        }
        _ => {}
    }
    let sha256 = sha256_hex(text.as_bytes());
    Ok((openkeito_model::load_str(text)?, sha256))
}

/// Reads and validates a model file. Returns the loaded model and the
/// SHA-256 of the file.
pub fn load_file(path: &Path) -> Result<(Loaded, String), Vec<Diagnostic>> {
    let bytes = fs::read(path).map_err(|e| {
        request_error(
            path.display().to_string(),
            format!("the model file cannot be read: {e}"),
            "Check the path.",
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        request_error(
            path.display().to_string(),
            "the model file is not UTF-8 text".to_string(),
            "Save the model as UTF-8 JSON.",
        )
    })?;
    load_text(&text)
}

/// The document of `openkeito validate` for a model labelled `label`.
pub fn validate_report(
    label: &str,
    result: Result<(Loaded, String), Vec<Diagnostic>>,
) -> (Value, u8) {
    match result {
        Ok((loaded, sha256)) => (
            json!({
                "tool": "openkeito",
                "version": env!("CARGO_PKG_VERSION"),
                "valid": true,
                "model": {"path": label, "name": loaded.model.name, "sha256": sha256},
                "steps": openkeito_model::step_count(&loaded.model.simulation),
                "components": loaded.model.components.len(),
                "errors": [],
                "warnings": loaded.warnings,
            }),
            0,
        ),
        Err(diagnostics) => invalid_document("model", label, diagnostics),
    }
}

/// The diagnostics of a document that is not valid, as its `errors` and its
/// `warnings`. What refuses a document comes with the warnings found beside
/// the errors; every document of the command keeps the two apart, as the
/// document of a valid model does.
pub fn by_severity(diagnostics: Vec<Diagnostic>) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    diagnostics
        .into_iter()
        .partition(|d| d.severity == openkeito_model::Severity::Error)
}

/// Shared JSON envelope for an invalid `validate` report (`valid: false`) or
/// a failed request (`status: "failed"`).
fn diagnostic_envelope(
    kind: &str,
    label: &str,
    diagnostics: Vec<Diagnostic>,
    extra: &[(&str, Value)],
) -> Value {
    let (errors, warnings) = by_severity(diagnostics);
    let mut doc = json!({
        "tool": "openkeito",
        "version": env!("CARGO_PKG_VERSION"),
        kind: {"path": label},
        "errors": errors,
        "warnings": warnings,
    });
    let obj = doc.as_object_mut().expect("envelope is an object");
    for (k, v) in extra {
        obj.insert((*k).to_string(), v.clone());
    }
    doc
}

/// Builds an invalid-document report (`"valid": false`) for `kind`
/// (`"model"`, `"schedule"`, or `"batch"`).
pub fn invalid_document(kind: &str, label: &str, diagnostics: Vec<Diagnostic>) -> (Value, u8) {
    (
        diagnostic_envelope(kind, label, diagnostics, &[("valid", json!(false))]),
        EXIT_INVALID,
    )
}

/// The document of `openkeito validate` for a schedule file.
pub fn validate_schedule_report(label: &str, text: &str) -> (Value, u8) {
    let sha256 = sha256_hex(text.as_bytes());
    match openkeito_model::load_schedule_str(text) {
        Ok(s) => (
            json!({
                "tool": "openkeito",
                "version": env!("CARGO_PKG_VERSION"),
                "valid": true,
                "schedule": {"path": label, "name": s.name, "sha256": sha256},
                "times": s.times.len(),
                "series": s.series.len(),
                "errors": [],
                "warnings": [],
            }),
            0,
        ),
        Err(diagnostics) => invalid_document("schedule", label, diagnostics),
    }
}

/// The document of a request that failed before or while running.
/// `diagnostics` may hold warnings beside the errors (those of a model that
/// is not valid): they go to `warnings`.
pub fn failed(label: &str, diagnostics: Vec<Diagnostic>) -> Value {
    diagnostic_envelope("model", label, diagnostics, &[("status", json!("failed"))])
}

/// The document of a failed `batch` request before its cases ran.
pub fn failed_batch(label: &str, diagnostics: Vec<Diagnostic>) -> Value {
    diagnostic_envelope(
        "batch",
        label,
        diagnostics,
        &[("command", json!("batch")), ("status", json!("failed"))],
    )
}

/// The document of `openkeito topology` for a model given as text: its
/// components with their terminals and nodes, its buses, its control blocks
/// with the signals between them, and what each diagnostic is about
/// (`openkeito_model::Topology`), with `valid`, `errors` and `warnings` as
/// `validate` reports them. A model that can be read has its connections
/// also when it is not valid (exit code 2); a text that cannot be read has
/// only the errors. The document does not depend on where the text came
/// from: the command, the WebAssembly module and the Python package return
/// the same one for the same text.
pub fn topology_report(text: &str) -> (Value, u8) {
    let envelope = topology_envelope;
    let wrong_kind = |what: &str, hint: &str| {
        let message = format!("this is a {what}, not a model");
        (
            envelope(false, request_error("format", message, hint)),
            EXIT_INVALID,
        )
    };
    match openkeito_model::document_format(text).as_deref() {
        Some(openkeito_model::BATCH_FORMAT) => {
            return wrong_kind(
                "batch file (format \"openkeito-batch\")",
                "Give the model of one case: `openkeito topology <model>`.",
            )
        }
        Some(openkeito_model::SCHEDULE_FORMAT) => {
            return wrong_kind(
                "schedule file (format \"openkeito-schedule\")",
                "Give a model: a schedule file has no circuit. The Python `openkeito.schedule` \
                 and `openkeito.transition` tools make models of a schedule.",
            )
        }
        _ => {}
    }
    let (model, diagnostics) = match openkeito_model::parse_str(text) {
        Ok(read) => read,
        Err(errors) => return (envelope(false, errors), EXIT_INVALID),
    };
    let topology = openkeito_model::topology(&model, &diagnostics);
    let valid = diagnostics
        .iter()
        .all(|d| d.severity != openkeito_model::Severity::Error);
    let mut document = envelope(valid, diagnostics);
    let object = document.as_object_mut().expect("envelope is an object");
    object.insert(
        "model".into(),
        json!({"name": model.name, "sha256": sha256_hex(text.as_bytes())}),
    );
    let parts = serde_json::to_value(&topology).expect("topology serializes");
    if let Value::Object(parts) = parts {
        object.extend(parts);
    }
    (document, if valid { 0 } else { EXIT_INVALID })
}

/// The head of the document of `openkeito topology`.
fn topology_envelope(valid: bool, diagnostics: Vec<Diagnostic>) -> Value {
    let (errors, warnings) = by_severity(diagnostics);
    json!({
        "tool": "openkeito",
        "version": env!("CARGO_PKG_VERSION"),
        "command": "topology",
        "valid": valid,
        "errors": errors,
        "warnings": warnings,
    })
}

/// Name of the file that `openkeito topology --out <dir>` writes.
pub const TOPOLOGY_FILE: &str = "topology.json";

/// `openkeito topology <model> [--out <dir>]`: the document of
/// [`topology_report`] for a model file. With `out`, the document is also
/// written into that directory as `topology.json`; a file of that name that
/// is already there is refused unless `force` is given.
pub fn topology_file(path: &Path, out: Option<&Path>, force: bool) -> (Value, u8) {
    let refused = |errors: Vec<Diagnostic>| (topology_envelope(false, errors), EXIT_INVALID);
    let text = match fs::read(path).map(String::from_utf8) {
        Ok(Ok(text)) => text,
        Ok(Err(_)) => {
            return refused(request_error(
                path.display().to_string(),
                "the model file is not UTF-8 text".to_string(),
                "Save the model as UTF-8 JSON.",
            ))
        }
        Err(e) => {
            return refused(request_error(
                path.display().to_string(),
                format!("the model file cannot be read: {e}"),
                "Check the path.",
            ))
        }
    };
    let (document, code) = topology_report(&text);
    let Some(out) = out else {
        return (document, code);
    };
    let file = out.join(TOPOLOGY_FILE);
    if file.exists() && !force {
        return refused(request_error(
            file.display().to_string(),
            "the output directory already holds a topology.json".to_string(),
            "Use a new output directory, or pass --force to overwrite.",
        ));
    }
    let written = fs::create_dir_all(out).and_then(|()| {
        let text = serde_json::to_string_pretty(&document).expect("document serializes");
        fs::write(&file, text + "\n")
    });
    match written {
        Ok(()) => (document, code),
        Err(e) => refused(not_writable(&file, "topology.json", &e)),
    }
}

/// Where the connections that `topology` reports for a validated model
/// differ from the circuit the solver builds of it
/// (`openkeito_core::component_nodes`); empty when they agree. For every
/// component: the nodes of its terminals are the nodes its expansion into
/// elements asked for, and where it has branch voltages, U(<name>) is
/// V(first terminal) - V(second terminal), phase by phase. The nodes other
/// than gnd must be the same; gnd must be a terminal where the circuit asks
/// for it by name. A connection to gnd that a component makes inside (the
/// shunt capacitances of a line, the star point of a machine) is not seen.
pub fn topology_mismatches(model: &openkeito_model::Model) -> Vec<String> {
    let topology = openkeito_model::topology(model, &[]);
    compare_with_netlist(&topology, &openkeito_core::component_nodes(model))
}

/// See [`topology_mismatches`].
pub fn compare_with_netlist(
    topology: &openkeito_model::Topology,
    built: &[openkeito_core::ComponentNodes],
) -> Vec<String> {
    use std::collections::BTreeSet;
    let ground = openkeito_model::GROUND;
    let mut found = Vec::new();
    if topology.components.len() != built.len() {
        found.push(format!(
            "topology has {} components, the circuit {}",
            topology.components.len(),
            built.len()
        ));
    }
    for (c, b) in topology.components.iter().zip(built) {
        if c.name != b.name {
            found.push(format!(
                "component \"{}\" is \"{}\" in the circuit",
                c.name, b.name
            ));
            continue;
        }
        let ours: BTreeSet<&str> = c
            .terminals
            .iter()
            .flat_map(|t| t.nodes.iter().map(String::as_str))
            .collect();
        let mut theirs: BTreeSet<&str> = b.nodes.iter().map(String::as_str).collect();
        // A star point that is grounded inside (a synchronous machine) is a
        // terminal at gnd that the expansion does not ask for by name.
        if ours.contains(ground) {
            theirs.insert(ground);
        }
        if ours != theirs {
            found.push(format!(
                "\"{}\" ({}): the terminals name the nodes {ours:?}, the circuit uses {theirs:?}",
                c.name, c.type_name
            ));
        }
        for (suffix, terms) in &b.voltages {
            let [first, second] = &c.terminals[..] else {
                found.push(format!(
                    "\"{}\" has a branch voltage in the circuit and {} terminals",
                    c.name,
                    c.terminals.len()
                ));
                break;
            };
            // "" of a component with two nodes, ".a" of a phase.
            let phase = openkeito_model::PHASES
                .iter()
                .position(|p| suffix.strip_prefix('.') == Some(p));
            let at = |t: &openkeito_model::TerminalTopology| match (phase, t.nodes.len()) {
                (_, 1) | (None, _) => t.nodes.first().cloned(),
                (Some(p), _) => t.nodes.get(p).cloned(),
            };
            let mut expected: Vec<(String, f64)> = [(at(first), 1.0), (at(second), -1.0)]
                .into_iter()
                .filter_map(|(n, c)| n.filter(|n| n != ground).map(|n| (n, c)))
                .collect();
            let mut terms = terms.clone();
            let order = |a: &(String, f64), b: &(String, f64)| a.0.cmp(&b.0);
            expected.sort_by(order);
            terms.sort_by(order);
            if expected != terms {
                found.push(format!(
                    "U({}{suffix}) is {terms:?} in the circuit, {expected:?} by the terminals",
                    c.name
                ));
            }
        }
    }
    found
}

/// Shortest text that reads back as exactly the same number.
pub fn number(v: f64) -> String {
    format!("{v:e}")
}

/// Where the text of a result file goes: into the file, or into memory.
enum Sink {
    File(BufWriter<fs::File>),
    Memory(Vec<u8>),
}

impl Sink {
    fn create(path: &Path) -> std::io::Result<Sink> {
        Ok(Sink::File(BufWriter::new(fs::File::create(path)?)))
    }

    fn memory() -> Sink {
        Sink::Memory(Vec::new())
    }

    /// Flushes and closes the file; returns the text kept in memory.
    fn finish(self) -> std::io::Result<Option<String>> {
        match self {
            Sink::File(mut w) => w.flush().map(|_| None),
            Sink::Memory(bytes) => Ok(Some(
                String::from_utf8(bytes).expect("the result files are UTF-8"),
            )),
        }
    }
}

impl Write for Sink {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::File(w) => w.write(buf),
            Sink::Memory(bytes) => bytes.write(buf),
        }
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Sink::File(w) => w.write_all(buf),
            Sink::Memory(bytes) => {
                bytes.extend_from_slice(buf);
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::File(w) => w.flush(),
            Sink::Memory(_) => Ok(()),
        }
    }
}

/// Writes a whole file of a result: into `path`, or into memory (the text
/// is returned).
fn write_text(
    path: Option<&Path>,
    content: impl FnOnce(&mut Sink) -> std::io::Result<()>,
) -> std::io::Result<Option<String>> {
    let mut sink = match path {
        Some(p) => Sink::create(p)?,
        None => Sink::memory(),
    };
    content(&mut sink)?;
    sink.finish()
}

fn write_events(w: &mut Sink, events: &[openkeito_core::SwitchEvent]) -> std::io::Result<()> {
    writeln!(w, "time_s,component,action,interrupted_current_a")?;
    for e in events {
        let current = e.interrupted_current_a.map(number).unwrap_or_default();
        writeln!(
            w,
            "{},{},{},{}",
            number(e.time_s),
            e.component,
            e.action,
            current
        )?;
    }
    Ok(())
}

/// Phasors of the steady state: name, unit, rms magnitude, angle (cosine
/// reference), real and imaginary parts.
fn write_phasors(w: &mut Sink, phasors: &[openkeito_core::PhasorValue]) -> std::io::Result<()> {
    writeln!(w, "quantity,unit,rms,angle_deg,real,imag")?;
    for p in phasors {
        writeln!(
            w,
            "{},{},{},{},{},{}",
            p.name,
            p.unit,
            number(p.value.norm()),
            number(p.value.arg().to_degrees()),
            number(p.value.re),
            number(p.value.im)
        )?;
    }
    Ok(())
}

/// What to do with the result of a run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Directory for the result files; `None` keeps everything in memory.
    pub out: Option<PathBuf>,
    /// Overwrite an existing result in `out`.
    pub force: bool,
    /// Keep every row in memory (for the Python package).
    pub keep_rows: bool,
    /// Largest number of threads of the run, instead of `simulation.threads`
    /// of the model: `--threads` of `openkeito run`, and 1 for the cases of
    /// a batch that runs several cases at the same time.
    pub threads: Option<usize>,
}

/// The rows of a run kept in memory: one vector per column.
#[derive(Debug, Clone, Default)]
pub struct Rows {
    /// `time_s` or `frequency_hz`.
    pub axis: Vec<f64>,
    pub names: Vec<String>,
    pub values: Vec<Vec<f64>>,
}

/// The summary of a run, its exit code and, if asked for, its rows.
#[derive(Debug, Clone)]
pub struct RunOutput {
    pub summary: Value,
    pub code: u8,
    pub rows: Option<Rows>,
    /// The result files as text, by file name ("result.csv", "events.csv",
    /// "phasors.csv", "scan.csv"), byte for byte what a run into a directory
    /// writes: only from [`run_texts`] and [`scan_texts`].
    pub texts: Vec<(&'static str, String)>,
}

impl RunOutput {
    /// The model or the request is invalid: nothing ran.
    pub fn invalid(label: &str, errors: Vec<Diagnostic>) -> RunOutput {
        RunOutput {
            summary: failed(label, errors),
            code: EXIT_INVALID,
            rows: None,
            texts: Vec::new(),
        }
    }

    /// The run could not write its result.
    fn failed(label: &str, errors: Vec<Diagnostic>, rows: Option<Rows>) -> RunOutput {
        RunOutput {
            summary: failed(label, errors),
            code: EXIT_FAILED,
            rows,
            texts: Vec::new(),
        }
    }
}

/// Smallest, largest and largest absolute value and the last value of each
/// column, updated row by row.
#[derive(Debug, Clone)]
struct Statistics {
    min: Vec<f64>,
    max: Vec<f64>,
    max_abs: Vec<f64>,
    last: Vec<f64>,
}

impl Statistics {
    fn new(n: usize) -> Self {
        Statistics {
            min: vec![f64::INFINITY; n],
            max: vec![f64::NEG_INFINITY; n],
            max_abs: vec![0.0; n],
            last: vec![f64::NAN; n],
        }
    }

    fn add(&mut self, values: &[f64]) {
        for (k, &v) in values.iter().enumerate() {
            if v.is_finite() {
                self.min[k] = self.min[k].min(v);
                self.max[k] = self.max[k].max(v);
                self.max_abs[k] = self.max_abs[k].max(v.abs());
            }
            self.last[k] = v;
        }
    }

    fn to_json(&self, names: &[String]) -> Value {
        let finite = |v: f64| if v.is_finite() { json!(v) } else { Value::Null };
        Value::Array(
            names
                .iter()
                .enumerate()
                .map(|(k, n)| {
                    json!({
                        "name": n,
                        "min": finite(self.min[k]),
                        "max": finite(self.max[k]),
                        "max_abs": finite(self.max_abs[k]),
                        "last": finite(self.last[k]),
                    })
                })
                .collect(),
        )
    }
}

/// The files of a result (of a run or of a scan), `summary.json` first:
/// without it, what is left of an interrupted run does not pass for a result.
const RESULT_FILES: [&str; 5] = [
    "summary.json",
    "result.csv",
    "events.csv",
    "phasors.csv",
    "scan.csv",
];

fn not_writable(path: &Path, what: &str, e: &std::io::Error) -> Vec<Diagnostic> {
    request_error(
        path.display().to_string(),
        format!("{what} cannot be written: {e}"),
        "Check that the output directory is writable.",
    )
}

/// Removes any existing result files (`summary.json`, `result.csv`,
/// `events.csv`, `phasors.csv`, `scan.csv`) from `out` so a `--force` run
/// never leaves an old completed result behind when the new run is invalid.
pub fn clear_stale_results(out: &Path) -> Result<(), Vec<Diagnostic>> {
    for path in RESULT_FILES
        .iter()
        .map(|n| out.join(n))
        .filter(|p| p.exists())
    {
        fs::remove_file(&path).map_err(|e| {
            request_error(
                path.display().to_string(),
                format!("the old result cannot be removed: {e}"),
                "Close programs that hold the file open, or use a new output directory.",
            )
        })?;
    }
    Ok(())
}

/// The output directory of one result.
struct OutDir {
    dir: PathBuf,
}

impl OutDir {
    /// Creates the directory. A result that is already there is refused, or
    /// with `force` removed (every file of it, so that nothing of the old
    /// result stays next to the new one; other files are not touched).
    fn prepare(out: &Path, force: bool) -> Result<OutDir, Vec<Diagnostic>> {
        let has_existing = RESULT_FILES.iter().any(|n| out.join(n).exists());
        if !force && has_existing {
            return Err(request_error(
                out.display().to_string(),
                "the output directory already holds a result".to_string(),
                "Use a new output directory for every run, or pass --force to overwrite.",
            ));
        }
        if force {
            clear_stale_results(out)?;
        }
        fs::create_dir_all(out).map_err(|e| {
            request_error(
                out.display().to_string(),
                format!("the output directory cannot be created: {e}"),
                "Check that the output directory is writable.",
            )
        })?;
        Ok(OutDir {
            dir: out.to_path_buf(),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn display(&self, name: &str) -> String {
        self.path(name).display().to_string()
    }
}

/// The rows of a result on their way to the CSV file (or its text in
/// memory) and, if asked for, into memory as numbers.
struct Table {
    writer: Option<Sink>,
    rows: Option<Rows>,
    line: String,
    written: u64,
}

impl Table {
    /// Creates `file` in `dir` (when there is one; without one, with
    /// `keep_text`, its text in memory) and writes its header: `axis` and
    /// the column names.
    fn create(
        dir: Option<&OutDir>,
        file: &str,
        axis: &str,
        names: &[String],
        keep_rows: bool,
        keep_text: bool,
    ) -> Result<Table, Vec<Diagnostic>> {
        let mut line = String::from(axis);
        for n in names {
            line.push(',');
            line.push_str(n);
        }
        line.push('\n');
        let writer = match dir {
            Some(d) => {
                let path = d.path(file);
                let mut w =
                    Sink::create(&path).map_err(|e| not_writable(&path, "the result file", &e))?;
                w.write_all(line.as_bytes())
                    .map_err(|e| not_writable(&path, "the result file", &e))?;
                Some(w)
            }
            None if keep_text => {
                let mut w = Sink::memory();
                w.write_all(line.as_bytes()).expect("memory is writable");
                Some(w)
            }
            None => None,
        };
        Ok(Table {
            writer,
            rows: keep_rows.then(|| Rows {
                axis: Vec::new(),
                names: names.to_vec(),
                values: vec![Vec::new(); names.len()],
            }),
            line,
            written: 0,
        })
    }

    /// Adds one row. A value that does not exist is an empty cell in the
    /// file and NaN in memory.
    fn row(
        &mut self,
        axis: f64,
        values: impl Iterator<Item = Option<f64>> + Clone,
    ) -> std::io::Result<()> {
        if let Some(w) = self.writer.as_mut() {
            self.line.clear();
            self.line.push_str(&number(axis));
            for v in values.clone() {
                self.line.push(',');
                if let Some(v) = v {
                    self.line.push_str(&number(v));
                }
            }
            self.line.push('\n');
            w.write_all(self.line.as_bytes())?;
        }
        if let Some(r) = self.rows.as_mut() {
            r.axis.push(axis);
            for (col, v) in r.values.iter_mut().zip(values) {
                col.push(v.unwrap_or(f64::NAN));
            }
        }
        self.written += 1;
        Ok(())
    }

    /// Flushes and closes the file; returns the text kept in memory.
    fn finish(&mut self) -> std::io::Result<Option<String>> {
        match self.writer.take() {
            Some(w) => w.finish(),
            None => Ok(None),
        }
    }
}

/// Writes `summary.json` (when there is a directory) and returns the output
/// of the command.
fn finish(
    label: &str,
    dir: Option<&OutDir>,
    summary: Value,
    code: u8,
    rows: Option<Rows>,
    texts: Vec<(&'static str, String)>,
) -> RunOutput {
    if let Some(d) = dir {
        let path = d.path("summary.json");
        let text = serde_json::to_string_pretty(&summary).expect("summary serializes");
        if let Err(e) = fs::write(&path, text + "\n") {
            return RunOutput::failed(label, not_writable(&path, "summary.json", &e), rows);
        }
    }
    RunOutput {
        summary,
        code,
        rows,
        texts,
    }
}

/// Runs a loaded model. `label` names it in the summary (its path, or
/// "<inline>").
pub fn run_loaded(label: &str, loaded: &Loaded, sha256: &str, opts: &RunOptions) -> RunOutput {
    run_to(label, loaded, sha256, opts, false)
}

/// Runs a loaded model without touching any file: the result files come
/// back as text in `texts` of the output ("result.csv", "events.csv" and,
/// with a steady-state initialization, "phasors.csv"), byte for byte what
/// [`run_loaded`] writes into a directory. The summary is the one of a run
/// without a directory. For hosts without files (the browser).
pub fn run_texts(label: &str, loaded: &Loaded, sha256: &str, threads: Option<usize>) -> RunOutput {
    let opts = RunOptions {
        threads,
        ..RunOptions::default()
    };
    run_to(label, loaded, sha256, &opts, true)
}

/// Computes the frequency scan of a loaded model without touching any file:
/// "scan.csv" comes back as text in `texts` of the output.
pub fn scan_texts(label: &str, loaded: &Loaded, sha256: &str) -> RunOutput {
    scan_to(label, loaded, sha256, &RunOptions::default(), true)
}

/// `keep_text`: without an output directory, keep the result files as text.
fn run_to(
    label: &str,
    loaded: &Loaded,
    sha256: &str,
    opts: &RunOptions,
    keep_text: bool,
) -> RunOutput {
    let model = &loaded.model;
    if opts.force {
        if let Some(out) = opts.out.as_deref() {
            if let Err(e) = clear_stale_results(out) {
                return RunOutput::invalid(label, e);
            }
        }
    }
    let build_started = Stopwatch::start();
    let mut simulator = match Simulator::from_model(model) {
        Ok(s) => s,
        Err(errors) => return RunOutput::invalid(label, errors),
    };
    if let Some(threads) = opts.threads {
        simulator.set_threads(threads);
    }
    let build_s = build_started.elapsed_s();

    let dir = match opts
        .out
        .as_deref()
        .map(|out| OutDir::prepare(out, opts.force))
    {
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return RunOutput::invalid(label, e),
        None => None,
    };
    let columns = simulator.columns().to_vec();
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let mut table = match Table::create(
        dir.as_ref(),
        "result.csv",
        "time_s",
        &names,
        opts.keep_rows,
        keep_text,
    ) {
        Ok(t) => t,
        Err(e) => return RunOutput::invalid(label, e),
    };
    let mut texts = Vec::new();
    let mut last_time = None;
    let mut stats_cols = Statistics::new(names.len());

    let started = Stopwatch::start();
    let (stats, result) = simulator.run_with_stats(|t, values| {
        table.row(t, values.iter().copied().map(Some))?;
        stats_cols.add(values);
        last_time = Some(t);
        Ok(())
    });
    // The rows written before a failure stay in result.csv.
    // The text kept in memory comes back after a failure too.
    let finished = table
        .finish()
        .map(|text| texts.extend(text.map(|t| ("result.csv", t))));
    let result = result.and_then(|_| finished.map_err(openkeito_core::SimError::Output));
    let elapsed_s = started.elapsed_s();

    let path_of = |name: &str| dir.as_ref().map(|d| d.display(name));
    let mut warnings = loaded.warnings.clone();
    warnings.extend(stats.warnings.iter().cloned());
    let mut counts: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    for e in &stats.events {
        *counts
            .entry(e.component.as_str())
            .or_default()
            .entry(e.action)
            .or_default() += 1;
    }
    let mut summary = json!({
        "tool": "openkeito",
        "version": env!("CARGO_PKG_VERSION"),
        "model": {"path": label, "name": model.name, "sha256": sha256},
        "simulation": {
            "method": model.simulation.method.as_str(),
            "time_step_s": model.simulation.time_step_s,
            "end_time_s": model.simulation.end_time_s,
            "steps": openkeito_model::step_count(&model.simulation),
            "output_interval_steps": model.simulation.output_interval(),
            "initialization": model.simulation.initialization.as_str(),
            "switching": model.simulation.switching().as_str(),
            "partitioning": model.simulation.partitioning().as_str(),
            // The largest number of threads asked for: by the caller, or by
            // the model. `solver.threads` is the number used.
            "threads": opts.threads.unwrap_or(model.simulation.threads()).max(1),
        },
        "result": {
            "csv_path": path_of("result.csv"),
            "rows": table.written,
            "last_time_s": last_time,
            "time_column": "time_s",
            "columns": columns,
            "statistics": stats_cols.to_json(&names),
        },
        "warnings": warnings,
        "elapsed_s": elapsed_s,
        // What the run counted, also up to a failure: the switching events
        // before a singular matrix are often its cause.
        "solver": {
            "unknowns": stats.unknowns,
            "linear_solver": stats.linear_solver,
            // The parts the circuit was divided into at its travelling-wave
            // lines, each with its own matrix (1: not divided), the unknowns
            // of the largest part, the lines that join parts, and the
            // threads that solved the parts.
            "parts": stats.parts,
            "largest_part_unknowns": stats.largest_part_unknowns,
            "links": stats.links,
            "threads": stats.threads,
            "max_condition_estimate": stats.max_condition_estimate,
            "steps": stats.steps,
            "factorizations": stats.factorizations,
            "solves": stats.solves,
            "resolves": stats.resolves,
            "max_resolves_per_step": stats.max_resolves_per_step,
            "cycle_fallbacks": stats.cycle_fallbacks,
            "operating_point_iterations": stats.operating_point_iterations,
            "max_operating_point_iterations_per_stage": stats.max_operating_point_iterations,
            // With "switching": "interpolated": the instants placed within
            // time steps, the steps solved in parts, and the steps in which
            // the limit of instants was reached (W270).
            "instants_within_steps": stats.instants_within_steps,
            "split_steps": stats.split_steps,
            "instant_limit_steps": stats.instant_limit_steps,
            // The passes over the control blocks of closed loops beyond the
            // first pass of each grid point; 0 without a loop.
            "control_loop_iterations": stats.control_loop_iterations,
        },
        "timing": {
            "build_s": build_s,
            "factorize_s": stats.timing.factorize_s,
            "solve_s": stats.timing.solve_s,
            "output_s": stats.timing.output_s,
            "run_s": stats.timing.total_s,
        },
        "events": &stats.events[..stats.events.len().min(SUMMARY_EVENTS)],
        "event_count": stats.events.len(),
        "events_truncated": stats.events.len() > SUMMARY_EVENTS,
        "event_counts": counts,
    });
    let object = summary.as_object_mut().expect("summary is an object");
    let code = match &result {
        Ok(()) => {
            object.insert("status".into(), json!("completed"));
            object.insert("errors".into(), json!([]));
            0
        }
        Err(e) => {
            object.insert("status".into(), json!("failed"));
            object.insert("errors".into(), json!([e.to_diagnostic()]));
            EXIT_FAILED
        }
    };
    let rows = table.rows.take();
    if let Some(init) = &stats.initialization {
        let mut block = json!(init);
        block["phasors"] = Value::Array(
            init.phasors
                .iter()
                .map(|p| {
                    json!({"name": p.name, "unit": p.unit, "rms": p.value.norm(),
                           "angle_deg": p.value.arg().to_degrees()})
                })
                .collect(),
        );
        if let Some(d) = &dir {
            let path = d.path("phasors.csv");
            if let Err(e) = write_text(Some(&path), |w| write_phasors(w, &init.phasors)) {
                return RunOutput::failed(label, not_writable(&path, "phasors.csv", &e), rows);
            }
            block["phasors_csv_path"] = json!(path.display().to_string());
        } else if keep_text {
            let text = write_text(None, |w| write_phasors(w, &init.phasors));
            texts.extend(
                text.expect("memory is writable")
                    .map(|t| ("phasors.csv", t)),
            );
        }
        object.insert("initialization".into(), block);
    }
    match &dir {
        Some(d) => {
            let path = d.path("events.csv");
            if let Err(e) = write_text(Some(&path), |w| write_events(w, &stats.events)) {
                return RunOutput::failed(label, not_writable(&path, "events.csv", &e), rows);
            }
            object.insert("events_csv_path".into(), json!(path.display().to_string()));
        }
        None => {
            object.insert("all_events".into(), json!(stats.events));
            if keep_text {
                let text = write_text(None, |w| write_events(w, &stats.events));
                texts.extend(text.expect("memory is writable").map(|t| ("events.csv", t)));
            }
        }
    }
    finish(label, dir.as_ref(), summary, code, rows, texts)
}

/// Computes the frequency scan of a loaded model.
pub fn scan_loaded(label: &str, loaded: &Loaded, sha256: &str, opts: &RunOptions) -> RunOutput {
    scan_to(label, loaded, sha256, opts, false)
}

/// `keep_text`: without an output directory, keep scan.csv as text.
fn scan_to(
    label: &str,
    loaded: &Loaded,
    sha256: &str,
    opts: &RunOptions,
    keep_text: bool,
) -> RunOutput {
    let model = &loaded.model;
    if opts.force {
        if let Some(out) = opts.out.as_deref() {
            if let Err(e) = clear_stale_results(out) {
                return RunOutput::invalid(label, e);
            }
        }
    }
    let build_started = Stopwatch::start();
    let scanner = match FrequencyScanner::from_model(model) {
        Ok(s) => s,
        Err(diagnostics) => return RunOutput::invalid(label, diagnostics),
    };
    let build_s = build_started.elapsed_s();
    let dir = match opts
        .out
        .as_deref()
        .map(|out| OutDir::prepare(out, opts.force))
    {
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return RunOutput::invalid(label, e),
        None => None,
    };
    let columns = scanner.columns().to_vec();
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let frequencies = scanner.frequencies().to_vec();
    let mut table = match Table::create(
        dir.as_ref(),
        "scan.csv",
        "frequency_hz",
        &names,
        opts.keep_rows,
        keep_text,
    ) {
        Ok(t) => t,
        Err(e) => return RunOutput::invalid(label, e),
    };
    let mut texts = Vec::new();
    let started = Stopwatch::start();
    let result = scanner.run(|f, values| table.row(f, values.iter().copied()));
    let finished = table
        .finish()
        .map(|text| texts.extend(text.map(|t| ("scan.csv", t))));
    let result = result.and_then(|stats| {
        finished
            .map(|_| stats)
            .map_err(openkeito_core::SimError::Output)
    });
    let elapsed_s = started.elapsed_s();
    let mut summary = json!({
        "tool": "openkeito",
        "version": env!("CARGO_PKG_VERSION"),
        "command": "scan",
        "model": {"path": label, "name": model.name, "sha256": sha256},
        "frequency_scan": {
            "frequencies": frequencies.len(),
            "first_hz": frequencies.first(),
            "last_hz": frequencies.last(),
        },
        "result": {
            "csv_path": dir.as_ref().map(|d| d.display("scan.csv")),
            "rows": table.written,
            "frequency_column": "frequency_hz",
            "columns": columns,
        },
        "warnings": loaded.warnings,
        "elapsed_s": elapsed_s,
    });
    let object = summary.as_object_mut().expect("summary is an object");
    let code = match result {
        Ok(stats) => {
            object.insert("status".into(), json!("completed"));
            object.insert("errors".into(), json!([]));
            object.insert(
                "solver".into(),
                json!({
                    "unknowns": stats.unknowns,
                    "linear_solver": stats.linear_solver,
                    "max_condition_estimate": stats.max_condition_estimate,
                    "factorizations": stats.factorizations,
                    "solves": stats.solves,
                }),
            );
            object.insert(
                "timing".into(),
                json!({
                    "build_s": build_s,
                    "factorize_s": stats.factorize_s,
                    "solve_s": stats.solve_s,
                    "output_s": stats.output_s,
                    "run_s": stats.total_s,
                }),
            );
            object.insert(
                "singular_frequencies_hz".into(),
                json!(stats.singular_frequencies_hz),
            );
            object.insert("warnings".into(), json!(stats.warnings));
            0
        }
        Err(e) => {
            object.insert("status".into(), json!("failed"));
            object.insert("errors".into(), json!([e.to_diagnostic()]));
            EXIT_FAILED
        }
    };
    let rows = table.rows.take();
    finish(label, dir.as_ref(), summary, code, rows, texts)
}

/// `openkeito batch` and its threads: not for hosts without threads and
/// files (the browser).
#[cfg(feature = "native")]
pub mod batch;
