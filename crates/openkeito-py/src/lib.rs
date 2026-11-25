//! `openkeito._native`: the Rust core seen from Python.
//!
//! Models go in as JSON text and summaries come back as JSON text, so that
//! Python sees exactly the documents of the `openkeito` command. Recorded
//! values come back as numpy arrays. The Python package `openkeito` wraps
//! these functions (python/openkeito/__init__.py).

use std::collections::HashMap;
use std::path::PathBuf;

use numpy::{PyArray1, PyArray2};
use openkeito_cli::{
    batch, clear_stale_results, load_text, request_error, run_loaded, run_loaded_with_controller,
    scan_loaded, validate_report, RunOptions, RunOutput,
};
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};

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

struct PyControllerCache {
    col_keys: Vec<Py<PyString>>,
    sig_keys: Vec<Py<PyString>>,
    sig_index: HashMap<String, usize>,
    mapping_type: Py<PyAny>,
}

impl PyControllerCache {
    fn new(py: Python<'_>, col_names: &[String], signal_names: &[String]) -> PyResult<Self> {
        let col_keys = col_names
            .iter()
            .map(|n| PyString::new(py, n).unbind())
            .collect();
        let sig_keys = signal_names
            .iter()
            .map(|n| PyString::new(py, n).unbind())
            .collect();
        let sig_index = signal_names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        let mapping_type = py.import("collections.abc")?.getattr("Mapping")?.unbind();
        Ok(PyControllerCache {
            col_keys,
            sig_keys,
            sig_index,
            mapping_type,
        })
    }
}

struct PyControllerRunner {
    controller: Py<PyAny>,
    cache: Option<PyControllerCache>,
    fatal_err: Option<PyErr>,
}

impl PyControllerRunner {
    fn invoke(
        &mut self,
        col_names: &[String],
        signal_names: &[String],
        t: f64,
        columns: &[f64],
        signals: &[f64],
        updates: &mut Vec<(usize, f64)>,
    ) -> Result<(), String> {
        let res: PyResult<()> = Python::attach(|py| {
            if self.cache.is_none() {
                self.cache = Some(PyControllerCache::new(py, col_names, signal_names)?);
            }
            let c = self.cache.as_ref().expect("initialized above");
            let state = PyDict::new(py);
            for (key, &val) in c.col_keys.iter().zip(columns) {
                state.set_item(key.bind(py), val)?;
            }
            for (key, &val) in c.sig_keys.iter().zip(signals) {
                state.set_item(key.bind(py), val)?;
            }
            let result = self.controller.bind(py).call1((t, &state))?;
            py.check_signals()?;
            if result.is_none() {
                return Ok(());
            }
            let mut insert_pair = |k: Bound<'_, PyAny>, v: Bound<'_, PyAny>| -> PyResult<()> {
                let name: String = k.extract().map_err(|_| {
                    PyValueError::new_err("controller returned a non-string signal name")
                })?;
                let &sig_idx = c.sig_index.get(&name).ok_or_else(|| {
                    PyValueError::new_err(format!("unknown control signal \"{name}\""))
                })?;
                let val: f64 = v.extract().map_err(|_| {
                    PyValueError::new_err(format!(
                        "controller value for signal \"{name}\" cannot be converted to float"
                    ))
                })?;
                if !val.is_finite() {
                    return Err(PyValueError::new_err(format!(
                        "controller returned non-finite value ({val}) for signal \"{name}\""
                    )));
                }
                updates.push((sig_idx, val));
                Ok(())
            };
            if let Ok(dict) = result.cast::<PyDict>() {
                for (k, v) in dict.iter() {
                    insert_pair(k, v)?;
                }
                return Ok(());
            }
            if result.is_instance(c.mapping_type.bind(py))? {
                let items = result.call_method0("items")?;
                for item in items.try_iter()? {
                    let (k, v): (Bound<'_, PyAny>, Bound<'_, PyAny>) = item?.extract()?;
                    insert_pair(k, v)?;
                }
                return Ok(());
            }
            let ty = result.get_type().name()?;
            Err(PyValueError::new_err(format!(
                "controller must return a mapping or None, got {ty}"
            )))
        });
        match res {
            Ok(()) => Ok(()),
            Err(err) => {
                // Propagate non-Exception BaseExceptions (KeyboardInterrupt,
                // SystemExit, GeneratorExit) to Python instead of wrapping them
                // as E252 and swallowing them when check=False.
                let is_fatal = Python::attach(|py| !err.is_instance_of::<PyException>(py));
                if is_fatal {
                    self.fatal_err = Some(err);
                    return Err("simulation stopped by Python interrupt or exit".to_string());
                }
                Err(err.to_string())
            }
        }
    }
}

/// Runs a model (JSON text) with a Python controller callback.
#[pyfunction]
#[pyo3(signature = (model, controller, out=None, force=false, threads=None, control_period_s=None, control_interval_steps=None))]
#[allow(clippy::too_many_arguments)]
fn run_with_controller<'py>(
    py: Python<'py>,
    model: &str,
    controller: Py<PyAny>,
    out: Option<PathBuf>,
    force: bool,
    threads: Option<usize>,
    control_period_s: Option<f64>,
    control_interval_steps: Option<u64>,
) -> PyResult<Bound<'py, PyDict>> {
    if !controller.bind(py).is_callable() {
        return Err(PyValueError::new_err("controller must be callable"));
    }
    if control_period_s.is_some() && control_interval_steps.is_some() {
        return Err(PyValueError::new_err(
            "pass at most one of control_period_s and control_interval_steps",
        ));
    }
    if let Some(steps) = control_interval_steps {
        if steps < 1 {
            return Err(PyValueError::new_err("control_interval_steps must be >= 1"));
        }
    }
    if let Some(period) = control_period_s {
        if !period.is_finite() || period <= 0.0 {
            return Err(PyValueError::new_err(
                "control_period_s must be a finite positive number",
            ));
        }
    }

    let model = model.to_string();
    let (output, runner) = py.detach(move || -> PyResult<(RunOutput, Option<PyControllerRunner>)> {
        match load_text(&model) {
            Ok((loaded, sha256)) => {
                let interval_steps = if let Some(steps) = control_interval_steps {
                    steps
                } else if let Some(period) = control_period_s {
                    let h = loaded.model.simulation.time_step_s;
                    let steps_f = (period / h).round();
                    if !steps_f.is_finite()
                        || steps_f < 1.0
                        || (period - steps_f * h).abs() > 1e-6 * period
                    {
                        return Err(PyValueError::new_err(format!(
                            "control_period_s ({period}) must be a positive integer multiple of simulation.time_step_s ({h})"
                        )));
                    }
                    steps_f as u64
                } else {
                    1
                };
                let opts = RunOptions {
                    out,
                    force,
                    keep_rows: true,
                    threads,
                };
                let mut runner = PyControllerRunner {
                    controller,
                    cache: None,
                    fatal_err: None,
                };
                let out = run_loaded_with_controller(
                    "<inline>",
                    &loaded,
                    &sha256,
                    &opts,
                    interval_steps,
                    |col_names, signal_names, t, columns, signals, updates| {
                        runner.invoke(col_names, signal_names, t, columns, signals, updates)
                    },
                );
                Ok((out, Some(runner)))
            }
            Err(mut errors) => {
                if force {
                    if let Some(dir) = out.as_deref() {
                        if let Err(io_errors) = clear_stale_results(dir) {
                            errors.extend(io_errors);
                        }
                    }
                }
                Ok((RunOutput::invalid("<inline>", errors), None))
            }
        }
    })?;
    if let Some(mut runner) = runner {
        if let Some(err) = runner.fatal_err.take() {
            return Err(err);
        }
    }
    py.check_signals()?;
    to_dict(py, output)
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
    m.add_function(wrap_pyfunction!(run_with_controller, m)?)?;
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    m.add_function(wrap_pyfunction!(run_batch, m)?)?;
    Ok(())
}
