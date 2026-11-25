//! `openkeito batch`: many cases run as one job, in parallel.
//!
//! Every case is independent: its own simulator, its own output directory,
//! no shared state. Running them on several threads therefore gives the same
//! results, bit for bit, as running them one after the other.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use openkeito_core::clock::Stopwatch;
use openkeito_model::{Batch, Diagnostic, Loaded, ModelRef};
use rayon::prelude::*;
use serde_json::{json, Value};

use crate::{
    failed_batch, load_file, load_text, number, request_error, run_loaded, RunOptions, RunOutput,
    EXIT_FAILED, EXIT_INVALID,
};

const SUMMARY_FILE: &str = "batch-summary.json";
const METRICS_FILE: &str = "batch-metrics.csv";

/// One case ready to run: its name, how its model is labelled in the
/// summary, the loaded model (or why it could not be loaded), and the raw
/// model JSON text when readable.
pub struct CaseInput {
    pub name: String,
    pub label: String,
    pub loaded: Result<(Loaded, String), Vec<Diagnostic>>,
    pub model_text: Option<String>,
}

/// Number of threads when none is given: the available parallelism.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// The number of threads a batch runs on: `jobs`, at least 1.
fn thread_count(jobs: Option<usize>) -> usize {
    jobs.unwrap_or_else(default_jobs).max(1)
}

/// Loads the models of the cases of a batch file. Paths are relative to
/// `base`, the directory of the batch file; `label` names the batch file.
/// A model written out in the batch file is read like a model file (its
/// SHA-256 is that of its JSON text in the batch file).
pub fn load_cases(batch: &Batch, base: &Path, label: &str) -> Vec<CaseInput> {
    batch
        .cases
        .iter()
        .map(|c| match &c.model {
            ModelRef::Path(p) => {
                let file = base.join(p);
                let model_text = fs::read_to_string(&file).ok();
                let loaded = match &model_text {
                    Some(text) => load_text(text),
                    None => load_file(&file),
                };
                CaseInput {
                    name: c.name.clone(),
                    label: file.display().to_string(),
                    loaded,
                    model_text,
                }
            }
            ModelRef::Inline(m) => {
                let raw = m.get().to_string();
                CaseInput {
                    name: c.name.clone(),
                    label: format!("{label}#{}", c.name),
                    loaded: load_text(&raw),
                    model_text: Some(raw),
                }
            }
        })
        .collect()
}

fn write_case_model(dir: &Path, model_json: &str, summary: &mut Value) {
    let model_path = dir.join("model.json");
    if let Err(e) = fs::write(&model_path, model_json) {
        let diag = Diagnostic::warning(
            "W300",
            model_path.display().to_string(),
            format!("the case model.json could not be written: {e}"),
            "Check that the case directory is writable; the model remains in the batch file.",
        );
        summary["warnings"]
            .as_array_mut()
            .expect("summary warnings is an array")
            .push(serde_json::to_value(diag).expect("diagnostic serializes"));
    }
}

/// Runs the cases on `jobs` threads. With `out`, each case writes into
/// `out/<name>`. Returns one output per case, in the order of `cases`.
pub fn run_cases(
    cases: &[CaseInput],
    out: Option<&Path>,
    force: bool,
    jobs: usize,
    keep_rows: bool,
) -> Vec<RunOutput> {
    let run_one = |c: &CaseInput| -> RunOutput {
        match &c.loaded {
            Ok((loaded, sha256)) => {
                let case_out = out.map(|o| o.join(&c.name));
                let mut output = run_loaded(
                    &c.label,
                    loaded,
                    sha256,
                    &RunOptions {
                        out: case_out.clone(),
                        force,
                        keep_rows,
                        // The cases share the threads of the batch: a case that
                        // runs beside others solves its parts on one thread.
                        threads: (jobs > 1 && cases.len() > 1).then_some(1),
                    },
                );
                if let (Some(dir), Some(model_json)) = (case_out.as_deref(), &c.model_text) {
                    if dir.is_dir() && output.code != EXIT_INVALID {
                        write_case_model(dir, model_json, &mut output.summary);
                    }
                }
                output
            }
            Err(errors) => RunOutput::invalid(&c.label, errors.clone()),
        }
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .expect("thread pool");
    pool.install(|| cases.par_iter().map(run_one).collect())
}

/// The statuses of a case, in the order of the counts of the summary.
const STATUSES: [&str; 3] = ["completed", "failed", "invalid"];

/// Status of one case from its exit code, as an index into `STATUSES`.
fn status_index(code: u8) -> usize {
    match code {
        0 => 0,
        EXIT_FAILED => 1,
        _ => 2,
    }
}

fn status(code: u8) -> &'static str {
    STATUSES[status_index(code)]
}

/// What the batch summary says about the job itself.
pub struct BatchInfo<'a> {
    /// How the batch is labelled: its file, or "<inline>".
    pub label: &'a str,
    pub name: &'a str,
    pub sha256: &'a str,
    pub out: Option<&'a Path>,
    pub jobs: usize,
    pub elapsed_s: f64,
}

/// The batch summary and its exit code: 0 when every case completed, 1 when
/// any failed or was invalid.
pub fn batch_summary(info: &BatchInfo, cases: &[CaseInput], outputs: &[RunOutput]) -> (Value, u8) {
    let out = info.out;
    let mut counts = [0usize; 3];
    let list: Vec<Value> = cases
        .iter()
        .zip(outputs)
        .map(|(c, o)| {
            counts[status_index(o.code)] += 1;
            let s = &o.summary;
            json!({
                "name": c.name,
                "status": status(o.code),
                "model": c.label,
                "out_dir": out.map(|d| d.join(&c.name).display().to_string()),
                "elapsed_s": s.get("elapsed_s"),
                "warnings": s.get("warnings").and_then(Value::as_array).map_or(0, Vec::len),
                "errors": s.get("errors").cloned().unwrap_or(json!([])),
            })
        })
        .collect();
    let code = if counts[0] == cases.len() {
        0
    } else {
        EXIT_FAILED
    };
    (
        json!({
            "tool": "openkeito",
            "version": env!("CARGO_PKG_VERSION"),
            "command": "batch",
            "batch": {"path": info.label, "name": info.name, "sha256": info.sha256},
            "status": if code == 0 { "completed" } else { "failed" },
            "jobs": info.jobs,
            "counts": {"completed": counts[0], "failed": counts[1], "invalid": counts[2]},
            "cases": list,
            "metrics_csv_path": out.map(|d| d.join(METRICS_FILE).display().to_string()),
            "elapsed_s": info.elapsed_s,
        }),
        code,
    )
}

/// One row per case and recorded column: the statistics of the column.
fn write_metrics(path: &Path, cases: &[CaseInput], outputs: &[RunOutput]) -> std::io::Result<()> {
    let mut w = BufWriter::new(fs::File::create(path)?);
    writeln!(w, "case,status,column,unit,min,max,max_abs,last")?;
    let cell = |v: Option<&Value>| v.and_then(Value::as_f64).map(number).unwrap_or_default();
    for (c, o) in cases.iter().zip(outputs) {
        let result = &o.summary["result"];
        let units: Vec<&str> = result["columns"]
            .as_array()
            .map(|cols| {
                cols.iter()
                    .map(|c| c["unit"].as_str().unwrap_or(""))
                    .collect()
            })
            .unwrap_or_default();
        match result["statistics"].as_array() {
            Some(stats) => {
                for (k, s) in stats.iter().enumerate() {
                    writeln!(
                        w,
                        "{},{},{},{},{},{},{},{}",
                        c.name,
                        status(o.code),
                        s["name"].as_str().unwrap_or(""),
                        units.get(k).copied().unwrap_or(""),
                        cell(s.get("min")),
                        cell(s.get("max")),
                        cell(s.get("max_abs")),
                        cell(s.get("last"))
                    )?;
                }
            }
            None => writeln!(w, "{},{},,,,,,", c.name, status(o.code))?,
        }
    }
    w.flush()
}

/// Runs the cases and writes the files of the batch into `out` (when there
/// is one). Returns the batch summary, its exit code and every case's output.
/// When the batch files cannot be written, the summary says so in `errors`
/// and the code is 1.
fn run_and_report(
    info: BatchInfo,
    cases: &[CaseInput],
    force: bool,
    keep_rows: bool,
) -> (Value, u8, Vec<RunOutput>) {
    let started = Stopwatch::start();
    let outputs = run_cases(cases, info.out, force, info.jobs, keep_rows);
    let info = BatchInfo {
        elapsed_s: started.elapsed_s(),
        ..info
    };
    let (mut summary, mut code) = batch_summary(&info, cases, &outputs);
    if let Some(dir) = info.out {
        let written = write_metrics(&dir.join(METRICS_FILE), cases, &outputs).and_then(|_| {
            let text = serde_json::to_string_pretty(&summary).expect("summary serializes");
            fs::write(dir.join(SUMMARY_FILE), text + "\n")
        });
        if let Err(e) = written {
            summary["status"] = json!("failed");
            summary["errors"] = json!(request_error(
                dir.display().to_string(),
                format!("the batch summary cannot be written: {e}"),
                "Check that the output directory is writable.",
            ));
            code = EXIT_FAILED;
        }
    }
    (summary, code, outputs)
}

/// Creates the output directory of a batch. A batch result that is already
/// there is refused, or with `force` its summary and metrics are removed
/// (directories of cases that the new batch does not have are left alone).
fn prepare_out(out: &Path, force: bool) -> Result<(), Vec<Diagnostic>> {
    let files = [out.join(SUMMARY_FILE), out.join(METRICS_FILE)];
    if !force && files.iter().any(|f| f.exists()) {
        return Err(request_error(
            out.display().to_string(),
            "the output directory already holds a batch result".to_string(),
            "Use a new output directory for every batch, or pass --force to overwrite.",
        ));
    }
    let io = |e: std::io::Error, what: &str| {
        request_error(
            out.display().to_string(),
            format!("the output directory cannot be {what}: {e}"),
            "Check that the output directory is writable.",
        )
    };
    for f in files.iter().filter(|f| f.exists()) {
        fs::remove_file(f).map_err(|e| io(e, "cleared"))?;
    }
    fs::create_dir_all(out).map_err(|e| io(e, "created"))
}

/// Reads a batch file: its text and SHA-256, the batch and its cases.
fn read_batch_file(path: &Path) -> Result<(String, Batch, Vec<CaseInput>), Vec<Diagnostic>> {
    let label = path.display().to_string();
    let text = fs::read_to_string(path).map_err(|e| {
        request_error(
            label.clone(),
            format!("the batch file cannot be read: {e}"),
            "Check the path; the file must be UTF-8 JSON.",
        )
    })?;
    let batch = openkeito_model::load_batch_str(&text)?;
    let base: PathBuf = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let cases = load_cases(&batch, &base, &label);
    Ok((crate::sha256_hex(text.as_bytes()), batch, cases))
}

/// `openkeito batch <file> --out <dir>`.
pub fn run_batch_file(path: &Path, out: &Path, force: bool, jobs: Option<usize>) -> (Value, u8) {
    let label = path.display().to_string();
    let (sha256, batch, cases) = match read_batch_file(path) {
        Ok(b) => b,
        Err(errors) => return (failed_batch(&label, errors), EXIT_INVALID),
    };
    if let Err(errors) = prepare_out(out, force) {
        return (failed_batch(&label, errors), EXIT_INVALID);
    }
    let info = BatchInfo {
        label: &label,
        name: &batch.name,
        sha256: &sha256,
        out: Some(out),
        jobs: thread_count(jobs),
        elapsed_s: 0.0,
    };
    let (summary, code, _) = run_and_report(info, &cases, force, false);
    (summary, code)
}

/// The document of `openkeito validate` for a batch file: the batch file
/// itself and the model of every case. The path of a diagnostic of a model
/// starts with its case.
pub fn validate_batch_report(path: &Path) -> (Value, u8) {
    let label = path.display().to_string();
    let invalid =
        |diagnostics: Vec<Diagnostic>| crate::invalid_document("batch", &label, diagnostics);
    let (sha256, batch, cases) = match read_batch_file(path) {
        Ok(b) => b,
        Err(errors) => return invalid(errors),
    };
    let in_case = |c: &CaseInput, list: &[Diagnostic]| -> Vec<Diagnostic> {
        list.iter()
            .map(|d| Diagnostic {
                path: format!("case \"{}\": {}", c.name, d.path),
                ..d.clone()
            })
            .collect()
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for c in &cases {
        match &c.loaded {
            Ok((loaded, _)) => warnings.extend(in_case(c, &loaded.warnings)),
            Err(e) => errors.extend(in_case(c, e)),
        }
    }
    if !errors.is_empty() {
        // With the warnings of the cases that are valid.
        errors.extend(warnings);
        return invalid(errors);
    }
    (
        json!({
            "tool": "openkeito",
            "version": env!("CARGO_PKG_VERSION"),
            "valid": true,
            "batch": {"path": label, "name": batch.name, "sha256": sha256},
            "cases": cases.len(),
            "errors": [],
            "warnings": warnings,
        }),
        0,
    )
}

/// Runs models given as JSON texts (for the Python package): returns the
/// batch summary, its exit code and every case's output. Case names that
/// cannot be directory names or are not unique (E402) run nothing: the
/// summary is the document of a failed request, without cases.
pub fn run_batch_texts(
    cases: Vec<(String, String)>,
    out: Option<&Path>,
    force: bool,
    jobs: Option<usize>,
    keep_rows: bool,
) -> (Value, u8, Vec<RunOutput>) {
    let errors = openkeito_model::check_case_names(cases.iter().map(|(name, _)| name.as_str()));
    if !errors.is_empty() {
        return (failed_batch("<inline>", errors), EXIT_INVALID, Vec::new());
    }
    let inputs: Vec<CaseInput> = cases
        .into_iter()
        .map(|(name, text)| {
            let loaded = load_text(&text);
            CaseInput {
                label: format!("<inline>#{name}"),
                loaded,
                model_text: Some(text),
                name,
            }
        })
        .collect();
    if let Some(dir) = out {
        if let Err(errors) = prepare_out(dir, force) {
            return (failed_batch("<inline>", errors), EXIT_INVALID, Vec::new());
        }
    }
    let info = BatchInfo {
        label: "<inline>",
        name: "",
        sha256: "",
        out,
        jobs: thread_count(jobs),
        elapsed_s: 0.0,
    };
    run_and_report(info, &inputs, force, keep_rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_case_model_failure_emits_w300_with_hint() {
        let missing_dir = std::env::temp_dir().join("openkeito-missing-batch-case-dir-for-w300");
        let _ = fs::remove_dir_all(&missing_dir);
        let mut summary = json!({ "warnings": [] });
        write_case_model(&missing_dir, "{}", &mut summary);
        let warnings = summary["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["code"], "W300");
        assert_eq!(warnings[0]["severity"], "warning");
        assert!(warnings[0]["message"]
            .as_str()
            .unwrap()
            .contains("model.json"));
        assert!(!warnings[0]["hint"].as_str().unwrap().is_empty());
    }
}
