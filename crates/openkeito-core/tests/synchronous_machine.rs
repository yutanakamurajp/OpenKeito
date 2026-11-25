//! The synchronous machine against textbook values, an independent
//! integration of Park's equations, the classical short-circuit formula and
//! linearized solutions.
//!
//! The machine of most tests is the 555 MVA, 24 kV, 60 Hz generator of
//! Kundur, "Power System Stability and Control", examples 3.1, 3.2 and 4.1.

mod common;

use std::f64::consts::PI;

use common::{diagnostics, run_model as run, Run};
use openkeito_model::Severity;
use serde_json::{json, Value};

const W0: f64 = 2.0 * PI * 60.0;
const S_BASE: f64 = 555e6;
const V_LL: f64 = 24e3;
const Z_BASE: f64 = V_LL * V_LL / S_BASE;

/// Peak of the rated phase current.
fn i_base() -> f64 {
    (2.0_f64 / 3.0).sqrt() * S_BASE / V_LL
}

/// Peak of the rated phase voltage.
fn v_base() -> f64 {
    (2.0_f64 / 3.0).sqrt() * V_LL
}

impl Run {
    /// The values of the column `name`, or of the signal of that name.
    fn column(&self, name: &str) -> Vec<f64> {
        let signal = format!("S({name})");
        let k = self
            .columns
            .iter()
            .position(|n| n == name || *n == signal)
            .unwrap_or_else(|| panic!("no column {name} among {:?}", self.columns));
        self.rows.iter().map(|r| r[k]).collect()
    }

    fn first(&self, name: &str) -> f64 {
        self.column(name)[0]
    }
}

fn codes(model: &Value) -> Vec<&'static str> {
    diagnostics(model)
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .collect()
}

/// The standard parameters of a machine, per unit and seconds.
#[derive(Clone, Copy)]
struct Machine {
    xd: f64,
    xq: f64,
    xd1: f64,
    /// `None`: one damper winding on the q axis.
    xq1: Option<f64>,
    xd2: f64,
    xq2: f64,
    xl: f64,
    ra: f64,
    td01: f64,
    td02: f64,
    tq01: f64,
    tq02: f64,
    h: f64,
    d: f64,
}

/// The generator of Kundur's examples: Ld = 1.81, Lq = 1.76, Ll = 0.15,
/// Ra = 0.003, Ld' = 0.30, Lq' = 0.65, Ld'' = 0.23, Lq'' = 0.25,
/// Td0' = 8.0 s, Td0'' = 0.03 s, Tq0' = 1.0 s, Tq0'' = 0.07 s, H = 3.5.
const KUNDUR: Machine = Machine {
    xd: 1.81,
    xq: 1.76,
    xd1: 0.30,
    xq1: Some(0.65),
    xd2: 0.23,
    xq2: 0.25,
    xl: 0.15,
    ra: 0.003,
    td01: 8.0,
    td02: 0.03,
    tq01: 1.0,
    tq02: 0.07,
    h: 3.5,
    d: 0.0,
};

/// A salient-pole machine with one damper winding on the q axis and very
/// different subtransient reactances of the two axes.
const SALIENT: Machine = Machine {
    xd: 1.1,
    xq: 0.7,
    xd1: 0.35,
    xq1: None,
    xd2: 0.2,
    xq2: 0.3,
    xl: 0.12,
    ra: 0.004,
    td01: 5.0,
    td02: 0.04,
    tq01: 0.0,
    tq02: 0.08,
    h: 4.0,
    d: 0.0,
};

impl Machine {
    /// The component "G1" on bus "b1" with `extra` fields.
    fn component(&self, extra: Value) -> Value {
        let mut c = json!({
            "type": "synchronous_machine", "name": "G1", "bus": "b1",
            "rated_power_mva": S_BASE / 1e6, "rated_voltage_kv": V_LL / 1e3, "frequency_hz": 60.0,
            "xd_pu": self.xd, "xq_pu": self.xq, "xd_transient_pu": self.xd1,
            "xd_subtransient_pu": self.xd2, "xq_subtransient_pu": self.xq2,
            "xl_pu": self.xl, "ra_pu": self.ra,
            "td0_transient_s": self.td01, "td0_subtransient_s": self.td02,
            "tq0_subtransient_s": self.tq02,
            "inertia_h_s": self.h, "damping_pu": self.d,
        });
        if let Some(x) = self.xq1 {
            c["xq_transient_pu"] = json!(x);
            c["tq0_transient_s"] = json!(self.tq01);
        }
        for (k, v) in extra.as_object().expect("an object") {
            c[k] = v.clone();
        }
        c
    }

    /// Leakage inductances and resistances of the rotor windings of the d
    /// axis and of the q axis, by the classical expressions.
    fn windings(&self) -> ([f64; 2], [f64; 2], Vec<f64>, Vec<f64>) {
        let two = |x: f64, x1: f64, x2: f64, t1: f64, t2: f64| {
            let lm = x - self.xl;
            let l1 = lm * (x1 - self.xl) / (x - x1);
            let l2 = (x1 - self.xl) * (x2 - self.xl) / (x1 - x2);
            (
                [l1, l2],
                [(lm + l1) / (W0 * t1), (l2 + x1 - self.xl) / (W0 * t2)],
            )
        };
        let (ld, rd) = two(self.xd, self.xd1, self.xd2, self.td01, self.td02);
        let (lq, rq) = match self.xq1 {
            Some(x1) => {
                let (l, r) = two(self.xq, x1, self.xq2, self.tq01, self.tq02);
                (l.to_vec(), r.to_vec())
            }
            None => {
                let lm = self.xq - self.xl;
                let l1 = lm * (self.xq2 - self.xl) / (self.xq - self.xq2);
                (vec![l1], vec![(lm + l1) / (W0 * self.tq02)])
            }
        };
        (ld, rd, lq, rq)
    }
}

/// Measurement blocks and outputs for the signals `names` of "G1".
fn signals(names: &[&str]) -> (Vec<Value>, Vec<Value>) {
    let controls = names
        .iter()
        .map(|n| {
            json!({"type": "measurement", "name": n,
                   "quantity": {"type": "component_signal", "component": "G1", "signal": n}})
        })
        .collect();
    let outputs = names
        .iter()
        .map(|n| json!({"type": "signal", "signal": n}))
        .collect();
    (controls, outputs)
}

fn model(simulation: Value, components: Vec<Value>, names: &[&str], more: Vec<Value>) -> Value {
    let (mut controls, mut outputs) = signals(names);
    for c in &more {
        outputs.push(json!({"type": "signal", "signal": c["name"]}));
    }
    controls.extend(more);
    outputs.push(json!({"type": "branch_current", "component": "G1"}));
    outputs.push(json!({"type": "bus_voltage", "bus": "b1"}));
    json!({
        "format": "openkeito-model", "version": "0.5",
        "simulation": simulation,
        "components": components,
        "controls": controls,
        "outputs": outputs,
    })
}

/// Solves the n x n system a x = b by Gaussian elimination with pivoting.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for k in 0..n {
        let p = (k..n)
            .max_by(|&i, &j| a[i][k].abs().total_cmp(&a[j][k].abs()))
            .unwrap();
        a.swap(k, p);
        b.swap(k, p);
        for i in k + 1..n {
            let f = a[i][k] / a[k][k];
            for j in k..n {
                a[i][j] -= f * a[k][j];
            }
            b[i] -= f * b[k];
        }
    }
    for k in (0..n).rev() {
        for j in k + 1..n {
            b[k] -= a[k][j] * b[j];
        }
        b[k] /= a[k][k];
    }
    b
}

/// Park's equations of a machine whose terminals are shorted through a
/// resistance, integrated on their own with the classical Runge-Kutta
/// method: the reference of the short-circuit tests. Flux linkages
/// [d, fd, 1d], [q, 1q(, 2q)], the speed and the rotor angle; nothing of it
/// is shared with the solver.
struct Park {
    m: Machine,
    /// Stator resistance with the resistance of the fault, per unit.
    r: f64,
    /// Inductance matrices of the axes, for the vectors [-i_s, i_r...].
    l_d: Vec<Vec<f64>>,
    l_q: Vec<Vec<f64>>,
    r_d: [f64; 2],
    r_q: Vec<f64>,
    /// Field voltage in the reciprocal system.
    e_fd: f64,
}

impl Park {
    fn new(m: Machine, external_resistance: f64) -> Park {
        let (ld, rd, lq, rq) = m.windings();
        let matrix = |lm: f64, leak: &[f64]| {
            let n = leak.len() + 1;
            let mut l = vec![vec![lm; n]; n];
            l[0][0] += m.xl;
            for (k, lk) in leak.iter().enumerate() {
                l[k + 1][k + 1] += lk;
            }
            l
        };
        let lad = m.xd - m.xl;
        Park {
            m,
            r: m.ra + external_resistance,
            l_d: matrix(lad, &ld),
            l_q: matrix(m.xq - m.xl, &lq),
            r_d: rd,
            r_q: rq,
            // No load at rated voltage: i_fd = 1 / L_ad.
            e_fd: rd[0] / lad,
        }
    }

    /// The state at no load and rated voltage: psi_d = 1 from the field
    /// current 1 / L_ad alone.
    fn no_load(&self) -> Vec<f64> {
        let lad = self.m.xd - self.m.xl;
        let i_fd = 1.0 / lad;
        let mut y = vec![1.0, self.l_d[1][1] * i_fd, 1.0];
        y.extend(vec![0.0; self.l_q.len()]);
        y.extend([1.0, 0.0]);
        y
    }

    /// Currents [i_d, i_fd, i_1d] and [i_q, i_1q(, i_2q)] of the fluxes.
    fn currents(&self, y: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let nq = self.l_q.len();
        let mut d = solve_linear(self.l_d.clone(), y[..3].to_vec());
        let mut q = solve_linear(self.l_q.clone(), y[3..3 + nq].to_vec());
        d[0] = -d[0];
        q[0] = -q[0];
        (d, q)
    }

    fn derivative(&self, y: &[f64]) -> Vec<f64> {
        let nq = self.l_q.len();
        let (d, q) = self.currents(y);
        let (psi_d, psi_q) = (y[0], y[3]);
        let w = y[3 + nq];
        let torque = psi_d * q[0] - psi_q * d[0];
        // The terminal voltage of the short circuit is 0 behind `r`.
        let mut f = vec![
            W0 * (w * psi_q + self.r * d[0]),
            W0 * (self.e_fd - self.r_d[0] * d[1]),
            W0 * (-self.r_d[1] * d[2]),
            W0 * (-w * psi_d + self.r * q[0]),
        ];
        for k in 1..nq {
            f.push(W0 * (-self.r_q[k - 1] * q[k]));
        }
        f.push((0.0 - torque - self.m.d * (w - 1.0)) / (2.0 * self.m.h));
        f.push(W0 * (w - 1.0));
        f
    }

    fn step(&self, y: &[f64], dt: f64) -> Vec<f64> {
        let add = |a: &[f64], b: &[f64], f: f64| -> Vec<f64> {
            a.iter().zip(b).map(|(x, k)| x + f * k).collect()
        };
        let k1 = self.derivative(y);
        let k2 = self.derivative(&add(y, &k1, dt / 2.0));
        let k3 = self.derivative(&add(y, &k2, dt / 2.0));
        let k4 = self.derivative(&add(y, &k3, dt));
        (0..y.len())
            .map(|i| y[i] + dt / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]))
            .collect()
    }
}

/// What the short-circuit tests compare, at one time.
#[derive(Clone, Copy, Debug)]
struct Sample {
    /// Current of phase a into the machine, per unit of the peak rated
    /// current.
    ia: f64,
    id: f64,
    iq: f64,
    /// Field current, 1 at no load.
    ifd: f64,
    speed: f64,
    angle: f64,
}

/// A three-phase short circuit at the terminals of the machine at no load
/// and rated voltage, at `t_fault`, through `r_fault` ohm per phase: the run
/// with time step `h`, and the reference at its rows after the fault.
fn short_circuit(m: Machine, h: f64, method: &str) -> (Run, Vec<(usize, Sample)>) {
    let (t_fault, r_fault, end) = (0.01, 2e-3, 0.21);
    let mut components = vec![
        m.component(json!({})),
        json!({"type": "fault", "name": "F1", "bus": "b1", "phases": ["a", "b", "c"], "ground": true,
               "fault_resistance_ohm": r_fault, "ground_resistance_ohm": 1e-3, "start_time_s": t_fault}),
    ];
    for p in ["a", "b", "c"] {
        components.push(json!({"type": "resistor", "name": format!("R{p}"),
            "nodes": [format!("b1.{p}"), "gnd"], "resistance_ohm": 1e6}));
    }
    let run = run(&model(
        json!({"time_step_s": h, "end_time_s": end, "method": method, "initialization": "steady_state"}),
        components,
        &[
            "id_pu",
            "iq_pu",
            "field_current_pu",
            "speed_pu",
            "angle_rad",
        ],
        vec![],
    ));
    // The reference, from the instant of the fault, with a fifth of the
    // smallest time step of the tests (25 us): its error, of the order
    // (w0 dt)^4 = 1e-11, is far below what is compared.
    let park = Park::new(m, r_fault / Z_BASE);
    let lad = m.xd - m.xl;
    let dt = 5e-6;
    let per_row = (h / dt).round() as usize;
    let first = (t_fault / h).round() as usize;
    let nq = park.l_q.len();
    let mut y = park.no_load();
    let mut reference = Vec::new();
    for (row, t) in run.t.iter().enumerate().skip(first) {
        if row > first {
            for _ in 0..per_row {
                y = park.step(&y, dt);
            }
        }
        let (d, q) = park.currents(&y);
        let theta = W0 * t + y[4 + nq] - PI / 2.0;
        reference.push((
            row,
            Sample {
                ia: -(d[0] * theta.cos() - q[0] * theta.sin()),
                id: d[0],
                iq: q[0],
                ifd: lad * d[1],
                speed: y[3 + nq],
                angle: y[4 + nq],
            },
        ));
    }
    (run, reference)
}

/// Largest differences between a short-circuit run and its reference:
/// phase current, dq currents, field current (all per unit), speed and
/// angle.
fn short_circuit_errors(run: &Run, reference: &[(usize, Sample)]) -> [f64; 5] {
    let ia = run.column("I(G1.a)");
    let (id, iq) = (run.column("id_pu"), run.column("iq_pu"));
    let ifd = run.column("field_current_pu");
    let (speed, angle) = (run.column("speed_pu"), run.column("angle_rad"));
    let mut e = [0.0_f64; 5];
    for (row, s) in reference {
        let k = *row;
        e[0] = e[0].max((ia[k] / i_base() - s.ia).abs());
        e[1] = e[1].max((id[k] - s.id).abs().max((iq[k] - s.iq).abs()));
        e[2] = e[2].max((ifd[k] - s.ifd).abs());
        e[3] = e[3].max((speed[k] - s.speed).abs());
        e[4] = e[4].max((angle[k] - s.angle).abs());
    }
    e
}

#[test]
fn the_steady_state_of_the_555_mva_machine_is_that_of_the_textbook() {
    // Kundur, example 3.2: rated output 555 MVA at power factor 0.9
    // (lagging) and rated terminal voltage. The example accounts for
    // saturation with the factor Ksd = Ksq = 0.835 on the mutual
    // inductances Lad = 1.66 and Laq = 1.61, which is the same as a machine
    // with those constant inductances, and prints
    //   delta_i = 39.1 degrees, ed = 0.631, eq = 0.776, id = 0.905,
    //   iq = 0.425, ifd = 1.565, efd = 0.000939, Te = 0.9027 pu
    // (as remembered from the book; every value also follows from the
    // equations of its section 3.6.5, which the loop below evaluates for
    // this case and for the unsaturated machine).
    let (p, q) = (0.9, 0.9 * (0.9_f64).acos().tan());
    let (l_fd, r_fd) = (0.165, 0.0006);
    let printed = [39.1, 0.631, 0.776, 0.905, 0.425, 1.565, 0.000939, 0.9027];
    for (ks, printed) in [(0.835, Some(printed)), (1.0, None)] {
        let (lad, laq) = (ks * 1.66, ks * 1.61);
        let (xd, xq) = (0.15 + lad, 0.15 + laq);
        // The transient reactance and time constant that give the field
        // winding of the example with these mutual inductances.
        let xd1 = 0.15 + lad * l_fd / (lad + l_fd);
        let machine = Machine {
            xd,
            xq,
            xd1,
            td01: (lad + l_fd) / (W0 * r_fd),
            ..KUNDUR
        };
        let names = [
            "angle_rad",
            "vd_pu",
            "vq_pu",
            "id_pu",
            "iq_pu",
            "field_current_pu",
            "field_voltage_pu",
            "electrical_torque_pu",
            "initial_field_voltage_pu",
            "initial_mechanical_torque_pu",
            "active_power_pu",
            "reactive_power_pu",
            "terminal_voltage_pu",
            "speed_pu",
        ];
        let run = run(&model(
            json!({"time_step_s": 5e-5, "end_time_s": 0.02, "initialization": "steady_state"}),
            vec![
                machine.component(json!({"operating_point": {
                    "active_power_mw": p * S_BASE / 1e6, "reactive_power_mvar": q * S_BASE / 1e6,
                    "voltage_pu": 1.0, "angle_deg": 0.0}})),
                json!({"type": "three_phase_source", "name": "INF", "bus": "b1",
                       "line_voltage_rms_v": V_LL, "frequency_hz": 60.0, "angle_deg": 0.0}),
            ],
            &names,
            vec![],
        ));
        // Section 3.6.5 with Et = 1 at angle 0 and It = P - jQ:
        // delta_i = arg(Et + (Ra + j Xq) It), the dq components of both, the
        // field current from psi_d = eq + Ra iq = -Ld id + Lad ifd.
        let ra = machine.ra;
        let (re, im) = (1.0 + ra * p + xq * q, xq * p - ra * q);
        let delta = im.atan2(re);
        let (ed, eq) = (delta.sin(), delta.cos());
        let phi = (q / p).atan();
        let i = p.hypot(q);
        let (id, iq) = (i * (delta + phi).sin(), i * (delta + phi).cos());
        let i_fd = (eq + ra * iq + xd * id) / lad;
        let torque = p + ra * i * i;
        let expected = [
            delta,
            ed,
            eq,
            id,
            iq,
            lad * i_fd,
            lad * i_fd,
            torque,
            lad * i_fd,
            torque,
            p,
            q,
            1.0,
            1.0,
        ];
        for (name, value) in names.iter().zip(expected) {
            // The initial state is computed from the phasor solution: only
            // rounding separates it from the formulas.
            let got = run.first(name);
            assert!((got - value).abs() < 1e-9, "{name}: {got} for {value}");
        }
        if let Some(printed) = printed {
            let got = [
                run.first("angle_rad").to_degrees(),
                run.first("vd_pu"),
                run.first("vq_pu"),
                run.first("id_pu"),
                run.first("iq_pu"),
                // The reciprocal system of the book: ifd and efd = Rfd ifd.
                run.first("field_current_pu") / lad,
                run.first("field_voltage_pu") * r_fd / lad,
                run.first("electrical_torque_pu"),
            ];
            for (g, p) in got.iter().zip(printed) {
                // The book prints three or four digits, of results that it
                // computes from rounded ones.
                assert!((g - p).abs() < 4e-3 * p.abs(), "{g} for the printed {p}");
            }
        }
        // The phase currents are those of the power flow: into the machine,
        // -(P - jQ) per unit on the rms base.
        let ia = run.first("I(G1.a)") / i_base();
        assert!((ia + p).abs() < 1e-7, "{ia}");
        // Against an ideal source at the terminals nothing moves, but for
        // the integration error of the stator: (w0 h)^2 = 3.6e-4 of the
        // voltage across its reactance.
        for (name, value) in names.iter().zip(expected) {
            let last = *run.column(name).last().unwrap();
            assert!((last - value).abs() < 2e-5, "{name} moved to {last}");
        }
    }
}

/// One machine against an infinite bus through a line of 0.15 + j0.5 ohm
/// (about 0.5 per unit), started from the steady state.
fn infinite_bus(m: Machine, h: f64, end: f64, method: &str) -> Run {
    run(&model(
        json!({"time_step_s": h, "end_time_s": end, "method": method,
               "initialization": "steady_state", "output_interval_steps": 20}),
        vec![
            m.component(json!({"operating_point": {"active_power_mw": 450.0,
                "reactive_power_mvar": 150.0, "voltage_pu": 1.0, "angle_deg": 0.0}})),
            json!({"type": "coupled_rl", "name": "LINE", "buses": ["b1", "inf"],
                   "impedance": {"type": "sequence", "r1_ohm": 0.02, "l1_h": 0.0013,
                                 "r0_ohm": 0.06, "l0_h": 0.004}}),
            json!({"type": "three_phase_source", "name": "INF", "bus": "inf",
                   "line_voltage_rms_v": 20000.0, "frequency_hz": 60.0, "angle_deg": -25.0}),
        ],
        &[
            "speed_pu",
            "angle_rad",
            "electrical_torque_pu",
            "terminal_voltage_pu",
        ],
        vec![],
    ))
}

#[test]
fn a_steady_state_stays_where_it_started() {
    // The source of the line is not that of the power flow of the operating
    // point, so the phasor solution differs from it; the machine starts
    // from the solved terminal values, and the run stays there. What is
    // left is the error of the integration of the stator and the line,
    // which is of the order (w0 h)^2 and does not grow.
    for (method, bound) in [("2s-dirk", 0.2), ("trapezoidal", 0.5)] {
        let mut deviations = Vec::new();
        for h in [1e-4, 5e-5] {
            let r = infinite_bus(KUNDUR, h, 3.0, method);
            let s = r.stats.initialization.as_ref().unwrap();
            assert_eq!(s.checked_until_s, 3.0);
            let dv = s.relative_voltage_deviation;
            let di = s.relative_current_deviation.unwrap();
            let order = (W0 * h) * (W0 * h);
            println!("steady {method} h {h}: dv {dv:.3e} di {di:.3e} of (w0 h)^2 = {order:.3e}");
            assert!(
                dv < bound * order && di < bound * order,
                "{method} {h}: {dv} {di}"
            );
            // Speed, angle and torque: the largest departure from the
            // values at t = 0 over the first second and over the last.
            let mut worst = [0.0_f64; 2];
            for name in ["speed_pu", "angle_rad", "electrical_torque_pu"] {
                let c = r.column(name);
                for (t, v) in r.t.iter().zip(&c) {
                    let k = if *t <= 1.0 {
                        0
                    } else if *t >= 2.0 {
                        1
                    } else {
                        continue;
                    };
                    worst[k] = worst[k].max((v - c[0]).abs());
                }
            }
            println!("steady {method} h {h}: speed, angle, torque {worst:?}");
            assert!(worst[0] < bound * order, "{method} {h}: {worst:?}");
            // No drift: the last second is no farther away than the first
            // (the small swing that the integration error starts is damped).
            assert!(worst[1] <= 1.05 * worst[0], "{method} {h}: {worst:?}");
            deviations.push(dv.max(di).max(worst[0]));
        }
        let ratio = deviations[0] / deviations[1];
        assert!((3.5..4.5).contains(&ratio), "{method}: {deviations:?}");
    }
}

#[test]
fn a_terminal_short_circuit_follows_parks_equations() {
    // The same equations with the same parameters, integrated on their own
    // (`Park`). The errors are those of the integration, of second order:
    // bounds of a few (w0 h)^2 times the size of the quantity (currents of
    // up to 9 per unit), and a quarter at half the time step.
    for (machine, method) in [
        (KUNDUR, "2s-dirk"),
        (KUNDUR, "trapezoidal"),
        (SALIENT, "2s-dirk"),
    ] {
        let mut errors = Vec::new();
        for h in [1e-4, 5e-5] {
            let (run, reference) = short_circuit(machine, h, method);
            let e = short_circuit_errors(&run, &reference);
            let order = (W0 * h) * (W0 * h);
            println!(
                "short circuit xq2 {} {method} h {h}: {e:?} (order {order:.3e})",
                machine.xq2
            );
            assert!(e[0] < 3.0 * order, "{method} {h}: phase current {e:?}");
            assert!(e[1] < 3.0 * order, "{method} {h}: dq currents {e:?}");
            assert!(e[2] < 3.0 * order, "{method} {h}: field current {e:?}");
            assert!(e[3] < 0.01 * order, "{method} {h}: speed {e:?}");
            assert!(e[4] < 0.3 * order, "{method} {h}: angle {e:?}");
            // The peak of the reference, so that the comparison is not one
            // of small numbers.
            let peak = reference
                .iter()
                .fold(0.0_f64, |a, (_, s)| a.max(s.ia.abs()));
            assert!(peak > 5.0, "{peak}");
            errors.push(e);
        }
        for k in 0..3 {
            let ratio = errors[0][k] / errors[1][k];
            assert!((3.3..4.7).contains(&ratio), "{method} [{k}]: {errors:?}");
        }
    }
}

#[test]
fn the_short_circuit_current_has_the_classical_envelope() {
    // i_d = E [1/Xd + (1/Xd' - 1/Xd) e^{-t/Td'} + (1/Xd'' - 1/Xd') e^{-t/Td''}]
    //       - (E / Xd'') e^{-t/Ta} cos(w t),
    // i_q = (E / Xq'') e^{-t/Ta} sin(w t)
    // with Td' = Td0' Xd'/Xd, Td'' = Td0'' Xd''/Xd' and
    // Ta = 2 Xd'' Xq'' / ((Xd'' + Xq'') w0 Ra) (Kundur, section 3.7). The
    // formula neglects the resistances in the fluxes, takes the rotor
    // windings one after the other and holds the speed; it is good to a few
    // percent of the initial current E / Xd''. The inertia is made large
    // for it: with H = 3.5 s the losses of the fault slow the rotor by
    // 0.1 rad in 0.2 s, which shifts the phase of the currents.
    let m = Machine { h: 1e4, ..KUNDUR };
    let (run, reference) = short_circuit(m, 5e-5, "2s-dirk");
    let r = m.ra + 2e-3 / Z_BASE;
    let (td1, td2) = (m.td01 * m.xd1 / m.xd, m.td02 * m.xd2 / m.xd1);
    let ta = 2.0 * m.xd2 * m.xq2 / ((m.xd2 + m.xq2) * W0 * r);
    let (id, iq, ia) = (
        run.column("id_pu"),
        run.column("iq_pu"),
        run.column("I(G1.a)"),
    );
    let t0 = run.t[reference[0].0];
    let scale = 1.0 / m.xd2;
    let mut worst = [0.0_f64; 2];
    for (row, _) in &reference {
        let t = run.t[*row] - t0;
        let envelope = 1.0 / m.xd
            + (1.0 / m.xd1 - 1.0 / m.xd) * (-t / td1).exp()
            + (1.0 / m.xd2 - 1.0 / m.xd1) * (-t / td2).exp();
        let d = envelope - (-t / ta).exp() * (W0 * t).cos() / m.xd2;
        let q = (-t / ta).exp() * (W0 * t).sin() / m.xq2;
        // The rotor keeps the angle of no load, 0.
        let theta = W0 * run.t[*row] - PI / 2.0;
        let a = -(d * theta.cos() - q * theta.sin());
        worst[0] = worst[0].max((id[*row] - d).abs().max((iq[*row] - q).abs()));
        worst[1] = worst[1].max((ia[*row] / i_base() - a).abs());
    }
    println!("classical formula: {worst:?} of {scale}");
    assert!(worst[0] < 0.08 * scale, "dq currents {worst:?} of {scale}");
    assert!(
        worst[1] < 0.08 * scale,
        "phase current {worst:?} of {scale}"
    );
    // And it is not a comparison of nothing: the currents reach nearly
    // twice E / Xd''.
    let peak = id.iter().fold(0.0_f64, |a, v| a.max(v.abs()));
    assert!(peak > 1.8 * scale, "{peak}");
}

/// The local extrema of `y` after time `from`: (time, value), the time
/// refined by a parabola through the three samples around each.
fn extrema(t: &[f64], y: &[f64], from: f64) -> Vec<(f64, f64)> {
    let mut found = Vec::new();
    for k in 1..y.len() - 1 {
        let (a, b, c) = (y[k - 1], y[k], y[k + 1]);
        if t[k] > from && (b - a) * (c - b) < 0.0 {
            let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
            found.push((
                t[k] + offset * (t[k + 1] - t[k]),
                b - 0.25 * (a - c) * offset,
            ));
        }
    }
    found
}

#[test]
fn the_swing_against_an_infinite_bus_has_the_linearized_frequency_and_damping() {
    // With rotor time constants of hours the rotor fluxes are constant: the
    // machine is the voltage E'' behind X'' (equal in both axes), and a
    // small disturbance swings as
    //   2 H d2(delta)/dt2 + D d(delta)/dt + K_S w0 delta = 0 (in rad and pu),
    //   K_S = E'' V cos(delta0) / (X'' + Xe),
    // that is, with wn = sqrt(K_S w0 / 2 H), sigma = D / 4 H and the
    // frequency sqrt(wn^2 - sigma^2) (Kundur, section 12.2).
    let m = Machine {
        xd: 1.8,
        xq: 1.7,
        xd1: 0.3,
        xq1: Some(0.5),
        xd2: 0.25,
        xq2: 0.25,
        xl: 0.15,
        ra: 0.0,
        td01: 4e4,
        td02: 2e4,
        tq01: 4e4,
        tq02: 2e4,
        h: 3.5,
        d: 3.0,
    };
    let xe = 0.3;
    let (p, q) = (0.8, 0.3);
    // Terminal voltage 1 at angle 0, current P - jQ: the infinite bus is at
    // V - j Xe I and the internal voltage at V + j X'' I.
    let (inf_re, inf_im) = (1.0 - xe * q, -xe * p);
    let (e_re, e_im) = (1.0 + m.xd2 * q, m.xd2 * p);
    let (v_inf, e) = (inf_re.hypot(inf_im), e_re.hypot(e_im));
    let delta0 = e_im.atan2(e_re) - inf_im.atan2(inf_re);
    let ks = e * v_inf * delta0.cos() / (m.xd2 + xe);
    let wn = (ks * W0 / (2.0 * m.h)).sqrt();
    let sigma = m.d / (4.0 * m.h);
    let wd = (wn * wn - sigma * sigma).sqrt();

    let h = 1e-4;
    let run = run(&model(
        json!({"time_step_s": h, "end_time_s": 4.0, "initialization": "steady_state",
               "output_interval_steps": 10}),
        vec![
            m.component(
                json!({"mechanical_torque_signal": "tm", "operating_point": {
                "active_power_mw": p * S_BASE / 1e6, "reactive_power_mvar": q * S_BASE / 1e6,
                "voltage_pu": 1.0, "angle_deg": 0.0}}),
            ),
            json!({"type": "coupled_rl", "name": "LINE", "buses": ["b1", "inf"],
                   "impedance": {"type": "sequence", "r1_ohm": 0.0, "l1_h": xe * Z_BASE / W0,
                                 "r0_ohm": 0.0, "l0_h": 3.0 * xe * Z_BASE / W0}}),
            json!({"type": "three_phase_source", "name": "INF", "bus": "inf",
                   "line_voltage_rms_v": v_inf * V_LL, "frequency_hz": 60.0,
                   "angle_deg": inf_im.atan2(inf_re).to_degrees()}),
        ],
        &["angle_rad", "speed_pu", "initial_mechanical_torque_pu"],
        vec![
            json!({"type": "step", "name": "more", "time_s": 0.1, "before": 0.0, "after": 0.02}),
            json!({"type": "sum", "name": "tm",
                   "inputs": ["initial_mechanical_torque_pu", "more"], "signs": [1.0, 1.0]}),
        ],
    ));
    let delta = run.column("angle_rad");
    // Until the step nothing moves.
    let before = run.t.iter().position(|t| *t > 0.1).unwrap() - 1;
    // (But for the integration error, as in the test of the steady state.)
    assert!(
        (delta[before] - delta[0]).abs() < 0.05 * (W0 * h) * (W0 * h),
        "{}",
        delta[before] - delta[0]
    );
    let found = extrema(&run.t, &delta, 0.1);
    assert!(found.len() >= 8, "{found:?}");
    // Half periods between successive extrema, and the decay of their
    // distance: three successive extrema e1, e2, e3 around the final value
    // give (e3 - e2) / (e2 - e1) = -exp(-sigma T / 2).
    let n = found.len() - 1;
    let half_period = (found[n].0 - found[0].0) / n as f64;
    let measured_wd = PI / half_period;
    let ratios: Vec<f64> = (0..6)
        .map(|k| -(found[k + 2].1 - found[k + 1].1) / (found[k + 1].1 - found[k].1))
        .collect();
    let mean_ratio = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let measured_sigma = -mean_ratio.ln() / half_period;
    println!("swing: {measured_wd} rad/s for {wd}, {measured_sigma} 1/s for {sigma}");
    // The linearization (the swing is 0.03 rad wide) and the neglected
    // stator and line transients leave a fraction of a percent in the
    // frequency; the damping also has the small contribution of the rotor
    // resistances.
    assert!(
        (measured_wd / wd - 1.0).abs() < 0.01,
        "{measured_wd} rad/s for {wd}"
    );
    assert!(
        (measured_sigma / sigma - 1.0).abs() < 0.05,
        "{measured_sigma} 1/s for {sigma}"
    );
    // The new equilibrium: the torque step over K_S.
    let last = *delta.last().unwrap();
    let settled = found[n - 1].1 + (found[n].1 - found[n - 1].1) / (1.0 + mean_ratio);
    assert!(
        ((settled - delta[0]) / (0.02 / ks) - 1.0).abs() < 0.02,
        "{settled} {last}"
    );
}

/// exp(A t) of a 2 x 2 matrix.
fn exp2(a: [[f64; 2]; 2], t: f64) -> [[f64; 2]; 2] {
    let s = 0.5 * (a[0][0] + a[1][1]);
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let q2 = s * s - det;
    // exp(A t) = e^{s t} [cosh(q t) I + (sinh(q t) / q) (A - s I)].
    let (c, f) = if q2 > 0.0 {
        let q = q2.sqrt();
        ((q * t).cosh(), (q * t).sinh() / q)
    } else if q2 < 0.0 {
        let q = (-q2).sqrt();
        ((q * t).cos(), (q * t).sin() / q)
    } else {
        (1.0, t)
    };
    let e = (s * t).exp();
    [
        [e * (c + f * (a[0][0] - s)), e * f * a[0][1]],
        [e * f * a[1][0], e * (c + f * (a[1][1] - s))],
    ]
}

/// The terminal voltage of the machine `m` at no load after a step of the
/// field voltage, or of the reference of a proportional regulator with gain
/// `ka`, of size `step`, a time `t` after it. On open circuit the d axis is
/// two coupled windings: d(psi_r)/dt = w0 (e_r - R L_rr^-1 psi_r) and
/// v = psi_d = m' L_rr^-1 psi_r, with e_r = (R_fd / L_ad) Efd.
fn open_circuit_voltage(m: Machine, ka: Option<f64>, step: f64, t: f64) -> f64 {
    let (leak, r, _, _) = m.windings();
    let lad = m.xd - m.xl;
    let l = [[lad + leak[0], lad], [lad, lad + leak[1]]];
    let det = l[0][0] * l[1][1] - l[0][1] * l[1][0];
    let gamma = [
        [l[1][1] / det, -l[0][1] / det],
        [-l[1][0] / det, l[0][0] / det],
    ];
    let k = [
        lad * (gamma[0][0] + gamma[1][0]),
        lad * (gamma[0][1] + gamma[1][1]),
    ];
    let mut a = [
        [-W0 * r[0] * gamma[0][0], -W0 * r[0] * gamma[0][1]],
        [-W0 * r[1] * gamma[1][0], -W0 * r[1] * gamma[1][1]],
    ];
    // Input into the field winding per unit of Efd.
    let b = W0 * r[0] / lad;
    let mut input = b * step;
    if let Some(ka) = ka {
        // Efd = Efd0 + ka (step - k' x): the loop closes through the row of
        // the field winding.
        a[0][0] -= b * ka * k[0];
        a[0][1] -= b * ka * k[1];
        input = b * ka * step;
    }
    // x' = A x + [input, 0], x(0) = 0: x = (exp(A t) - I) A^-1 [input, 0].
    let da = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let particular = [a[1][1] * input / da, -a[1][0] * input / da];
    let e = exp2(a, t);
    let x = [
        (e[0][0] - 1.0) * particular[0] + e[0][1] * particular[1],
        e[1][0] * particular[0] + (e[1][1] - 1.0) * particular[1],
    ];
    1.0 + k[0] * x[0] + k[1] * x[1]
}

#[test]
fn the_open_circuit_voltage_follows_the_field_voltage_as_the_d_axis_windings_say() {
    let m = KUNDUR;
    let (h, t_step) = (1e-4, 0.05);
    let open = |controls: Vec<Value>| {
        let mut components = vec![m.component(json!({"field_voltage_signal": "efd"}))];
        for p in ["a", "b", "c"] {
            components.push(json!({"type": "resistor", "name": format!("R{p}"),
                "nodes": [format!("b1.{p}"), "gnd"], "resistance_ohm": 1e7}));
        }
        run(&model(
            json!({"time_step_s": h, "end_time_s": 1.0, "initialization": "steady_state"}),
            components,
            &[
                "terminal_voltage_pu",
                "initial_field_voltage_pu",
                "field_voltage_pu",
                "speed_pu",
            ],
            controls,
        ))
    };
    // A step of the field voltage from its initial value (1 at no load) by
    // 10 %: the voltage rises with Td0' = 8 s, after a delay of the damper
    // winding. The step block changes at the grid point of its time, and
    // the machine follows its signal in a straight line over the next
    // step: a step half a time step later.
    let r = open(vec![
        json!({"type": "step", "name": "more", "time_s": t_step, "before": 0.0, "after": 0.1}),
        json!({"type": "sum", "name": "efd", "inputs": ["initial_field_voltage_pu", "more"],
               "signs": [1.0, 1.0]}),
    ]);
    assert!((r.first("initial_field_voltage_pu") - 1.0).abs() < 1e-12);
    let v = r.column("terminal_voltage_pu");
    let mut worst = 0.0_f64;
    for (t, v) in r.t.iter().zip(&v) {
        let since = t - t_step - 0.5 * h;
        let exact = if since > 0.0 {
            open_circuit_voltage(m, None, 0.1, since)
        } else {
            1.0
        };
        worst = worst.max((v - exact).abs());
    }
    // The voltage moves by 0.011 in the run. The integration error of the
    // rotor fluxes is of the order (h / Td0'')^2 = 1e-5 of that; the load of
    // 1e7 ohm takes 1e-7 per unit of current.
    let moved = v.last().unwrap() - 1.0;
    println!("field step: moved {moved}, error {worst:e}");
    assert!((0.010..0.012).contains(&moved), "{moved}");
    assert!(worst < 2e-7, "{worst}");
    assert!((r.column("speed_pu").last().unwrap() - 1.0).abs() < 1e-9);

    // A proportional regulator, Efd = Efd0 + 50 (Vref - Vt), and a step of
    // its reference by 2 %: the closed loop of the same two windings. The
    // regulator acts one time step late (the signal the machine follows),
    // which moves the response by about h: h times the largest slope of the
    // voltage, 0.02 * 50 * w0 Rfd / Lad * k_fd = 0.1 per second, is 1e-5.
    let ka = 50.0;
    let r = open(vec![
        json!({"type": "initial_value", "name": "vref0", "input": "terminal_voltage_pu"}),
        json!({"type": "step", "name": "dvref", "time_s": t_step, "before": 0.0, "after": 0.02}),
        json!({"type": "sum", "name": "error", "inputs": ["vref0", "dvref", "terminal_voltage_pu"],
               "signs": [1.0, 1.0, -1.0]}),
        json!({"type": "gain", "name": "regulator", "input": "error", "gain": ka}),
        json!({"type": "sum", "name": "efd", "inputs": ["initial_field_voltage_pu", "regulator"],
               "signs": [1.0, 1.0]}),
    ]);
    let v = r.column("terminal_voltage_pu");
    let mut worst = 0.0_f64;
    for (t, v) in r.t.iter().zip(&v) {
        let since = t - t_step - 0.5 * h;
        let exact = if since > 0.0 {
            open_circuit_voltage(m, Some(ka), 0.02, since)
        } else {
            1.0
        };
        worst = worst.max((v - exact).abs());
    }
    let moved = v.last().unwrap() - 1.0;
    println!("regulator step: moved {moved}, error {worst:e}");
    // The loop gain 50 leaves 1 / 51 of the step as error once settled.
    assert!(moved > 0.015 && moved < 0.02, "{moved}");
    assert!(worst < 2e-5, "{worst}");
    // Until the step the regulator holds the initial state exactly.
    let k = (t_step / h) as usize - 1;
    assert!((v[k] - 1.0).abs() < 1e-9, "{}", v[k] - 1.0);
}

/// The machine behind a short line, with a resistive load that three
/// switches connect to its bus at `t_close`.
fn load_pickup(h: f64, switching: &str, t_close: f64) -> Run {
    let mut components = vec![
        KUNDUR.component(json!({"operating_point": {"active_power_mw": 200.0,
            "reactive_power_mvar": 50.0, "voltage_pu": 1.0, "angle_deg": 0.0}})),
        json!({"type": "coupled_rl", "name": "LINE", "buses": ["b1", "b2"],
               "impedance": {"type": "sequence", "r1_ohm": 0.01, "l1_h": 0.0003,
                             "r0_ohm": 0.03, "l0_h": 0.0009}}),
        json!({"type": "three_phase_load", "name": "BASE", "bus": "b2", "rated_voltage_kv": 24.0,
               "rated_frequency_hz": 60.0, "active_power_mw": 200.0, "reactive_power_mvar": 50.0}),
    ];
    for p in ["a", "b", "c"] {
        components.push(json!({"type": "switch", "name": format!("S{p}"),
            "nodes": [format!("b2.{p}"), format!("n.{p}")], "close_time_s": t_close}));
        components.push(json!({"type": "resistor", "name": format!("R{p}"),
            "nodes": [format!("n.{p}"), "gnd"], "resistance_ohm": 2.0}));
    }
    run(&model(
        json!({"time_step_s": h, "end_time_s": 0.03, "initialization": "steady_state",
               "switching": switching}),
        components,
        &["speed_pu", "electrical_torque_pu", "id_pu"],
        vec![],
    ))
}

#[test]
fn a_switch_near_the_machine_closes_within_a_step() {
    // The switches close 0.37 of a time step after a grid point. With
    // "interpolated" the step is split there, and the machine integrates
    // the two parts with their own stage coefficients; with "grid" the
    // load is connected 0.63 h late. The reference is a run with a time
    // step of h / 16 whose grid holds the instant.
    let h = 1.6e-4;
    let t_close = 0.01 + 0.375 * h;
    let reference = load_pickup(h / 16.0, "grid", t_close);
    let mut errors = Vec::new();
    for switching in ["interpolated", "grid"] {
        let r = load_pickup(h, switching, t_close);
        let mut worst = [0.0_f64; 2];
        for (name, k, scale) in [("I(G1.a)", 0, i_base()), ("electrical_torque_pu", 1, 1.0)] {
            let (c, exact) = (r.column(name), reference.column(name));
            for (row, v) in c.iter().enumerate() {
                worst[k] = worst[k].max((v - exact[16 * row]).abs() / scale);
            }
        }
        if switching == "interpolated" {
            assert_eq!(r.stats.split_steps, 1);
        }
        errors.push(worst);
    }
    // The load takes 0.5 per unit of current in a step. Split at the
    // instant, what is left is the integration error, a few (w0 h)^2 =
    // 3.6e-3 times that; at the grid point after it, the current is late by
    // 0.63 h at first, which at the slope of the current of the first
    // steps (the load current over the time constant L'' / R of a few ms)
    // is several times more.
    let order = (W0 * h) * (W0 * h);
    println!(
        "switch: interpolated {:?}, grid {:?}, order {order:e}",
        errors[0], errors[1]
    );
    assert!(errors[0][0] < 2.0 * order, "{errors:?}");
    assert!(errors[0][1] < 2.0 * order, "{errors:?}");
    assert!(errors[1][0] > 3.0 * errors[0][0], "{errors:?}");
}

#[test]
fn a_run_without_initialization_starts_from_the_operating_point() {
    // No load at rated voltage by default: the terminals carry the rated
    // voltage from the first row, with the rotor at angle 0.
    let mut components = vec![KUNDUR.component(json!({}))];
    for p in ["a", "b", "c"] {
        components.push(json!({"type": "resistor", "name": format!("R{p}"),
            "nodes": [format!("b1.{p}"), "gnd"], "resistance_ohm": 1e6}));
    }
    let r = run(&model(
        json!({"time_step_s": 1e-4, "end_time_s": 0.05}),
        components,
        &["terminal_voltage_pu", "speed_pu", "field_voltage_pu"],
        vec![],
    ));
    let (va, vb) = (r.column("V(b1.a)"), r.column("V(b1.b)"));
    // The row of t = 0 is the solution over a negligible interval, in which
    // the currents of the windings (0 at the start) cannot yet be those of
    // the resistors.
    for (k, t) in r.t.iter().enumerate().skip(1) {
        let a = v_base() * (W0 * t).cos();
        let b = v_base() * (W0 * t - 2.0 * PI / 3.0).cos();
        // (w0 h)^2 = 1.4e-3 of the amplitude at most.
        assert!((va[k] - a).abs() < 1e-3 * v_base(), "{t}: {} {a}", va[k]);
        assert!((vb[k] - b).abs() < 1e-3 * v_base(), "{t}: {} {b}", vb[k]);
    }
    assert!((r.first("field_voltage_pu") - 1.0).abs() < 1e-12);
    assert!((r.column("terminal_voltage_pu").last().unwrap() - 1.0).abs() < 1e-5);
}

#[test]
fn an_ungrounded_machine_carries_no_zero_sequence_current() {
    // One resistor from phase a to gnd. With the star point grounded it
    // carries the phase voltage over its resistance; ungrounded, the
    // windings have no return path and the resistor only pulls phase a to
    // gnd. From the steady state (W280: it is unbalanced), so that the
    // windings start with the currents of the resistors.
    let circuit = |neutral: Value| {
        model(
            json!({"time_step_s": 1e-4, "end_time_s": 0.05, "initialization": "steady_state"}),
            vec![
                KUNDUR.component(json!({"neutral": neutral})),
                json!({"type": "resistor", "name": "RA", "nodes": ["b1.a", "gnd"], "resistance_ohm": 100.0}),
                json!({"type": "resistor", "name": "RB", "nodes": ["b1.b", "gnd"], "resistance_ohm": 1e7}),
                json!({"type": "resistor", "name": "RC", "nodes": ["b1.c", "gnd"], "resistance_ohm": 1e7}),
            ],
            &["speed_pu"],
            vec![],
        )
    };
    let peak = |r: &Run, name: &str| r.column(name).iter().fold(0.0_f64, |a, v| a.max(v.abs()));
    let grounded = run(&circuit(json!({"type": "grounded"})));
    let i = peak(&grounded, "I(G1.a)");
    // Stator/zero-sequence impedance (< 1 ohm on 1.04 ohm base) drops < 1%
    // across the 100 ohm (and 1100 ohm below) external resistance.
    assert!((i / (v_base() / 100.0) - 1.0).abs() < 0.01, "{i}");
    let ungrounded = run(&circuit(json!({"type": "ungrounded"})));
    let sum: f64 = ["I(G1.a)", "I(G1.b)", "I(G1.c)"]
        .iter()
        .map(|n| ungrounded.column(n)[100])
        .sum();
    assert!(sum.abs() < 1e-9 * i_base(), "{sum}");
    assert!(
        peak(&ungrounded, "I(G1.a)") < 0.01,
        "{}",
        peak(&ungrounded, "I(G1.a)")
    );
    assert!(peak(&ungrounded, "V(b1.a)") < 1.0);
    // The other phases then carry the line-to-line voltage.
    let vb = peak(&ungrounded, "V(b1.b)");
    assert!(
        (vb / (3.0_f64.sqrt() * v_base()) - 1.0).abs() < 0.01,
        "{vb}"
    );
    // A neutral impedance is in the zero-sequence path three times: with
    // 1000 ohm the current of phase a is the phase voltage over
    // 100 + 1000 ohm (the three phases share 3 * 1000 ohm).
    let impedance = run(&circuit(
        json!({"type": "grounded", "resistance_ohm": 1000.0, "inductance_h": 0.0}),
    ));
    let i = peak(&impedance, "I(G1.a)");
    // Same < 1% stator-impedance voltage drop allowance as the grounded case.
    assert!((i / (v_base() / 1100.0) - 1.0).abs() < 0.01, "{i}");
}

#[test]
fn parameters_out_of_order_are_reported_with_what_to_change() {
    let with = |extra: Value| {
        model(
            json!({"time_step_s": 1e-4, "end_time_s": 0.01}),
            vec![
                KUNDUR.component(extra),
                json!({"type": "resistor", "name": "R1", "nodes": ["b1.a", "gnd"], "resistance_ohm": 1e3}),
            ],
            &[],
            vec![],
        )
    };
    assert_eq!(codes(&with(json!({}))), Vec::<&str>::new());
    // A transient reactance above the synchronous one, a subtransient one
    // below the leakage.
    assert_eq!(codes(&with(json!({"xd_transient_pu": 2.0}))), ["E280"]);
    assert_eq!(codes(&with(json!({"xq_subtransient_pu": 0.1}))), ["E280"]);
    let d = diagnostics(&with(json!({"xd_subtransient_pu": 0.35})));
    assert_eq!(d[0].path, "components[0].xd_pu");
    assert!(d[0]
        .message
        .contains("xd_pu > xd_transient_pu > xd_subtransient_pu > xl_pu"));
    assert!(d[0].hint.contains("machine base"));
    // Time constants: the subtransient one above the transient one.
    assert_eq!(codes(&with(json!({"td0_subtransient_s": 9.0}))), ["E281"]);
    assert_eq!(codes(&with(json!({"tq0_subtransient_s": 0.0}))), ["E281"]);
    let d = diagnostics(&with(json!({"tq0_subtransient_s": 2.0})));
    assert!(d[0].hint.contains("Td0' = Td' Xd / Xd'"));
    // One of the two values of the second q-axis winding.
    let mut one = KUNDUR.component(json!({}));
    one.as_object_mut().unwrap().remove("tq0_transient_s");
    let mut m = with(json!({}));
    m["components"][0] = one;
    assert_eq!(codes(&m), ["E282"]);
    // Ranges.
    assert_eq!(codes(&with(json!({"inertia_h_s": 0.0}))), ["E130"]);
    assert_eq!(codes(&with(json!({"ra_pu": -0.1}))), ["E130"]);
    assert_eq!(
        codes(&with(json!({"operating_point": {"voltage_pu": 0.0}}))),
        ["E130"]
    );
    // An unknown field is an error, as everywhere.
    assert_eq!(codes(&with(json!({"xd_saturated_pu": 1.6}))), ["E001"]);
    // A signal that does not exist.
    assert_eq!(
        codes(&with(json!({"field_voltage_signal": "avr"}))),
        ["E170"]
    );
    // The machine needs format 0.5.
    let mut old = with(json!({}));
    old["version"] = json!("0.4");
    assert_eq!(codes(&old), ["E102"]);
}

#[test]
fn an_unbalanced_steady_state_is_a_warning() {
    // A load on one phase: the phasor solution has a negative sequence,
    // which the rotor does not start with.
    let unbalanced = |ohm: f64| {
        run(&model(
            json!({"time_step_s": 1e-4, "end_time_s": 0.01, "initialization": "steady_state"}),
            vec![
                KUNDUR.component(json!({})),
                json!({"type": "resistor", "name": "RA", "nodes": ["b1.a", "gnd"], "resistance_ohm": ohm}),
                json!({"type": "resistor", "name": "RB", "nodes": ["b1.b", "gnd"], "resistance_ohm": 1e6}),
                json!({"type": "resistor", "name": "RC", "nodes": ["b1.c", "gnd"], "resistance_ohm": 1e6}),
            ],
            &["speed_pu"],
            vec![],
        ))
    };
    let warned = |r: &Run| r.stats.warnings.iter().any(|w| w.code == "W280");
    let r = unbalanced(5.0);
    assert!(warned(&r));
    let w = r.stats.warnings.iter().find(|w| w.code == "W280").unwrap();
    assert_eq!(w.path, "G1");
    assert!(w.hint.contains("positive-sequence"));
    assert!(!warned(&unbalanced(1e6)));
}

#[test]
fn the_stages_settle_in_a_few_solutions() {
    // The number of additional solutions per stage: less than one near a
    // steady state (none once the small transient that the integration
    // error starts has decayed: the first guess of a stage continues the
    // dq currents of the steps before, which then are constant), and
    // during a short circuit a few, more for the machine whose
    // subtransient reactances differ. Plain substitution contracts with
    // |Xd'' - Xq''| / (Xd'' + Xq'' + 2 X of the circuit), 0.04 for the
    // round rotor and 0.2 for the salient-pole machine, which would be 5
    // and 9 solutions per stage; with the sensitivity that the machine
    // learns from its iterations it is 1 to 2.
    let steady = infinite_bus(KUNDUR, 1e-4, 0.5, "2s-dirk");
    let stages = 2 * steady.stats.steps;
    assert!(
        steady.stats.operating_point_iterations < stages,
        "{} of {stages}",
        steady.stats.operating_point_iterations
    );
    for (m, mean_bound, max_bound) in [(KUNDUR, 2.0, 6), (SALIENT, 3.0, 8)] {
        let (run, _) = short_circuit(m, 1e-4, "2s-dirk");
        let stages = 2 * run.stats.steps;
        let mean = run.stats.operating_point_iterations as f64 / stages as f64;
        println!(
            "{mean} per stage, at most {}",
            run.stats.max_operating_point_iterations
        );
        assert!(mean < mean_bound, "{mean} per stage");
        assert!(
            run.stats.max_operating_point_iterations <= max_bound,
            "{}",
            run.stats.max_operating_point_iterations
        );
    }
}
