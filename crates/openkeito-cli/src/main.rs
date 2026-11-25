//! `openkeito`: validate and run models from the command line.
//!
//! Every command prints one JSON document on standard output. Exit codes:
//! 0 success, 1 the simulation failed, 2 the model or the request is invalid.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use openkeito_cli::{
    batch, clear_stale_results, failed, load_file, load_text, run_loaded, scan_loaded,
    validate_report, validate_schedule_report, RunOptions, EXIT_FAILED, EXIT_INVALID,
};
use openkeito_model::{BATCH_FORMAT, SCHEDULE_FORMAT};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(
    name = "openkeito",
    version,
    about = "Open electromagnetic transient simulator"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a model (or a schedule file, or a batch file with the models
    /// of its cases) without running it.
    Validate {
        /// Model, schedule or batch file (JSON).
        model: PathBuf,
    },
    /// Run a model and write result.csv and summary.json.
    Run {
        /// Model file (JSON).
        model: PathBuf,
        /// Output directory. Must not already hold a result unless --force is given.
        #[arg(long)]
        out: PathBuf,
        /// Overwrite an existing result in the output directory.
        #[arg(long)]
        force: bool,
        /// Largest number of threads that solve the parts of a circuit
        /// divided at its travelling-wave lines, instead of
        /// `simulation.threads` of the model.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        threads: Option<u32>,
    },
    /// Compute the impedances of `frequency_scan` and write scan.csv and
    /// summary.json.
    Scan {
        /// Model file (JSON) with a frequency_scan.
        model: PathBuf,
        /// Output directory. Must not already hold a result unless --force is given.
        #[arg(long)]
        out: PathBuf,
        /// Overwrite an existing result in the output directory.
        #[arg(long)]
        force: bool,
    },
    /// Run the cases of a batch file, in parallel, each into its own
    /// directory, and write batch-summary.json and batch-metrics.csv.
    Batch {
        /// Batch file (JSON, format "openkeito-batch").
        batch: PathBuf,
        /// Output directory. Must not already hold a batch result unless --force is given.
        #[arg(long)]
        out: PathBuf,
        /// Number of cases run at the same time. Default: the available parallelism.
        #[arg(long)]
        jobs: Option<usize>,
        /// Overwrite existing results in the output directory.
        #[arg(long)]
        force: bool,
    },
    /// Print the JSON Schema of the model format (or of the batch or the
    /// schedule format).
    Schema {
        /// Print the schema of batch files instead.
        #[arg(long, conflicts_with = "schedule")]
        batch: bool,
        /// Print the schema of schedule files instead.
        #[arg(long)]
        schedule: bool,
    },
    /// Print the supported components with parameters, units and examples.
    Components,
    /// Print the connections of a model: its components with their
    /// terminals and nodes, its buses, its control blocks with the signals
    /// between them, and what each diagnostic is about.
    Topology {
        /// Model file (JSON).
        model: PathBuf,
        /// Also write the document into this directory as topology.json.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Overwrite a topology.json in the output directory.
        #[arg(long)]
        force: bool,
    },
}

fn file_command(
    model: &std::path::Path,
    out: PathBuf,
    force: bool,
    scan: bool,
    threads: Option<u32>,
) -> (Value, u8) {
    let label = model.display().to_string();
    match load_file(model) {
        Ok((loaded, sha256)) => {
            let opts = RunOptions {
                out: Some(out),
                force,
                keep_rows: false,
                threads: threads.map(|t| t as usize),
            };
            let o = if scan {
                scan_loaded(&label, &loaded, &sha256, &opts)
            } else {
                run_loaded(&label, &loaded, &sha256, &opts)
            };
            (o.summary, o.code)
        }
        Err(mut errors) => {
            if force {
                if let Err(io_errors) = clear_stale_results(&out) {
                    errors.extend(io_errors);
                }
            }
            (failed(&label, errors), EXIT_INVALID)
        }
    }
}

/// `validate`: a model, a schedule file or a batch file (by its `format`).
fn validate_file(path: &std::path::Path) -> (Value, u8) {
    let label = path.display().to_string();
    let Ok(Ok(text)) = std::fs::read(path).map(String::from_utf8) else {
        // Not readable or not UTF-8: the model loader says which.
        return validate_report(&label, load_file(path));
    };
    match openkeito_model::document_format(&text).as_deref() {
        Some(SCHEDULE_FORMAT) => validate_schedule_report(&label, &text),
        Some(BATCH_FORMAT) => batch::validate_batch_report(path),
        _ => validate_report(&label, load_text(&text)),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let is_run = matches!(cli.command, Command::Run { .. });
    let (document, code) = match cli.command {
        Command::Validate { model } => validate_file(&model),
        Command::Run {
            model,
            out,
            force,
            threads,
        } => file_command(&model, out, force, false, threads),
        Command::Scan { model, out, force } => file_command(&model, out, force, true, None),
        Command::Batch {
            batch,
            out,
            jobs,
            force,
        } => batch::run_batch_file(&batch, &out, force, jobs),
        Command::Schema { batch, schedule } => {
            if batch {
                (openkeito_model::batch_json_schema(), 0)
            } else if schedule {
                (openkeito_model::schedule_json_schema(), 0)
            } else {
                (openkeito_model::json_schema(), 0)
            }
        }
        Command::Components => (json!(openkeito_model::component_catalog()), 0),
        Command::Topology { model, out, force } => {
            openkeito_cli::topology_file(&model, out.as_deref(), force)
        }
    };
    if is_run && std::env::var_os("OPENKEITO_DETAIL_TIMING").is_some() {
        if let Some(timing) = document.get("timing") {
            if let (Some(solve_s), Some(factorize_s), Some(output_s), Some(run_s)) = (
                timing.get("solve_s").and_then(Value::as_f64),
                timing.get("factorize_s").and_then(Value::as_f64),
                timing.get("output_s").and_then(Value::as_f64),
                timing.get("run_s").and_then(Value::as_f64),
            ) {
                let control_s = openkeito_core::last_detail_control_s();
                eprintln!(
                    "OPENKEITO_DETAIL_TIMING control_s={control_s:.6} solve_s={solve_s:.6} factorize_s={factorize_s:.6} output_s={output_s:.6} run_s={run_s:.6}"
                );
            }
        }
    }
    let text = serde_json::to_string_pretty(&document).expect("JSON document serializes");
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = writeln!(stdout, "{text}").and_then(|()| stdout.flush()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            return ExitCode::from(code);
        }
        eprintln!("openkeito: failed to write to standard output: {e}");
        return ExitCode::from(EXIT_FAILED);
    }
    ExitCode::from(code)
}
