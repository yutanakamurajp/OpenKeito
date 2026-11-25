//! The dense and the sparse LU give the same runs.
//!
//! Every example is run with `"linear_solver": "dense"` and `"sparse"`. The
//! two factorizations order the eliminations differently, so the results
//! differ by rounding. The rounding error of a solution is about kappa eps of
//! its largest entries, with kappa the condition estimate of the matrix
//! (eps = 2.2e-16); a diode bridge with 1 mohm and 1 Mohm states has
//! kappa = 1e9, a well-conditioned RLC circuit about 1. Over N steps of a
//! stable circuit the rounding of each step adds up at most linearly, so the
//! bound is (100 + N) kappa eps of the peak of each column. This bound is
//! loose for ill-conditioned circuits; the sharp check there is that the
//! switching instants decided by the solution (diodes, current zeros) are
//! the same.

mod common;

use std::path::Path;

use common::{run_sim, Run};
use openkeito_core::{FrequencyScanner, RunStats, Simulator};
use serde_json::Value;

fn example(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn with_solver(model: &Value, solver: &str) -> openkeito_model::Model {
    let mut m = model.clone();
    m["version"] = "0.3".into();
    m["simulation"]["linear_solver"] = solver.into();
    openkeito_model::load_str(&m.to_string())
        .unwrap_or_else(|e| panic!("{e:#?}"))
        .model
}

fn run(model: &openkeito_model::Model) -> Run {
    let sim = Simulator::from_model(model).unwrap();
    run_sim(sim, model.simulation.time_step_s)
}

/// Largest difference of a column relative to the peak of that column.
fn largest_difference(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    assert_eq!(a.len(), b.len());
    let columns = a[0].len();
    (0..columns)
        .map(|k| {
            let peak = a.iter().fold(0.0_f64, |m, r| m.max(r[k].abs()));
            let diff = a
                .iter()
                .zip(b)
                .fold(0.0_f64, |m, (x, y)| m.max((x[k] - y[k]).abs()));
            if peak > 0.0 {
                diff / peak
            } else {
                diff
            }
        })
        .fold(0.0, f64::max)
}

#[test]
fn dense_and_sparse_runs_agree() {
    for name in [
        "rlc.json",
        "rl_breaker.json",
        "buck.json",
        "buck_pi.json",
        "rectifier.json",
        "three_phase_fault.json",
    ] {
        let m = example(name);
        let dense = run(&with_solver(&m, "dense"));
        let sparse = run(&with_solver(&m, "sparse"));
        assert_eq!(dense.stats.linear_solver, "dense");
        assert_eq!(sparse.stats.linear_solver, "sparse");
        let diff = largest_difference(&dense.rows, &sparse.rows);
        let kappa = dense
            .stats
            .max_condition_estimate
            .max(sparse.stats.max_condition_estimate);
        let bound = (100.0 + dense.stats.steps as f64) * kappa * f64::EPSILON;
        eprintln!(
            "{name}: {} unknowns, {} rows, condition estimate {kappa:.1e}, largest difference \
             {diff:.2e} of the column peak (bound {bound:.1e})",
            dense.stats.unknowns,
            dense.rows.len()
        );
        assert!(diff < bound, "{name}: {diff:e}");
        let events = |s: &RunStats| -> Vec<(f64, String, &'static str)> {
            s.events
                .iter()
                .map(|e| (e.time_s, e.component.clone(), e.action))
                .collect()
        };
        assert_eq!(events(&dense.stats), events(&sparse.stats), "{name}");
    }
}

#[test]
fn auto_takes_the_dense_solver_for_small_circuits_only() {
    // Measured (tests/benchmark.rs): the sparse LU is faster per step from
    // about 12 unknowns on; "auto" switches at AUTO_SPARSE_UNKNOWNS = 16.
    for (name, solver) in [("rlc.json", "dense"), ("three_phase_fault.json", "sparse")] {
        let stats = run(&with_solver(&example(name), "auto")).stats;
        assert_eq!(
            stats.linear_solver, solver,
            "{name}: {} unknowns",
            stats.unknowns
        );
        assert_eq!(
            stats.unknowns >= openkeito_model::AUTO_SPARSE_UNKNOWNS,
            solver == "sparse"
        );
    }
}

#[test]
fn dense_and_sparse_scans_agree() {
    let m = example("three_phase_fault.json");
    let scan = |solver: &str| {
        let mut rows: Vec<Vec<Option<f64>>> = Vec::new();
        let stats = FrequencyScanner::from_model(&with_solver(&m, solver))
            .unwrap()
            .run(|_, v| {
                rows.push(v.to_vec());
                Ok(())
            })
            .unwrap();
        (rows, stats.max_condition_estimate)
    };
    let ((d, kd), (s, ks)) = (scan("dense"), scan("sparse"));
    let kappa = kd.max(ks);
    let mut worst: f64 = 0.0;
    for (a, b) in d.iter().zip(&s) {
        // Magnitudes relative, angles in degrees.
        for (k, (x, y)) in a.iter().zip(b).enumerate() {
            let (x, y) = (x.unwrap(), y.unwrap());
            let e = if k % 2 == 0 {
                (x - y).abs() / x
            } else {
                (x - y).abs() / 180.0
            };
            worst = worst.max(e);
        }
    }
    // One factorization per frequency: no accumulation, bound 10 kappa eps.
    let bound = 10.0 * kappa * f64::EPSILON;
    eprintln!(
        "scan: condition estimate {kappa:.1e}, largest difference {worst:.2e} (bound {bound:.1e})"
    );
    assert!(worst < bound, "{worst:e}");
}
