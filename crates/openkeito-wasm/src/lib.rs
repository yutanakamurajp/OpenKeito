//! OpenKeito for the browser: the commands `validate`, `run`, `scan`,
//! `topology`, `schema` and `components` of `openkeito`, without files.
//!
//! A model comes in as JSON text. Every function returns what the command
//! prints (a JSON document as text); `run` and `scan` also return the result
//! files as text, byte for byte what the command writes into its output
//! directory. The work is done by the library behind the command
//! (`openkeito-cli` without its feature `native`): nothing is implemented
//! twice.
//!
//! One thread: WebAssembly in a page has no threads of its own, so a run
//! solves the parts of a divided circuit one after the other (the results do
//! not depend on the number of threads). `batch` is not offered.

use openkeito_cli::{load_text, run_texts, scan_texts, validate_report, RunOutput};
use openkeito_model::{Diagnostic, BATCH_FORMAT, SCHEDULE_FORMAT};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

/// The label of a model without a name of its own, as in the Python package.
const INLINE: &str = "<inline>";

fn text_of(document: &Value) -> String {
    serde_json::to_string_pretty(document).expect("JSON document serializes")
}

fn label_of(label: &Option<String>) -> &str {
    label.as_deref().unwrap_or(INLINE)
}

/// The version of OpenKeito, as `openkeito --version` and `version` of the
/// summaries report it.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// `openkeito schema`: the JSON Schema of the model format.
#[wasm_bindgen]
pub fn schema() -> String {
    text_of(&openkeito_model::json_schema())
}

/// `openkeito schema --schedule`: the JSON Schema of schedule files.
#[wasm_bindgen(js_name = scheduleSchema)]
pub fn schedule_schema() -> String {
    text_of(&openkeito_model::schedule_json_schema())
}

/// `openkeito components`: the supported components with parameters, units
/// and examples.
#[wasm_bindgen]
pub fn components() -> String {
    text_of(&json!(openkeito_model::component_catalog()))
}

/// A batch file names its models by path and runs them on several threads:
/// neither exists here.
fn batch_not_supported() -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        "E600",
        "format",
        "batch files are not supported in the browser: there are no files to read the models of \
         the cases from and no threads to run them on"
            .to_string(),
        "Pass the model of one case at a time, or use `openkeito batch` on the command line or \
         `openkeito.run_batch` in Python.",
    )]
}

/// A schedule file is validated or turned into models by the schedule tools,
/// not run directly.
fn schedule_not_supported() -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        "E600",
        "format",
        "this is a schedule file (format \"openkeito-schedule\"), not a model".to_string(),
        "Validate schedule files with `validate`, or convert them to models with the Python \
         `openkeito.schedule` / `openkeito.transition` tools.",
    )]
}

fn load_model_for_wasm(text: &str) -> Result<(openkeito_model::Loaded, String), Vec<Diagnostic>> {
    match openkeito_model::document_format(text).as_deref() {
        Some(BATCH_FORMAT) => Err(batch_not_supported()),
        Some(SCHEDULE_FORMAT) => Err(schedule_not_supported()),
        _ => load_text(text),
    }
}

/// `openkeito validate`: checks a model or a schedule file (by its `format`)
/// without running it. Returns the document of the command; `valid` says
/// whether there are errors. `label` names the document in the report
/// (default "<inline>").
#[wasm_bindgen]
pub fn validate(text: &str, label: Option<String>) -> String {
    let label = label_of(&label);
    let (document, _code) = match openkeito_model::document_format(text).as_deref() {
        Some(SCHEDULE_FORMAT) => openkeito_cli::validate_schedule_report(label, text),
        Some(BATCH_FORMAT) => validate_report(label, Err(batch_not_supported())),
        _ => validate_report(label, load_text(text)),
    };
    text_of(&document)
}

/// `openkeito topology`: the connections of a model as a document, for a
/// diagram: its components with their terminals and nodes, its buses, its
/// control blocks with the signals between them, and what each diagnostic
/// is about. A model that can be read has them also when it is not valid
/// (`valid` says so); a text that cannot be read has only `errors`. The
/// document is the one the command prints for the same text.
#[wasm_bindgen]
pub fn topology(text: &str) -> String {
    text_of(&openkeito_cli::topology_report(text).0)
}

/// What `run` and `scan` return: the summary and the result files.
#[wasm_bindgen]
pub struct Output {
    summary: String,
    code: u8,
    texts: Vec<(&'static str, String)>,
}

#[wasm_bindgen]
impl Output {
    /// The summary (the content of `summary.json`) as JSON text. Read
    /// `status`, then `errors`, `events` and `warnings`, before the CSV.
    #[wasm_bindgen(getter)]
    pub fn summary(&self) -> String {
        self.summary.clone()
    }

    /// The exit code of the command: 0 completed, 1 the simulation failed, 2
    /// the model is invalid.
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> u8 {
        self.code
    }

    /// The names of the result files there are: "result.csv" and
    /// "events.csv" (and "phasors.csv" with a steady-state initialization)
    /// of a run, "scan.csv" of a scan; none when the model is invalid.
    #[wasm_bindgen(getter)]
    pub fn files(&self) -> Vec<String> {
        self.texts.iter().map(|(n, _)| n.to_string()).collect()
    }

    /// The text of the result file `name`, byte for byte what the command
    /// writes into its output directory; undefined when there is none.
    pub fn file(&self, name: &str) -> Option<String> {
        self.texts
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| t.clone())
    }
}

impl From<RunOutput> for Output {
    fn from(o: RunOutput) -> Output {
        Output {
            summary: text_of(&o.summary),
            code: o.code,
            texts: o.texts,
        }
    }
}

/// `openkeito run`: validates and runs a model. The rows computed before a
/// failure stay in "result.csv".
#[wasm_bindgen]
pub fn run(text: &str, label: Option<String>) -> Output {
    let label = label_of(&label);
    match load_model_for_wasm(text) {
        // One thread, whatever `simulation.threads` asks for.
        Ok((loaded, sha256)) => run_texts(label, &loaded, &sha256, Some(1)),
        Err(errors) => RunOutput::invalid(label, errors),
    }
    .into()
}

/// `openkeito scan`: computes the impedances of `frequency_scan` of a model.
#[wasm_bindgen]
pub fn scan(text: &str, label: Option<String>) -> Output {
    let label = label_of(&label);
    match load_model_for_wasm(text) {
        Ok((loaded, sha256)) => scan_texts(label, &loaded, &sha256),
        Err(errors) => RunOutput::invalid(label, errors),
    }
    .into()
}

/// Sets the clock behind `timing` and `elapsed_s` of the summaries: a
/// function without arguments that returns milliseconds and never goes back,
/// such as `() => performance.now()`. Without an argument (or with null) the
/// clock is taken away and every time in the summaries is 0.
///
/// When the module starts, `performance.now()` of the host is the clock (if
/// the host has it), so this is only needed for another clock or for none;
/// `resetClock` goes back to it. No result depends on the clock.
///
/// A run reads the clock twice per output row. Reading `performance.now()`
/// takes some 0.05 to 0.1 microseconds, a clock given here several times
/// that (the call goes through JavaScript glue): a host that wants the
/// shortest computing time and no timing takes the clock away.
#[wasm_bindgen(js_name = setClock)]
pub fn set_clock(milliseconds: Option<js_sys::Function>) {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    host::set_clock(milliseconds);
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let _ = milliseconds; // Everywhere else the clock is `std::time::Instant`.
}

/// Goes back to the clock the module starts with: `performance.now()` of the
/// host, or none when the host does not have it.
#[wasm_bindgen(js_name = resetClock)]
pub fn reset_clock() {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    host::reset_clock();
}

/// What only exists in a JavaScript host: its clock and its console.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod host {
    use std::cell::RefCell;

    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// `performance.now()`: milliseconds since the page (or the
        /// process) started.
        #[wasm_bindgen(js_namespace = performance, js_name = now)]
        fn performance_now() -> f64;

        #[wasm_bindgen(js_namespace = console, js_name = error)]
        fn console_error(message: &str);
    }

    thread_local! {
        /// The clock given with `setClock`.
        static GIVEN: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
    }

    fn performance_seconds() -> f64 {
        performance_now() / 1000.0
    }

    /// A clock that fails or returns something else than a number reads 0.
    fn given_seconds() -> f64 {
        GIVEN.with(|g| {
            g.borrow()
                .as_ref()
                .and_then(|f| f.call0(&JsValue::NULL).ok())
                .and_then(|v| v.as_f64())
                .map_or(0.0, |ms| ms / 1000.0)
        })
    }

    pub fn set_clock(milliseconds: Option<js_sys::Function>) {
        let clock = milliseconds
            .is_some()
            .then_some(given_seconds as fn() -> f64);
        GIVEN.with(|g| *g.borrow_mut() = milliseconds);
        openkeito_core::clock::set_clock(clock);
    }

    pub fn reset_clock() {
        GIVEN.with(|g| *g.borrow_mut() = None);
        let clock = has_performance_now().then_some(performance_seconds as fn() -> f64);
        openkeito_core::clock::set_clock(clock);
    }

    /// Whether the host has `performance.now`.
    fn has_performance_now() -> bool {
        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("performance"))
            .ok()
            .filter(|p| p.is_object())
            .and_then(|p| js_sys::Reflect::get(&p, &JsValue::from_str("now")).ok())
            .is_some_and(|now| now.is_function())
    }

    /// Runs when the module is instantiated.
    #[wasm_bindgen(start)]
    fn start() {
        // A panic ends in the trap "unreachable" without a word: say first
        // what happened and where.
        std::panic::set_hook(Box::new(|info| {
            console_error(&format!("openkeito: {info}"));
        }));
        reset_clock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RLC: &str = include_str!("../../../examples/rlc.json");
    const FAULT: &str = include_str!("../../../examples/three_phase_fault.json");

    fn json_of(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    #[test]
    fn run_returns_the_summary_and_the_files_as_text() {
        let out = run(RLC, None);
        let summary = json_of(&out.summary());
        assert_eq!(out.code(), 0, "{summary}");
        assert_eq!(summary["status"], "completed");
        assert_eq!(summary["model"]["path"], INLINE);
        assert_eq!(summary["simulation"]["threads"], 1);
        assert_eq!(out.files(), ["result.csv", "events.csv"]);
        let csv = out.file("result.csv").unwrap();
        assert!(csv.starts_with("time_s,"));
        assert_eq!(
            csv.lines().count() as u64,
            summary["result"]["rows"].as_u64().unwrap() + 1
        );
        assert_eq!(
            out.file("events.csv").unwrap().lines().next(),
            Some("time_s,component,action,interrupted_current_a")
        );
        assert_eq!(out.file("scan.csv"), None);
        // The parts of the timing are there (on the build machine from
        // `Instant`; in the browser from the clock of the host).
        for part in ["build_s", "factorize_s", "solve_s", "output_s", "run_s"] {
            assert!(summary["timing"][part].is_number(), "{part}");
        }
    }

    #[test]
    fn an_invalid_model_has_diagnostics_and_no_files() {
        let out = run(r#"{"format": "openkeito-model"}"#, Some("m.json".into()));
        let summary = json_of(&out.summary());
        assert_eq!(out.code(), 2);
        assert_eq!(summary["status"], "failed");
        assert_eq!(summary["model"]["path"], "m.json");
        assert!(!summary["errors"].as_array().unwrap().is_empty());
        assert!(out.files().is_empty());

        let report = json_of(&validate("not JSON", None));
        assert_eq!(report["valid"], false);
        assert!(report["errors"][0]["hint"].is_string());
        // Errors and warnings are apart, also when the model is not valid.
        assert!(summary["warnings"].is_array() && report["warnings"].is_array());
        for document in [&summary, &report] {
            let errors = document["errors"].as_array().unwrap();
            assert!(errors.iter().all(|d| d["severity"] == "error"));
        }
    }

    #[test]
    fn validate_tells_models_schedules_and_batches_apart() {
        let report = json_of(&validate(RLC, None));
        assert_eq!(report["valid"], true, "{report}");
        assert!(report["steps"].as_u64().unwrap() > 0);

        let schedule = r#"{"format": "openkeito-schedule", "version": "0.5"}"#;
        let report = json_of(&validate(schedule, None));
        assert!(report.get("schedule").is_some(), "{report}");

        let batch = r#"{"format": "openkeito-batch", "version": "0.5", "cases": []}"#;
        let report = json_of(&validate(batch, None));
        assert_eq!(report["valid"], false);
        assert_eq!(report["errors"][0]["code"], "E600");
        assert!(report["errors"][0]["hint"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn scan_returns_scan_csv() {
        let out = scan(FAULT, None);
        let summary = json_of(&out.summary());
        assert_eq!(out.code(), 0, "{summary}");
        assert_eq!(summary["status"], "completed");
        assert_eq!(out.files(), ["scan.csv"]);
        let csv = out.file("scan.csv").unwrap();
        assert!(csv.starts_with("frequency_hz,"));
        assert_eq!(
            csv.lines().count() as u64,
            summary["result"]["rows"].as_u64().unwrap() + 1
        );
        for part in ["build_s", "factorize_s", "solve_s", "output_s", "run_s"] {
            assert!(summary["timing"][part].is_number(), "{part}");
        }

        // A model without `frequency_scan`: diagnostics, no file.
        let out = scan(RLC, None);
        assert_eq!(out.code(), 2);
        assert!(out.files().is_empty());
        assert!(json_of(&out.summary())["errors"][0]["hint"].is_string());
    }

    /// A run started from the steady state also returns phasors.csv.
    #[test]
    fn a_run_from_the_steady_state_returns_the_phasors() {
        let out = run(FAULT, None);
        assert_eq!(out.code(), 0, "{}", out.summary());
        assert_eq!(out.files(), ["result.csv", "phasors.csv", "events.csv"]);
        assert!(out.file("phasors.csv").unwrap().starts_with(
            "quantity,unit,rms,angle_deg,real,imag
"
        ));
    }

    #[test]
    fn topology_is_the_document_of_the_command() {
        for text in [RLC, FAULT, "{ not JSON"] {
            let document = json_of(&topology(text));
            assert_eq!(document, openkeito_cli::topology_report(text).0);
            assert_eq!(document["command"], "topology");
        }
        assert_eq!(json_of(&topology(RLC))["components"][0]["name"], "E1");
        assert_eq!(json_of(&topology("{ not JSON"))["valid"], false);
    }

    #[test]
    fn schema_and_components_are_json() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(json_of(&schema()).is_object());
        assert!(json_of(&schedule_schema()).is_object());
        assert!(!json_of(&components()).as_array().unwrap().is_empty());
    }

    #[test]
    fn run_and_scan_refuse_batch_and_schedule_with_e600() {
        let batch = r#"{"format": "openkeito-batch", "version": "0.1", "cases": []}"#;
        let schedule =
            r#"{"format": "openkeito-schedule", "version": "0.1", "times": [], "series": []}"#;
        for text in [batch, schedule] {
            for out in [run(text, None), scan(text, None)] {
                let summary = json_of(&out.summary());
                assert_eq!(out.code(), 2, "{summary}");
                assert_eq!(summary["errors"][0]["code"], "E600", "{summary}");
                assert!(out.files().is_empty());
            }
        }
    }
}
