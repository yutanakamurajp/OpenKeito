//! Shared by the test files: a run with its rows (`controls.rs`,
//! `converter.rs` and `synchronous_machine.rs` add their own ways of
//! reading a column), and
//!
//! - for `power_electronics.rs` and `switching.rs`, the circuits of the buck
//!   converter and the diode bridge with their exact solutions for ideal
//!   switching instants, which do not depend on the time step or on the
//!   events of the run;
//! - for `three_phase.rs` and `steady_state.rs`, the phasors (`phasors`).
//!
//! A helper that computes a reference value keeps the order of its
//! operations when it is moved here: the comparisons of the tests then see
//! the same bits as before.
#![allow(dead_code)]

pub mod phasors;

use openkeito_core::{RunStats, SimError, Simulator};
use openkeito_model::Diagnostic;
use serde_json::{json, Value};

/// A run: the times and the values of its rows, the names of its columns,
/// what it counted and its time step.
pub struct Run {
    pub t: Vec<f64>,
    pub rows: Vec<Vec<f64>>,
    pub columns: Vec<String>,
    pub stats: RunStats,
    pub h: f64,
}

impl Run {
    /// Index of the column `name`.
    pub fn col(&self, name: &str) -> usize {
        self.columns
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("no column {name} in {:?}", self.columns))
    }

    /// Steady-state phasor of the recorded quantity `name`.
    pub fn steady(&self, name: &str) -> openkeito_core::Complex64 {
        self.stats
            .initialization
            .as_ref()
            .expect("initialized run")
            .phasors
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no phasor {name}"))
            .value
    }
}

/// What the validation and the building of the circuit say about a model:
/// the errors, or the warnings of a model without errors.
pub fn diagnostics(model: &Value) -> Vec<Diagnostic> {
    match openkeito_model::load_str(&model.to_string()) {
        Ok(loaded) => match Simulator::from_model(&loaded.model) {
            Ok(_) => loaded.warnings,
            Err(d) => d,
        },
        Err(d) => d,
    }
}

/// Sets `simulation.switching` (and the version that has it).
pub fn with_switching(mut model: Value, switching: &str) -> Value {
    model["version"] = "0.5".into();
    model["simulation"]["switching"] = switching.into();
    model
}

/// Runs a built `Simulator` whose time step is `h`.
pub fn try_run_sim(sim: Simulator, h: f64) -> Result<Run, SimError> {
    let columns = sim.columns().iter().map(|c| c.name.clone()).collect();
    let mut t = Vec::new();
    let mut rows = Vec::new();
    let stats = sim.run(|time, row| {
        t.push(time);
        rows.push(row.to_vec());
        Ok(())
    })?;
    Ok(Run {
        t,
        rows,
        columns,
        stats,
        h,
    })
}

/// Runs a built `Simulator` whose time step is `h`, panicking on failure.
pub fn run_sim(sim: Simulator, h: f64) -> Run {
    try_run_sim(sim, h).unwrap_or_else(|e| panic!("run fails: {:?}", e.to_diagnostic()))
}

pub fn try_run(model: &Value) -> Result<Run, SimError> {
    let loaded = openkeito_model::load_str(&model.to_string())
        .unwrap_or_else(|e| panic!("model is valid: {e:#?}"));
    let sim = Simulator::from_model(&loaded.model).expect("circuit builds");
    try_run_sim(sim, loaded.model.simulation.time_step_s)
}

/// Runs a model as it is.
pub fn run_model(model: &Value) -> Run {
    try_run(model).unwrap_or_else(|e| panic!("run fails: {:?}", e.to_diagnostic()))
}

/// Runs a model with the given `simulation.switching`.
pub fn run_with(model: &Value, switching: &str) -> Run {
    run_model(&with_switching(model.clone(), switching))
}

pub const RON: f64 = 1e-3;
pub const ROFF: f64 = 1e6;

// ---------------------------------------------------------------------------
// Exact solution of a linear system x' = A x + b with two states.

pub type M2 = [[f64; 2]; 2];

/// e^{A t} for a 2x2 matrix, from its eigenvalues. Real eigenvalues use
/// Sylvester's formula, which stays finite for stiff systems.
pub fn expm(a: M2, t: f64) -> M2 {
    let s = 0.5 * (a[0][0] + a[1][1]);
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let disc = s * s - det;
    let shifted = [[a[0][0] - s, a[0][1]], [a[1][0], a[1][1] - s]];
    // p * I + q * (A - s I)
    let combine = |p: f64, q: f64| -> M2 {
        [
            [p + q * shifted[0][0], q * shifted[0][1]],
            [q * shifted[1][0], p + q * shifted[1][1]],
        ]
    };
    if disc > 1e-12 * s * s {
        let r = disc.sqrt();
        let (l1, l2) = (s + r, s - r);
        let (e1, e2) = ((l1 * t).exp(), (l2 * t).exp());
        // (e1 (A - l2 I) - e2 (A - l1 I)) / (l1 - l2), with A - l I = (A - s I) + (s - l) I
        combine(
            (e1 * (s - l2) - e2 * (s - l1)) / (l1 - l2),
            (e1 - e2) / (l1 - l2),
        )
    } else if disc < -1e-12 * s * s {
        let w = (-disc).sqrt();
        let e = (s * t).exp();
        combine(e * (w * t).cos(), e * (w * t).sin() / w)
    } else {
        let e = (s * t).exp();
        combine(e, e * t)
    }
}

/// x(t) for x' = A x + b from x0.
pub fn propagate(a: M2, b: [f64; 2], x0: [f64; 2], t: f64) -> [f64; 2] {
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    // Steady state x_ss = -A^-1 b.
    let xss = [
        -(a[1][1] * b[0] - a[0][1] * b[1]) / det,
        -(-a[1][0] * b[0] + a[0][0] * b[1]) / det,
    ];
    let p = expm(a, t);
    let d = [x0[0] - xss[0], x0[1] - xss[1]];
    [
        xss[0] + p[0][0] * d[0] + p[0][1] * d[1],
        xss[1] + p[1][0] * d[0] + p[1][1] * d[1],
    ]
}

// ---------------------------------------------------------------------------
// Buck converter: E -- S1 -- sw -- L1 -- out, D1 from gnd to sw, C1 and R1
// from out to gnd. States x = [i_L, v_C].

#[derive(Debug, Clone, Copy)]
pub struct Buck {
    pub e: f64,
    pub l: f64,
    pub c: f64,
    pub r: f64,
    pub f: f64,
    pub duty: f64,
}

pub const CCM: Buck = Buck {
    e: 100.0,
    l: 1e-3,
    c: 1e-4,
    r: 10.0,
    f: 1e4,
    duty: 0.4,
};

pub const DCM: Buck = Buck {
    e: 100.0,
    l: 50e-6,
    c: 1e-3,
    r: 20.0,
    f: 1e4,
    // Comparator crossings between grid points for h = 2, 1, 0.5, 0.25 us.
    duty: 0.4013,
};

impl Buck {
    /// Outputs: I(L1), V(out), V(sw), S(pwm).
    pub fn model(&self, method: &str, h: f64, end: f64, x0: [f64; 2]) -> Value {
        json!({
            "format": "openkeito-model",
            "version": "0.2",
            "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
            "components": [
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": self.e}},
                {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "sw"], "gate": "pwm", "on_resistance_ohm": RON, "off_resistance_ohm": ROFF},
                {"type": "diode", "name": "D1", "nodes": ["gnd", "sw"], "on_resistance_ohm": RON, "off_resistance_ohm": ROFF},
                {"type": "inductor", "name": "L1", "nodes": ["sw", "out"], "inductance_h": self.l, "initial_current_a": x0[0]},
                {"type": "capacitor", "name": "C1", "nodes": ["out", "gnd"], "capacitance_f": self.c, "initial_voltage_v": x0[1]},
                {"type": "resistor", "name": "R1", "nodes": ["out", "gnd"], "resistance_ohm": self.r},
            ],
            "controls": [
                {"type": "constant", "name": "duty", "value": self.duty},
                {"type": "triangle_wave", "name": "carrier", "frequency_hz": self.f},
                {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]},
            ],
            "outputs": [
                {"type": "branch_current", "component": "L1"},
                {"type": "node_voltage", "node": "out"},
                {"type": "node_voltage", "node": "sw"},
                {"type": "signal", "signal": "pwm"},
            ],
        })
    }

    pub fn system(&self, s_on: bool, d_on: bool) -> (M2, [f64; 2]) {
        let g = |on: bool| if on { 1.0 / RON } else { 1.0 / ROFF };
        let (gs, gd) = (g(s_on), g(d_on));
        let gt = gs + gd;
        // KCL at sw: v_sw = (gs E - i_L) / (gs + gd).
        (
            [
                [-1.0 / (self.l * gt), -1.0 / self.l],
                [1.0 / self.c, -1.0 / (self.r * self.c)],
            ],
            [gs * self.e / (gt * self.l), 0.0],
        )
    }

    /// The state after `dt` with S1 in the state `s`, the diode conducting
    /// while S1 is off and the inductor current is positive, and turning
    /// off at the exact zero crossing.
    fn advance(&self, s: bool, x: [f64; 2], dt: f64) -> [f64; 2] {
        let d = !s && x[0] > 0.0;
        let (a, b) = self.system(s, d);
        let next = propagate(a, b, x, dt);
        if !(d && next[0] < 0.0) {
            return next;
        }
        // Bisection for the zero crossing of i_L within dt.
        let (mut lo, mut hi) = (0.0, dt);
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if propagate(a, b, x, mid)[0] > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let at_zero = propagate(a, b, x, hi);
        let (a2, b2) = self.system(false, false);
        propagate(a2, b2, at_zero, dt - hi)
    }

    /// The instants at which the gate changes up to `end`, for the constant
    /// duty and the triangular carrier that starts at its minimum: S1 is on
    /// while duty > carrier, from (k - D/2)/f to (k + D/2)/f. Each with the
    /// state after it.
    pub fn gate_instants(&self, end: f64) -> Vec<(f64, bool)> {
        let mut v = Vec::new();
        let mut k = 0.0;
        loop {
            let off = (k + 0.5 * self.duty) / self.f;
            let on = (k + 1.0 - 0.5 * self.duty) / self.f;
            if off > end {
                break;
            }
            v.push((off, false));
            if on <= end {
                v.push((on, true));
            }
            k += 1.0;
        }
        v
    }

    /// Exact solution at the grid points k h for ideal switching instants:
    /// the gate changes at the crossings of duty and carrier, the diode
    /// turns off at the zero of the inductor current.
    pub fn ideal(&self, h: f64, steps: usize, x0: [f64; 2]) -> Vec<[f64; 2]> {
        let gates = self.gate_instants((steps as f64 + 1.0) * h);
        let mut next = 0;
        let mut s = true;
        let mut x = x0;
        let mut out = vec![x];
        for k in 0..steps {
            let mut t = k as f64 * h;
            let t1 = (k + 1) as f64 * h;
            while next < gates.len() && gates[next].0 <= t1 {
                let (tg, state) = gates[next];
                if tg > t {
                    x = self.advance(s, x, tg - t);
                    t = tg;
                }
                s = state;
                next += 1;
            }
            if t1 > t {
                x = self.advance(s, x, t1 - t);
            }
            out.push(x);
        }
        out
    }

    /// Exact solution at the grid points for the states `s_on[k]` of S1
    /// over step k (the instants a run with switching at grid points used)
    /// and ideally timed diode zero crossings.
    pub fn reference(&self, h: f64, s_on: &[bool], x0: [f64; 2]) -> Vec<[f64; 2]> {
        let mut x = x0;
        let mut out = vec![x];
        for &s in s_on {
            x = self.advance(s, x, h);
            out.push(x);
        }
        out
    }
}

/// Error constant of 2S-DIRK: R(z) - e^z = C3 z^3 + O(z^4) with
/// R(z) = (1 + (1 - 2 gamma) z)/(1 - gamma z)^2, |C3| = |3 gamma^2 - 2 gamma^3 - 1/6|.
/// The local error of a step is this times h^3 |x'''|.
pub fn dirk_error_constant() -> f64 {
    let g = 1.0 - std::f64::consts::FRAC_1_SQRT_2;
    (3.0 * g * g - 2.0 * g * g * g - 1.0 / 6.0).abs()
}

/// Fraction of the time from t0 to t1 in which `name` conducts, from the
/// switching events (off before the first).
pub fn on_time_fraction(stats: &RunStats, name: &str, t0: f64, t1: f64) -> f64 {
    let mut on = false;
    let mut last = t0;
    let mut total = 0.0;
    for e in stats.events.iter().filter(|e| e.component == name) {
        let t = e.time_s.clamp(t0, t1);
        if on {
            total += t - last;
        }
        last = t;
        on = e.action == "close";
    }
    if on {
        total += t1 - last;
    }
    total / (t1 - t0)
}

/// Mean of column `k` over the rows with t0 <= t < t1 (a whole number of
/// periods of the grid values).
pub fn mean(out: &Run, k: usize, t0: f64, t1: f64) -> f64 {
    let v: Vec<f64> = out
        .t
        .iter()
        .zip(&out.rows)
        .filter(|(t, _)| **t >= t0 - 1e-12 && **t < t1 - 1e-12)
        .map(|(_, r)| r[k])
        .collect();
    v.iter().sum::<f64>() / v.len() as f64
}

// ---------------------------------------------------------------------------
// Single-phase diode bridge: source between a and gnd, D1 a->p, D3 gnd->p,
// D2 n->a, D4 n->gnd, load between p and n.

pub const V_RMS: f64 = 100.0;
pub const F_AC: f64 = 50.0;

/// Outputs: U(R1), I(D1), I(D2), I(D3), I(D4).
pub fn bridge(h: f64, end: f64, r: f64, c: Option<f64>, ron: f64) -> Value {
    let vp = V_RMS * 2f64.sqrt();
    let d = |name: &str, a: &str, k: &str| json!({"type": "diode", "name": name, "nodes": [a, k], "on_resistance_ohm": ron, "off_resistance_ohm": ROFF});
    let mut components = vec![
        json!({"type": "voltage_source", "name": "E1", "nodes": ["a", "gnd"], "waveform": {"type": "sine", "amplitude": vp, "frequency_hz": F_AC}}),
        d("D1", "a", "p"),
        d("D2", "n", "a"),
        d("D3", "gnd", "p"),
        d("D4", "n", "gnd"),
        json!({"type": "resistor", "name": "R1", "nodes": ["p", "n"], "resistance_ohm": r}),
    ];
    if let Some(c) = c {
        components.push(
            json!({"type": "capacitor", "name": "C1", "nodes": ["p", "n"], "capacitance_f": c}),
        );
    }
    json!({
        "format": "openkeito-model",
        "version": "0.2",
        "simulation": {"time_step_s": h, "end_time_s": end},
        "components": components,
        "outputs": [
            {"type": "branch_voltage", "component": "R1"},
            {"type": "branch_current", "component": "D1"},
            {"type": "branch_current", "component": "D2"},
            {"type": "branch_current", "component": "D3"},
            {"type": "branch_current", "component": "D4"},
        ],
    })
}

/// Turn-off and turn-on angles (from the zero crossing of the source) and
/// the lowest voltage, relative to the peak, of the capacitor-input
/// rectifier with ideal elements: the diodes stop conducting where
/// C dv/dt + v/R = 0, tan(off) = -w R C; the capacitor then decays as
/// sin(off) exp(-(th - off)/(w R C)) until it meets |sin(th)| again.
pub fn capacitor_input_angles(w: f64, r: f64, c: f64) -> (f64, f64, f64) {
    let wrc = w * r * c;
    let off = std::f64::consts::PI - wrc.atan();
    let gap = |th: f64| off.sin() * (-(th - off) / wrc).exp() - (th - std::f64::consts::PI).sin();
    let (mut lo, mut hi) = (std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if gap(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let on = lo - std::f64::consts::PI;
    (off, on, on.sin())
}
