//! Run times of three-phase reference systems (not run in CI):
//!
//! cargo test --release -p openkeito-core --test benchmark -- --ignored --nocapture --test-threads=1
//!
//! A ring of N buses at 66 kV joined by 10 km pi lines, a grounded star load
//! of 10 MW + 3 Mvar on every bus, and on every tenth bus an 11 kV source
//! behind a 100 MVA YNd11 transformer. A three-phase fault on bus N/2 starts
//! at 0.1 s and is cleared at 0.2 s (current zeros). The run starts from the
//! steady state. Results go to docs/performance.md.

use std::time::Instant;

use openkeito_core::{RunStats, Simulator};
use serde_json::{json, Value};

const F: f64 = 50.0;

/// The ring system of `n` buses as a model.
pub fn ring(n: usize, h: f64, end: f64, solver: &str) -> Value {
    let w = 2.0 * std::f64::consts::PI * F;
    let mut c = Vec::new();
    for k in 0..n {
        let next = (k + 1) % n;
        c.push(json!({"type": "pi_line", "name": format!("L{k}"), "buses": [format!("b{k}"), format!("b{next}")],
            "length_km": 10.0,
            "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3, "c1_f_per_km": 12.7e-9,
                       "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}}));
        // 10 MW + 3 Mvar at 66 kV: Z = V^2 (P + jQ) / (P^2 + Q^2).
        let (p, q, v) = (10e6, 3e6, 66e3);
        let s2 = p * p + q * q;
        let (r, x) = (v * v * p / s2, v * v * q / s2);
        c.push(json!({"type": "coupled_rl", "name": format!("LD{k}"), "buses": [format!("b{k}"), "gnd"],
            "impedance": {"type": "sequence", "r1_ohm": r, "l1_h": x / w, "r0_ohm": r, "l0_h": x / w}}));
        if k % 10 == 0 {
            c.push(json!({"type": "three_phase_source", "name": format!("G{k}"), "bus": format!("g{k}"),
                "line_voltage_rms_v": 11e3, "frequency_hz": F,
                "impedance": {"type": "sequence", "r1_ohm": 0.01, "l1_h": 0.1 / w, "r0_ohm": 0.01, "l0_h": 0.1 / w}}));
            c.push(json!({"type": "three_phase_transformer", "name": format!("T{k}"), "buses": [format!("b{k}"), format!("g{k}")],
                "vector_group": "YNd11", "rated_voltage_kv": [66.0, 11.0], "rated_power_mva": 100.0, "rated_frequency_hz": F,
                "impedance_percent": 12.0, "resistance_percent": 0.3, "no_load_current_percent": 0.2, "no_load_loss_kw": 50.0}));
        }
    }
    let faulted = n / 2;
    c.push(json!({"type": "fault", "name": "F1", "bus": format!("b{faulted}"), "phases": ["a", "b", "c"], "ground": true,
        "start_time_s": 0.1, "clear_time_s": 0.2}));
    json!({
        "format": "openkeito-model",
        "version": "0.3",
        "name": format!("ring of {n} buses"),
        "simulation": {"time_step_s": h, "end_time_s": end, "initialization": "steady_state",
                       "linear_solver": solver, "output_interval_steps": 10},
        "components": c,
        "outputs": [{"type": "bus_voltage", "bus": format!("b{faulted}")},
                    {"type": "branch_current", "component": "F1"}],
    })
}

struct Measured {
    stats: RunStats,
    wall_s: f64,
    rows: Vec<Vec<f64>>,
}

fn measure(model: &Value) -> Measured {
    let loaded = openkeito_model::load_str(&model.to_string()).expect("valid model");
    let started = Instant::now();
    let sim = Simulator::from_model(&loaded.model).expect("builds");
    let mut rows = Vec::new();
    let stats = sim
        .run(|_, row| {
            rows.push(row.to_vec());
            Ok(())
        })
        .expect("runs");
    Measured {
        stats,
        wall_s: started.elapsed().as_secs_f64(),
        rows,
    }
}

fn print_row(n: usize, h: f64, end: f64, m: &Measured) {
    let s = &m.stats;
    let t = &s.timing;
    let per_step = m.wall_s / s.steps as f64;
    let scale = 1.0 / end;
    let init = s.initialization.as_ref().unwrap();
    println!(
        "| {n} | {} | {} | {:.0} | {:.2} | {:.3} s{} | {:.2} | {} / {:.4} s | {} / {:.3} s | {:.1e} | {:.1e} |",
        s.unknowns,
        s.linear_solver,
        h * 1e6,
        end,
        m.wall_s * scale,
        if end < 1.0 { " (extrapolated)" } else { "" },
        per_step * 1e6,
        s.factorizations,
        t.factorize_s,
        s.solves,
        t.solve_s,
        init.relative_voltage_deviation,
        s.max_condition_estimate,
    );
}

#[test]
#[ignore = "takes minutes; run by hand with --release"]
fn ring_systems() {
    println!(
        "| Buses | Unknowns | Solver | Step (us) | Simulated (s) | Wall clock for 1 s | Per step (us) | \
         Factorizations / time | Solutions / time | Departure from steady state | Condition estimate |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for h in [10e-6, 50e-6] {
        for n in [10, 30, 100, 300, 1000] {
            for solver in ["dense", "sparse"] {
                // Dense runs grow as unknowns^2 per step; run them shorter
                // and scale to 1 s.
                let end = match (solver, n) {
                    ("dense", 1000) => continue,
                    ("dense", 300) => 0.02,
                    ("dense", 100) => 0.2,
                    _ => 1.0,
                };
                let m = measure(&ring(n, h, end, solver));
                print_row(n, h, end, &m);
                assert!(!m.rows.is_empty());
            }
        }
    }
}

#[test]
#[ignore = "run by hand with --release"]
fn dense_and_sparse_per_step_by_size() {
    // Where "auto" should switch: time per step of small rings.
    println!("| Buses | Unknowns | Dense per step (us) | Sparse per step (us) |");
    println!("|---|---|---|---|");
    for n in [2, 3, 4, 5, 6, 8, 10, 15, 20] {
        let mut per_step = Vec::new();
        let mut unknowns = 0;
        for solver in ["dense", "sparse"] {
            // Best of 3 to reduce noise.
            let best = (0..3)
                .map(|_| {
                    let m = measure(&ring(n, 10e-6, 0.2, solver));
                    unknowns = m.stats.unknowns;
                    m.wall_s / m.stats.steps as f64
                })
                .fold(f64::INFINITY, f64::min);
            per_step.push(best);
        }
        println!(
            "| {n} | {unknowns} | {:.2} | {:.2} |",
            per_step[0] * 1e6,
            per_step[1] * 1e6
        );
    }
}

#[test]
#[ignore = "run by hand with --release"]
fn dense_and_sparse_per_step_for_the_examples() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    println!("| Example | Unknowns | Dense per step (us) | Sparse per step (us) |");
    println!("|---|---|---|---|");
    for name in [
        "rlc",
        "rl_breaker",
        "buck",
        "rectifier",
        "three_phase_fault",
    ] {
        let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
        let mut m: Value = serde_json::from_str(&text).unwrap();
        m["version"] = "0.3".into();
        let mut per_step = Vec::new();
        let mut unknowns = 0;
        for solver in ["dense", "sparse"] {
            m["simulation"]["linear_solver"] = solver.into();
            let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
            let best = (0..5)
                .map(|_| {
                    let sim = Simulator::from_model(&loaded.model).unwrap();
                    let stats = sim.run(|_, _| Ok(())).unwrap();
                    unknowns = stats.unknowns;
                    (stats.timing.total_s - stats.timing.output_s) / stats.steps as f64
                })
                .fold(f64::INFINITY, f64::min);
            per_step.push(best);
        }
        println!(
            "| {name} | {unknowns} | {:.3} | {:.3} |",
            per_step[0] * 1e6,
            per_step[1] * 1e6
        );
    }
}

#[test]
#[ignore = "run by hand with --release"]
fn dense_and_sparse_per_step_of_a_chain() {
    // An ideal source and k coupled RL sections with a load: 6 + 3 k unknowns.
    println!("| Sections | Unknowns | Dense per step (us) | Sparse per step (us) |");
    println!("|---|---|---|---|");
    for k in 1..=8 {
        let mut c = vec![
            json!({"type": "three_phase_source", "name": "G", "bus": "n0",
            "line_voltage_rms_v": 66e3, "frequency_hz": F}),
        ];
        for s in 0..k {
            c.push(json!({"type": "coupled_rl", "name": format!("Z{s}"), "buses": [format!("n{s}"), format!("n{}", s + 1)],
                "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": 0.01, "r0_ohm": 3.0, "l0_h": 0.03}}));
        }
        c.push(json!({"type": "coupled_rl", "name": "LD", "buses": [format!("n{k}"), "gnd"],
            "impedance": {"type": "sequence", "r1_ohm": 400.0, "l1_h": 0.4, "r0_ohm": 400.0, "l0_h": 0.4}}));
        let mut per_step = Vec::new();
        let mut unknowns = 0;
        for solver in ["dense", "sparse"] {
            let m = json!({"format": "openkeito-model", "version": "0.3",
                "simulation": {"time_step_s": 1e-5, "end_time_s": 0.2, "linear_solver": solver},
                "components": c, "outputs": [{"type": "bus_voltage", "bus": format!("n{k}")}]});
            let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
            let best = (0..5)
                .map(|_| {
                    let stats = Simulator::from_model(&loaded.model)
                        .unwrap()
                        .run(|_, _| Ok(()))
                        .unwrap();
                    unknowns = stats.unknowns;
                    (stats.timing.total_s - stats.timing.output_s) / stats.steps as f64
                })
                .fold(f64::INFINITY, f64::min);
            per_step.push(best);
        }
        println!(
            "| {k} | {unknowns} | {:.3} | {:.3} |",
            per_step[0] * 1e6,
            per_step[1] * 1e6
        );
    }
}

/// The ring with every line of type `line_type`.
fn ring_of(n: usize, h: f64, end: f64, line_type: &str) -> Value {
    let mut m = ring(n, h, end, "auto");
    for c in m["components"].as_array_mut().unwrap() {
        if c["type"] == "pi_line" {
            c["type"] = line_type.into();
        }
    }
    if line_type != "pi_line" {
        m["version"] = "0.5".into();
    }
    m
}

/// Best time per step of 5 runs of 0.2 s at h = 10 us, without the time of
/// the output, and the statistics of the last run.
fn best_per_step(model: &Value) -> (f64, RunStats) {
    let loaded = openkeito_model::load_str(&model.to_string()).expect("valid model");
    let mut best = f64::INFINITY;
    let mut last = None;
    for _ in 0..5 {
        let stats = Simulator::from_model(&loaded.model)
            .expect("builds")
            .run(|_, _| Ok(()))
            .expect("runs");
        best = best.min((stats.timing.total_s - stats.timing.output_s) / stats.steps as f64);
        last = Some(stats);
    }
    (best, last.unwrap())
}

#[test]
#[ignore = "run by hand with --release"]
fn pi_ring_per_step() {
    // The reference for changes that must not slow down models without the
    // new components: compare before and after a change.
    // Pin the test to one core for that (see `bergeron_ring_per_step`).
    println!("| Buses | Unknowns | Per step (us) |");
    println!("|---|---|---|");
    for n in [10, 100] {
        let (per_step, stats) = best_per_step(&ring_of(n, 10e-6, 0.2, "pi_line"));
        println!("| {n} | {} | {:.3} |", stats.unknowns, per_step * 1e6);
    }
}

/// Time of a fixed piece of arithmetic on the core this thread runs on now:
/// tells the kinds of cores of a processor apart.
fn reference_s() -> f64 {
    (0..3)
        .map(|_| {
            let started = Instant::now();
            let mut s = 0.0_f64;
            for i in 1..400_000_u32 {
                s += (std::hint::black_box(i) as f64).sqrt() / (1.0 + s);
            }
            std::hint::black_box(s);
            started.elapsed().as_secs_f64()
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
#[ignore = "run by hand with --release"]
fn bergeron_ring_per_step() {
    // The 10 km lines of the ring have travel times of 33.8 us (line modes)
    // and 45.8 us (zero sequence): above the step of 10 us, so the same ring
    // runs with either line model. The Bergeron ring has the same unknowns
    // (one pi section has no inner nodes), but no entries between the buses
    // of a line: the base for solving the buses in parallel.
    //
    // On a processor with two kinds of cores the system moves a thread that
    // runs for long to the slower kind: runs made one after the other are
    // then not comparable (the first version of this test took the pi line
    // on a fast core and the Bergeron line on a slow one, 30 % apart). So
    // the two lines are run in turn, a reference loop before and after each
    // run tells the core it ran on (not what happened in between), and only
    // runs on one kind of core are compared:
    // the fastest, or a slower one when a line never ran on the fastest. For
    // clean numbers pin the test to one core all the same:
    //
    // cmd /c start /b /wait /affinity 1 <test executable> --ignored --nocapture bergeron_ring_per_step
    const LINES: [&str; 2] = ["pi_line", "bergeron_line"];
    const ROUNDS: usize = 6;
    struct Timed {
        per_step: f64,
        reference: f64,
    }
    let sizes = [10, 30, 100, 300];
    let mut runs: Vec<[Vec<Timed>; 2]> = Vec::new();
    let mut last: Vec<[Option<RunStats>; 2]> = Vec::new();
    for n in sizes {
        let models = LINES.map(|line| {
            openkeito_model::load_str(&ring_of(n, 10e-6, 0.2, line).to_string())
                .expect("valid model")
        });
        let mut timed = [Vec::new(), Vec::new()];
        let mut stats = [None, None];
        for _ in 0..ROUNDS {
            for (k, loaded) in models.iter().enumerate() {
                let before = reference_s();
                let s = Simulator::from_model(&loaded.model)
                    .expect("builds")
                    .run(|_, _| Ok(()))
                    .expect("runs");
                timed[k].push(Timed {
                    per_step: (s.timing.total_s - s.timing.output_s) / s.steps as f64,
                    // The slower of the cores before and after the run.
                    reference: before.max(reference_s()),
                });
                stats[k] = Some(s);
            }
        }
        runs.push(timed);
        last.push(stats);
    }
    let fastest = runs
        .iter()
        .flatten()
        .flatten()
        .map(|t| t.reference)
        .fold(f64::INFINITY, f64::min);
    println!(
        "| Buses | Line | Unknowns | Per step (us) | Against pi_line | Runs compared | Cores | \
         Departure from steady state |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for ((n, timed), stats) in sizes.iter().zip(&runs).zip(&last) {
        // The kind of core to compare on: the fastest of the test, or, when
        // one of the lines never ran there, the fastest that both ran on.
        let slowest_best = timed
            .iter()
            .map(|runs| {
                runs.iter()
                    .map(|t| t.reference)
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max);
        let (kind, limit) = if slowest_best <= 1.1 * fastest {
            ("fastest", 1.1 * fastest)
        } else {
            ("slower", 1.1 * slowest_best)
        };
        // The best run among those on that kind of core.
        let best = |k: usize| {
            let compared: Vec<f64> = timed[k]
                .iter()
                .filter(|t| {
                    t.reference <= limit && (kind == "fastest" || t.reference > 1.1 * fastest)
                })
                .map(|t| t.per_step)
                .collect();
            (
                compared.iter().copied().fold(f64::INFINITY, f64::min),
                compared.len(),
            )
        };
        let pi = best(0).0;
        for (k, line) in LINES.iter().enumerate() {
            let (per_step, count) = best(k);
            let s = stats[k].as_ref().unwrap();
            println!(
                "| {n} | {line} | {} | {:.3} | {:.3} | {count} of {ROUNDS} | {kind} | {:.1e} |",
                s.unknowns,
                per_step * 1e6,
                per_step / pi,
                s.initialization
                    .as_ref()
                    .unwrap()
                    .relative_voltage_deviation,
            );
        }
    }
}

/// A run of `model` not divided ("off"), or divided at its lines ("auto",
/// "every_line") and solved by `threads` threads: the time per step without the output,
/// the statistics and the rows.
fn timed(model: &Value, partitioning: &str, threads: usize) -> (f64, RunStats, Vec<Vec<f64>>) {
    let mut model = model.clone();
    model["simulation"]["partitioning"] = partitioning.into();
    model["version"] = "0.5".into();
    let loaded = openkeito_model::load_str(&model.to_string()).expect("valid model");
    let mut sim = Simulator::from_model(&loaded.model).expect("builds");
    sim.use_threads(threads);
    let mut rows = Vec::new();
    let stats = sim
        .run(|_, row| {
            rows.push(row.to_vec());
            Ok(())
        })
        .expect("runs");
    let per_step = (stats.timing.total_s - stats.timing.output_s) / stats.steps as f64;
    (per_step, stats, rows)
}

/// Largest difference between two runs, relative to the largest value.
fn relative_difference(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    let mut largest = 0.0_f64;
    let mut difference = 0.0_f64;
    for (ra, rb) in a.iter().zip(b) {
        for (va, vb) in ra.iter().zip(rb) {
            largest = largest.max(va.abs());
            difference = difference.max((va - vb).abs());
        }
    }
    difference / largest
}

/// Blocks of `block` buses joined by pi lines (10 km) into a chain, with a
/// load on every bus and a source behind a transformer on the first bus of
/// every block; the blocks are joined into a ring by 50 km lines of type
/// `long_line`. With Bergeron lines the circuit divides into the blocks.
pub fn blocks(count: usize, block: usize, long_line: &str, end: f64) -> Value {
    let w = 2.0 * std::f64::consts::PI * F;
    let per_km = json!({"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3, "c1_f_per_km": 12.7e-9,
                        "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9});
    let mut c = Vec::new();
    for g in 0..count {
        for k in 0..block {
            let bus = format!("b{g}_{k}");
            if k + 1 < block {
                c.push(json!({"type": "pi_line", "name": format!("L{g}_{k}"), "buses": [bus, format!("b{g}_{}", k + 1)],
                    "length_km": 10.0, "per_km": per_km}));
            }
            let (p, q, v) = (10e6, 3e6, 66e3);
            let s2 = p * p + q * q;
            let (r, x) = (v * v * p / s2, v * v * q / s2);
            c.push(json!({"type": "coupled_rl", "name": format!("LD{g}_{k}"), "buses": [bus, "gnd"],
                "impedance": {"type": "sequence", "r1_ohm": r, "l1_h": x / w, "r0_ohm": r, "l0_h": x / w}}));
        }
        c.push(json!({"type": "three_phase_source", "name": format!("G{g}"), "bus": format!("g{g}"),
            "line_voltage_rms_v": 11e3, "frequency_hz": F,
            "impedance": {"type": "sequence", "r1_ohm": 0.01, "l1_h": 0.1 / w, "r0_ohm": 0.01, "l0_h": 0.1 / w}}));
        c.push(json!({"type": "three_phase_transformer", "name": format!("T{g}"), "buses": [format!("b{g}_0"), format!("g{g}")],
            "vector_group": "YNd11", "rated_voltage_kv": [66.0, 11.0], "rated_power_mva": 100.0, "rated_frequency_hz": F,
            "impedance_percent": 12.0, "resistance_percent": 0.3, "no_load_current_percent": 0.2, "no_load_loss_kw": 50.0}));
        let next = (g + 1) % count;
        c.push(json!({"type": long_line, "name": format!("X{g}"),
            "buses": [format!("b{g}_{}", block - 1), format!("b{next}_0")], "length_km": 50.0, "per_km": per_km}));
    }
    let faulted = format!("b{}_{}", count / 2, block / 2);
    c.push(json!({"type": "fault", "name": "F1", "bus": faulted, "phases": ["a", "b", "c"], "ground": true,
        "start_time_s": 0.1, "clear_time_s": 0.2}));
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "name": format!("{count} blocks of {block} buses"),
        "simulation": {"time_step_s": 10e-6, "end_time_s": end, "initialization": "steady_state",
                       "output_interval_steps": 10},
        "components": c,
        "outputs": [{"type": "bus_voltage", "bus": faulted}, {"type": "branch_current", "component": "F1"}],
    })
}

/// The numbers of an environment variable, or of `default`.
fn numbers(variable: &str, default: &str) -> Vec<usize> {
    std::env::var(variable)
        .unwrap_or_else(|_| default.into())
        .split(',')
        .map(|t| t.trim().parse().expect("a whole number"))
        .collect()
}

/// One table of `partition_per_step`. Every system is given with Bergeron
/// lines and with pi lines in their place, and is run: with pi lines; with
/// Bergeron lines not divided; divided at every line on one thread; divided
/// as "auto" does with the numbers of threads of OPENKEITO_BENCH_THREADS
/// (default "1"); and divided at every line with the largest of them. The
/// configurations are run in turn, `ROUNDS` times, and the best time of each
/// is taken.
fn partition_table(models: &[(String, Value, Value)]) {
    const ROUNDS: usize = 5;
    let threads = numbers("OPENKEITO_BENCH_THREADS", "1");
    let most = threads.iter().copied().max().unwrap_or(1);
    println!(
        "| System | Unknowns | Lines | Partitioning | Parts (largest) | Threads | Per step (us) | \
         Against undivided | Departure from steady state | Difference from undivided |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (name, bergeron, pi) in models {
        let mut configurations = vec![(pi, "off", 1), (bergeron, "off", 1)];
        configurations.push((bergeron, "every_line", 1));
        configurations.extend(threads.iter().map(|&t| (bergeron, "auto", t)));
        if most > 1 {
            configurations.push((bergeron, "every_line", most));
        }
        let mut best = vec![f64::INFINITY; configurations.len()];
        let mut last = Vec::new();
        for _ in 0..ROUNDS {
            last.clear();
            for (k, (model, partitioning, threads)) in configurations.iter().enumerate() {
                let (per_step, stats, rows) = timed(model, partitioning, *threads);
                best[k] = best[k].min(per_step);
                last.push((stats, rows));
            }
        }
        for (k, (_, partitioning, _)) in configurations.iter().enumerate() {
            let (stats, rows) = &last[k];
            let (lines, against, difference) = if k == 0 {
                ("pi_line", "-".to_string(), "-".to_string())
            } else {
                (
                    "bergeron_line",
                    format!("{:.2}", best[k] / best[1]),
                    format!("{:.1e}", relative_difference(rows, &last[1].1)),
                )
            };
            println!(
                "| {name} | {} | {lines} | {partitioning} | {} ({}) | {} | {:.2} | {against} | {:.1e} | {difference} |",
                stats.unknowns,
                stats.parts,
                stats.largest_part_unknowns,
                stats.threads,
                best[k] * 1e6,
                stats
                    .initialization
                    .as_ref()
                    .unwrap()
                    .relative_voltage_deviation,
            );
        }
    }
}

/// Time per step of circuits divided at their Bergeron lines, against the
/// same circuits solved with one matrix (docs/performance.md). Pin the test
/// to the cores to measure on, as many as the largest number of threads:
///
/// set OPENKEITO_BENCH_THREADS=1,2,4,8
/// cmd /c start /b /wait /affinity <mask> <test executable> --ignored --nocapture partition_per_step
///
/// OPENKEITO_BENCH_BUSES chooses the rings (default "10,30,100,300,1000";
/// "0" for none), OPENKEITO_BENCH_BLOCKS=0 leaves the blocks out.
#[test]
#[ignore = "run by hand with --release"]
fn partition_per_step() {
    let rings: Vec<(String, Value, Value)> = numbers("OPENKEITO_BENCH_BUSES", "10,30,100,300,1000")
        .into_iter()
        .filter(|&n| n > 0)
        .map(|n| {
            (
                format!("ring of {n} buses"),
                ring_of(n, 10e-6, 0.2, "bergeron_line"),
                ring_of(n, 10e-6, 0.2, "pi_line"),
            )
        })
        .collect();
    partition_table(&rings);
    if numbers("OPENKEITO_BENCH_BLOCKS", "1") != [0] {
        let blocks: Vec<(String, Value, Value)> = [(8, 30), (16, 30), (8, 100)]
            .into_iter()
            .map(|(count, block)| {
                (
                    format!("{count} blocks of {block} buses"),
                    blocks(count, block, "bergeron_line", 0.2),
                    blocks(count, block, "pi_line", 0.2),
                )
            })
            .collect();
        partition_table(&blocks);
    }
}

/// Time per step of the examples, which show what the components of M4
/// cost: best of `ROUNDS` runs, without the time of the output. Pin the test
/// to one core.
#[test]
#[ignore = "run by hand with --release"]
fn examples_per_step() {
    const ROUNDS: usize = 9;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .collect();
    names.sort();
    println!(
        "| Example | Unknowns | Control blocks | Steps | Per step (us) | Solutions per step | \
         Stage iterations per step | Factorizations | Split steps |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for name in names {
        let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
        let loaded = openkeito_model::load_str(&text).unwrap();
        let blocks = openkeito_model::expanded(&loaded.model).controls.len();
        let mut best = f64::INFINITY;
        let mut last = None;
        for _ in 0..ROUNDS {
            let stats = Simulator::from_model(&loaded.model)
                .expect("builds")
                .run(|_, _| Ok(()))
                .expect("runs");
            best = best.min((stats.timing.total_s - stats.timing.output_s) / stats.steps as f64);
            last = Some(stats);
        }
        let s = last.unwrap();
        println!(
            "| {name} | {} | {blocks} | {} | {:.3} | {:.2} | {:.2} | {} | {} |",
            s.unknowns,
            s.steps,
            best * 1e6,
            s.solves as f64 / s.steps as f64,
            s.operating_point_iterations as f64 / s.steps as f64,
            s.factorizations,
            s.split_steps,
        );
    }
}

/// The time of a fixed piece of arithmetic on the cores this process may
/// use, to tell fast cores from slow ones: run it with one core per mask.
#[test]
#[ignore = "run by hand with --release"]
fn reference_speed() {
    println!("reference {:.1} us", reference_s() * 1e6);
}
