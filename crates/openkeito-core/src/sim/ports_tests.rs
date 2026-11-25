//! Tests of what the solver offers to elements beyond a linear stamp: the
//! iteration of the operating point within a stage, the conditions of the
//! run in `prepare`, and signals read and offered. No component uses them
//! yet, so mock elements take the place of ordinary ones.

use std::sync::{Arc, Mutex};

use super::*;
use crate::element::{inject, phasor, volt, Node, OperatingPoint, SignalResponse};
use crate::linalg::{Complex64, Stamp};

/// A source E1 of `volts` behind R1 = 1 ohm, feeding R2 from n2 to gnd.
/// Unknowns: V(n1) = 0, V(n2) = 1. R2 is element 2.
fn divider(method: &str, volts: f64) -> Simulator {
    let text = format!(
        r#"{{"format": "openkeito-model", "version": "0.1",
            "simulation": {{"time_step_s": 1e-6, "end_time_s": 1e-5, "method": "{method}"}},
            "components": [
              {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": {volts}}}}},
              {{"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0}},
              {{"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0}}
            ]}}"#
    );
    let model = openkeito_model::load_str(&text).unwrap().model;
    Simulator::from_model(&model).unwrap()
}

/// A resistor R1 = 2 ohm from n1 to gnd and a current source J1 of 0 A into
/// n1, with the control blocks `controls`. Unknown: V(n1) = 0. J1 is
/// element 1.
fn injection(controls: &str) -> Simulator {
    let text = format!(
        r#"{{"format": "openkeito-model", "version": "0.2",
            "simulation": {{"time_step_s": 0.01, "end_time_s": 0.05}},
            "components": [
              {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 2.0}},
              {{"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {{"type": "dc", "value": 0.0}}}}
            ],
            "controls": {controls}}}"#
    );
    let model = openkeito_model::load_str(&text).unwrap().model;
    Simulator::from_model(&model).unwrap()
}

/// The rows of a run and its statistics.
fn run_rows(sim: Simulator) -> (Vec<Vec<f64>>, RunStats) {
    let mut rows = Vec::new();
    let stats = sim
        .run(|_, row| {
            rows.push(row.to_vec());
            Ok(())
        })
        .unwrap();
    (rows, stats)
}

/// A resistor to gnd with a piecewise-linear characteristic: in segment k,
/// which ends at the voltage `segments[k].0`, i = g v + j with
/// (g, j) = (`segments[k].1`, `segments[k].2`). It moves to the segment of
/// the voltage it sees.
struct Piecewise {
    name: String,
    a: Node,
    segments: Vec<(f64, f64, f64)>,
    segment: usize,
}

impl Element for Piecewise {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        let i = self.a.unwrap();
        m.add(i, i, self.segments[self.segment].1);
    }
    fn rhs(&self, _t: f64, _stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, None, self.segments[self.segment].2);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        let (_, g, j) = self.segments[self.segment];
        g * volt(x, self.a) + j
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        phasor(x, self.a)
    }
    fn has_operating_point(&self) -> bool {
        true
    }
    fn update_operating_point(&mut self, _t: f64, _stage: &Stage, x: &[f64]) -> OperatingPoint {
        let v = volt(x, self.a);
        let k = self.segments.iter().position(|s| v < s.0).unwrap();
        if k == self.segment {
            return OperatingPoint::Consistent;
        }
        let same_slope = self.segments[k].1 == self.segments[self.segment].1;
        self.segment = k;
        if same_slope {
            OperatingPoint::RhsChanged
        } else {
            OperatingPoint::MatrixChanged
        }
    }
}

/// The divider with R2 replaced by a piecewise-linear resistor that starts
/// in `segment`.
fn with_piecewise(volts: f64, segments: &[(f64, f64, f64)], segment: usize) -> Simulator {
    let mut sim = divider("2s-dirk", volts);
    sim.elements[2] = Box::new(Piecewise {
        name: "R2".to_string(),
        a: Some(1),
        segments: segments.to_vec(),
        segment,
    });
    sim.refresh_roles();
    sim
}

/// Continuous characteristic: i = v below 1 V, i = 4 v - 3 from 1 V to 2 V,
/// i = 0.1 v + 4.8 above 2 V.
const THREE_SEGMENTS: [(f64, f64, f64); 3] =
    [(1.0, 1.0, 0.0), (2.0, 4.0, -3.0), (f64::INFINITY, 0.1, 4.8)];

#[test]
fn a_piecewise_linear_resistor_finds_its_segment_in_two_iterations() {
    // With 6 V behind 1 ohm, v = (6 - j) / (1 + g) in segment (g, j):
    // segment 0 gives 3 V, which belongs to segment 2; segment 2 gives
    // 1.2 / 1.1 = 1.09 V, which belongs to segment 1; segment 1 gives
    // 9 / 5 = 1.8 V, which is in it. The exact solution is v = 1.8 V and
    // i = 4 * 1.8 - 3 = 4.2 A = (6 - 1.8) / 1.
    let (rows, stats) = run_rows(with_piecewise(6.0, &THREE_SEGMENTS, 0));
    assert_eq!(rows.len(), 11);
    for row in &rows {
        // Columns: V(n1), V(n2), I(E1), I(R1), I(R2). Rounding only.
        assert!((row[1] - 1.8).abs() < 1e-14, "{row:?}");
        assert!((row[4] - 4.2).abs() < 1e-14, "{row:?}");
        assert!((row[3] - 4.2).abs() < 1e-14, "{row:?}");
    }
    // Both changes happen in the first solution (t = 0+); every later stage
    // starts in the right segment.
    assert_eq!(stats.operating_point_iterations, 2);
    assert_eq!(stats.max_operating_point_iterations, 2);
    // Each change of a slope costs one factorization and one solution more
    // than a run that starts in the right segment.
    let (reference_rows, reference) = rows_of_reference();
    assert_eq!(rows, reference_rows);
    assert_eq!(reference.operating_point_iterations, 0);
    assert_eq!(stats.factorizations, reference.factorizations + 2);
    assert_eq!(stats.solves, reference.solves + 2);
}

fn rows_of_reference() -> (Vec<Vec<f64>>, RunStats) {
    run_rows(with_piecewise(6.0, &THREE_SEGMENTS, 1))
}

#[test]
fn a_change_of_the_right_hand_side_alone_does_not_factorize_again() {
    // Two segments of the same slope: i = v below 1 V, i = v + 1 above.
    // Segment 0 gives 6 / 2 = 3 V, segment 1 gives (6 - 1) / 2 = 2.5 V.
    let segments = [(1.0, 1.0, 0.0), (f64::INFINITY, 1.0, 1.0)];
    let (rows, stats) = run_rows(with_piecewise(6.0, &segments, 0));
    for row in &rows {
        assert!((row[1] - 2.5).abs() < 1e-14, "{row:?}");
    }
    let (_, reference) = run_rows(with_piecewise(6.0, &segments, 1));
    assert_eq!(stats.operating_point_iterations, 1);
    assert_eq!(stats.factorizations, reference.factorizations);
    assert_eq!(stats.solves, reference.solves + 1);
}

#[test]
fn operating_points_that_do_not_converge_end_in_e205() {
    // With 5 V, segment 0 gives 2.5 V (segment 2) and segment 2 gives
    // 0.2 / 1.1 = 0.18 V (segment 0): moving to the segment of the solution
    // goes back and forth for ever.
    let sim = with_piecewise(5.0, &THREE_SEGMENTS, 0);
    let (stats, result) = sim.run_with_stats(|_, _| Ok(()));
    let err = result.unwrap_err();
    match &err {
        SimError::OperatingPointNotConverged {
            time_s,
            components,
            iterations,
        } => {
            // The first solution, at t = 0+.
            assert!(*time_s < 1e-6);
            assert_eq!(components, &["R2"]);
            assert_eq!(*iterations, MAX_OPERATING_POINT_ITERATIONS);
        }
        other => panic!("unexpected error {other:?}"),
    }
    // One solution, and one more for every iteration.
    assert_eq!(stats.solves, 1 + MAX_OPERATING_POINT_ITERATIONS);
    let d = err.to_diagnostic();
    assert_eq!(d.code, "E205");
    assert_eq!(d.path, "R2");
    assert!(d.message.contains("R2"));
    assert!(d.hint.contains("time_step_s"));
}

#[test]
fn a_circuit_without_such_elements_is_not_iterated() {
    let sim = divider("2s-dirk", 6.0);
    assert!(sim.operating_points.is_empty());
    assert!(sim.signal_readers.is_empty());
    assert_eq!(sim.element_signals().count(), 0);
    let (_, stats) = run_rows(sim);
    assert_eq!(stats.operating_point_iterations, 0);
    assert_eq!(stats.max_operating_point_iterations, 0);
    // t = 0+, the regular matrix; t = 0+ and two stages for each of 10 steps.
    assert_eq!(stats.factorizations, 2);
    assert_eq!(stats.solves, 21);
}

/// A 1 S conductance to gnd that keeps what `prepare` received.
struct Recorder {
    a: Node,
    seen: Arc<Mutex<Option<Stages>>>,
}

impl Element for Recorder {
    fn name(&self) -> &str {
        "R2"
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        *self.seen.lock().unwrap() = Some(stages.clone());
        Ok(())
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        let i = self.a.unwrap();
        m.add(i, i, 1.0);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        volt(x, self.a)
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        phasor(x, self.a)
    }
}

#[test]
fn prepare_receives_the_time_step_and_the_stages_of_the_method() {
    let seen = |method: &str| {
        let seen = Arc::new(Mutex::new(None));
        let mut sim = divider(method, 6.0);
        sim.elements[2] = Box::new(Recorder {
            a: Some(1),
            seen: seen.clone(),
        });
        sim.run(|_, _| Ok(())).unwrap();
        let stages: Stages = seen.lock().unwrap().take().expect("prepare was called");
        stages
    };
    let fractions =
        |stages: &[Stage]| -> Vec<f64> { stages.iter().map(|s| s.time_fraction).collect() };
    let gamma = 1.0 - std::f64::consts::FRAC_1_SQRT_2;

    let dirk = seen("2s-dirk");
    assert_eq!(dirk.time_step_s, 1e-6);
    assert_eq!(dirk.method, Method::TwoStageDirk);
    assert_eq!(fractions(&dirk.regular), [gamma, 1.0]);
    assert_eq!(fractions(&dirk.after_event), [gamma, 1.0]);
    assert_eq!(fractions(&dirk.half_steps), [0.5, 1.0]);
    assert_eq!(dirk.alphas()[0], 1.0 / (gamma * 1e-6));

    let trapezoidal = seen("trapezoidal");
    assert_eq!(trapezoidal.method, Method::Trapezoidal);
    assert_eq!(fractions(&trapezoidal.regular), [1.0]);
    assert_eq!(fractions(&trapezoidal.after_event), [0.5, 1.0]);
    assert_eq!(trapezoidal.alphas()[0], 2.0 / 1e-6);
    // The solutions at t = 0+ cover a negligible interval.
    assert_eq!(trapezoidal.initial.len(), 2);
    assert!((trapezoidal.initial_interval - 1e-12).abs() < 1e-24);
}

/// A current source into `b` whose value is the signal `signal`, held over
/// each step.
struct Controlled {
    b: Node,
    signal: usize,
    held: f64,
}

impl Element for Controlled {
    fn name(&self) -> &str {
        "J1"
    }
    fn stamp(&self, _alpha: f64, _m: &mut dyn Stamp) {}
    fn rhs(&self, _t: f64, _stage: &Stage, b: &mut [f64]) {
        inject(b, None, self.b, self.held);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, _x: &[f64]) -> f64 {
        self.held
    }
    fn current_phasor(&self, _k: usize, _omega: f64, _x: &[Complex64]) -> Complex64 {
        Complex64::ZERO
    }
    fn reads_signals(&self) -> bool {
        true
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        if signals[self.signal] == self.held {
            return SignalResponse::Unchanged;
        }
        self.held = signals[self.signal];
        SignalResponse::ValueChanged
    }
}

fn with_controlled(controls: &str) -> Simulator {
    let mut sim = injection(controls);
    sim.elements[1] = Box::new(Controlled {
        b: Some(0),
        signal: 0,
        held: 0.0,
    });
    sim.refresh_roles();
    assert_eq!(sim.signal_readers, [1]);
    sim
}

#[test]
fn a_signal_read_by_an_element_acts_over_the_next_step() {
    // The carrier is u(t) = 2 t for t < 0.5 s. The value read at grid point
    // k holds over step k, so the row at t_k shows 2 ohm * u(t_k - h).
    let controls = r#"[{"type": "triangle_wave", "name": "u", "frequency_hz": 1.0}]"#;
    let (rows, stats) = run_rows(with_controlled(controls));
    let h = 0.01;
    assert_eq!(rows.len(), 6);
    for (k, row) in rows.iter().enumerate() {
        // Columns: V(n1), I(R1), I(J1), S(u).
        let held = if k == 0 {
            0.0
        } else {
            2.0 * (k - 1) as f64 * h
        };
        assert!((row[0] - 2.0 * held).abs() < 1e-14, "row {k}: {row:?}");
        assert!((row[2] - held).abs() < 1e-14, "row {k}: {row:?}");
        assert!(
            (row[3] - 2.0 * k as f64 * h).abs() < 1e-14,
            "row {k}: {row:?}"
        );
    }
    // A change of a value is not a switching action.
    assert!(stats.events.is_empty());
}

#[test]
fn a_value_read_at_t_0_is_in_the_first_row() {
    // The controls give u = 3 at t = 0; t = 0+ is solved again with it.
    let controls = r#"[{"type": "constant", "name": "u", "value": 3.0}]"#;
    let (rows, stats) = run_rows(with_controlled(controls));
    for row in &rows {
        assert!((row[0] - 6.0).abs() < 1e-14, "{row:?}");
    }
    // t = 0+ twice, then two stages for each of 5 steps.
    assert_eq!(stats.solves, 12);
    assert!(stats.events.is_empty());
}

/// An element without a stamp that counts the steps it was advanced by and
/// offers the count as signals.
struct Counter {
    steps: u32,
}

impl Element for Counter {
    fn name(&self) -> &str {
        "J1"
    }
    fn stamp(&self, _alpha: f64, _m: &mut dyn Stamp) {}
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, _x: &[f64]) -> f64 {
        0.0
    }
    fn current_phasor(&self, _k: usize, _omega: f64, _x: &[Complex64]) -> Complex64 {
        Complex64::ZERO
    }
    fn advance(&mut self, stage: &Stage, _x: &[f64]) {
        if stage.commit {
            self.steps += 1;
        }
    }
    fn signal_names(&self) -> Vec<String> {
        vec!["count".to_string(), "negative".to_string()]
    }
    fn signal(&self, k: usize) -> f64 {
        [1.0, -1.0][k] * self.steps as f64
    }
}

#[test]
fn an_element_offers_internal_quantities_as_named_signals() {
    let mut sim = injection("[]");
    sim.elements[1] = Box::new(Counter { steps: 0 });
    sim.refresh_roles();
    let names: Vec<&str> = sim.element_signals().collect();
    assert_eq!(names, ["J1.count", "J1.negative"]);
    // At t = 0 the signals have their initial values.
    assert_eq!(sim.element_signal(0), 0.0);
    let mut stats = RunStats::default();
    let mut sink = |_: f64, _: &[f64]| Ok(());
    sim.run_loop(&mut sink, &mut stats).unwrap();
    // After the run they belong to the last grid point: 5 steps.
    assert_eq!(stats.steps, 5);
    assert_eq!(sim.element_signal(0), 5.0);
    assert_eq!(sim.element_signal(1), -5.0);
}

#[test]
fn the_trial_solution_for_the_diode_states_is_iterated_too() {
    // The divider with a blocking diode across the piecewise-linear
    // resistor (cathode at n2). A diode makes the run settle the diode
    // states at t = 0 with a trial solution; the segment is found there, so
    // the diode is judged on 1.8 V and not on the 3 V of the first guess.
    let text = r#"{"format": "openkeito-model", "version": "0.2",
        "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-5},
        "components": [
          {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 6.0}},
          {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0},
          {"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0},
          {"type": "diode", "name": "D1", "nodes": ["gnd", "n2"]}
        ]}"#;
    let model = openkeito_model::load_str(text).unwrap().model;
    let mut sim = Simulator::from_model(&model).unwrap();
    sim.elements[2] = Box::new(Piecewise {
        name: "R2".to_string(),
        a: Some(1),
        segments: THREE_SEGMENTS.to_vec(),
        segment: 0,
    });
    sim.refresh_roles();
    let (rows, stats) = run_rows(sim);
    for row in &rows {
        // The blocking resistance of 1e6 ohm takes 1.8 uA: v moves by
        // 1.8e-6 / 5 S.
        assert!((row[1] - 1.8).abs() < 1e-6, "{row:?}");
    }
    assert!(stats.events.is_empty());
    // One trial solution, in which both changes of the segment happen.
    assert_eq!(stats.resolves, 1);
    assert_eq!(stats.operating_point_iterations, 2);
}

#[test]
fn a_measurement_reads_the_signal_of_an_element_at_the_grid_point() {
    // No component offers signals yet, so a measurement of a node voltage
    // is pointed at the first signal of the mock element by hand: the block
    // then gives the count of the steps taken up to each grid point.
    let controls = r#"[{"type": "measurement", "name": "m", "quantity": {"type": "node_voltage", "node": "n1"}}]"#;
    let mut sim = injection(controls);
    sim.elements[1] = Box::new(Counter { steps: 0 });
    sim.refresh_roles();
    sim.measurements[0].clear();
    sim.measured_signals = vec![(0, 0)];
    let (rows, _) = run_rows(sim);
    for (k, row) in rows.iter().enumerate() {
        // Columns: V(n1), I(R1), I(J1), S(m).
        assert_eq!(row[3], k as f64, "row {k}");
    }
}
