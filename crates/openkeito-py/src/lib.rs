//! `openkeito._native`: the Rust core seen from Python.
//!
//! Models go in as JSON text and summaries come back as JSON text, so that
//! Python sees exactly the documents of the `openkeito` command. Recorded
//! values come back as numpy arrays. The Python package `openkeito` wraps
//! these functions (python/openkeito/__init__.py).

use std::path::PathBuf;

use numpy::{PyArray1, PyArray2};
use openkeito_cli::{
    batch, clear_stale_results, load_text, request_error, run_loaded, scan_loaded, validate_report,
    RunOptions, RunOutput,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// The software version.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The document of `openkeito validate` for a model given as JSON text.
#[pyfunction]
fn validate(model: &str) -> String {
    if openkeito_model::document_format(model).as_deref() == Some(openkeito_model::SCHEDULE_FORMAT)
    {
        return validate_report(
            "<inline>",
            Err(request_error(
                "format",
                "this is a schedule file (format \"openkeito-schedule\"), not a model".to_string(),
                "Use `validate_schedule` to validate a schedule file.",
            )),
        )
        .0
        .to_string();
    }
    validate_report("<inline>", load_text(model)).0.to_string()
}

/// The document of `openkeito validate` for a schedule given as JSON text.
#[pyfunction]
fn validate_schedule(schedule: &str) -> String {
    openkeito_cli::validate_schedule_report("<inline>", schedule)
        .0
        .to_string()
}

/// The document of `openkeito topology` for a model given as JSON text.
#[pyfunction]
fn topology(model: &str) -> String {
    openkeito_cli::topology_report(model).0.to_string()
}

/// Where the connections of `topology` differ from the circuit that the
/// solver builds of the model (JSON text): a list of sentences, empty when
/// they agree. Raises ValueError for a model that is not valid.
#[pyfunction]
fn topology_mismatches(model: &str) -> PyResult<Vec<String>> {
    match load_text(model) {
        Ok((loaded, _)) => Ok(openkeito_cli::topology_mismatches(&loaded.model)),
        Err(errors) => Err(pyo3::exceptions::PyValueError::new_err(
            errors
                .iter()
                .map(|d| format!("{} at {}: {}", d.code, d.path, d.message))
                .collect::<Vec<_>>()
                .join("; "),
        )),
    }
}

/// JSON Schema of the model format, or of the batch or schedule format.
#[pyfunction]
#[pyo3(signature = (batch=false, schedule=false))]
fn schema(batch: bool, schedule: bool) -> String {
    if batch {
        openkeito_model::batch_json_schema().to_string()
    } else if schedule {
        openkeito_model::schedule_json_schema().to_string()
    } else {
        openkeito_model::json_schema().to_string()
    }
}

/// The component catalog as JSON text.
#[pyfunction]
fn components() -> String {
    serde_json::to_string(&openkeito_model::component_catalog()).expect("catalog serializes")
}

/// A run's output as a dict: "summary" (JSON text), "code", and with rows
/// "axis" (1-D array), "names" (list) and "values" (2-D array, one row per
/// column).
fn to_dict<'py>(py: Python<'py>, o: RunOutput) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("summary", o.summary.to_string())?;
    d.set_item("code", o.code)?;
    if let Some(rows) = o.rows {
        d.set_item("axis", PyArray1::from_vec(py, rows.axis))?;
        d.set_item("names", PyList::new(py, rows.names)?)?;
        let values = if rows.values.is_empty() {
            PyArray2::zeros(py, [0, 0], false)
        } else {
            PyArray2::from_vec2(py, &rows.values)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?
        };
        d.set_item("values", values)?;
    }
    Ok(d)
}

fn run_or_scan<'py>(
    py: Python<'py>,
    model: &str,
    out: Option<PathBuf>,
    force: bool,
    threads: Option<usize>,
    scan: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let model = model.to_string();
    // The run does not touch Python objects: let other Python threads run.
    let output = py.detach(move || match load_text(&model) {
        Ok((loaded, sha256)) => {
            let opts = RunOptions {
                out,
                force,
                keep_rows: true,
                threads,
            };
            if scan {
                scan_loaded("<inline>", &loaded, &sha256, &opts)
            } else {
                run_loaded("<inline>", &loaded, &sha256, &opts)
            }
        }
        Err(mut errors) => {
            if force {
                if let Some(dir) = out.as_deref() {
                    if let Err(io_errors) = clear_stale_results(dir) {
                        errors.extend(io_errors);
                    }
                }
            }
            RunOutput::invalid("<inline>", errors)
        }
    });
    to_dict(py, output)
}

/// Runs a model (JSON text). With `out`, also writes the files of
/// `openkeito run` there.
#[pyfunction]
#[pyo3(signature = (model, out=None, force=false, threads=None))]
fn run<'py>(
    py: Python<'py>,
    model: &str,
    out: Option<PathBuf>,
    force: bool,
    threads: Option<usize>,
) -> PyResult<Bound<'py, PyDict>> {
    run_or_scan(py, model, out, force, threads, false)
}

/// Computes the frequency scan of a model (JSON text). With `out`, also
/// writes the files of `openkeito scan` there.
#[pyfunction]
#[pyo3(signature = (model, out=None, force=false))]
fn scan<'py>(
    py: Python<'py>,
    model: &str,
    out: Option<PathBuf>,
    force: bool,
) -> PyResult<Bound<'py, PyDict>> {
    run_or_scan(py, model, out, force, None, true)
}

/// Runs cases [(name, model JSON text)] on `jobs` threads. Returns a dict
/// with "summary" (the batch summary, JSON text), "code" and "cases" (one
/// run output dict per case, with rows when `keep_rows`). Case names that
/// cannot be directory names or are not unique run nothing: "cases" is
/// empty and the summary holds the errors.
#[pyfunction]
#[pyo3(signature = (cases, out=None, jobs=None, force=false, keep_rows=false))]
fn run_batch<'py>(
    py: Python<'py>,
    cases: Vec<(String, String)>,
    out: Option<PathBuf>,
    jobs: Option<usize>,
    force: bool,
    keep_rows: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let (summary, code, outputs) =
        py.detach(move || batch::run_batch_texts(cases, out.as_deref(), force, jobs, keep_rows));
    let d = PyDict::new(py);
    d.set_item("summary", summary.to_string())?;
    d.set_item("code", code)?;
    let list = PyList::empty(py);
    for o in outputs {
        list.append(to_dict(py, o)?)?;
    }
    d.set_item("cases", list)?;
    Ok(d)
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(validate_schedule, m)?)?;
    m.add_function(wrap_pyfunction!(topology, m)?)?;
    m.add_function(wrap_pyfunction!(topology_mismatches, m)?)?;
    m.add_function(wrap_pyfunction!(schema, m)?)?;
    m.add_function(wrap_pyfunction!(components, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    m.add_function(wrap_pyfunction!(run_batch, m)?)?;
    Ok(())
}
