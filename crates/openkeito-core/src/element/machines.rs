//! The synchronous machine in the dq axes of its rotor.
//!
//! Machine quantities are in per unit (reciprocal system of Kundur,
//! chapter 3; stator currents of the dq axes in the generator convention,
//! out of the machine), time in seconds:
//!
//! ```text
//! v_d = d(psi_d)/dt / w0 - w psi_q - ra i_d
//! v_q = d(psi_q)/dt / w0 + w psi_d - ra i_q
//! d(psi_r)/dt = w0 (e_r - R_r i_r)                    per axis, rotor windings
//! psi_s = -(l_m + l_l) i_s + m' i_r,  psi_r = L_rr i_r - m i_s
//! Te = psi_d i_q - psi_q i_d
//! 2 H dw/dt = Tm - Te - D (w - 1),  d(delta)/dt = w0 (w - 1)
//! theta = w0 t + delta - pi/2        angle of the d axis from the axis of phase a
//! ```
//!
//! To the circuit the machine is a voltage behind its subtransient
//! reactance. With the rotor currents eliminated, psi_s = k' psi_r - l_sub i_s
//! per axis; with x the mean of the subtransient inductances of the two axes
//! and psi~ = k' psi_r - (l_sub - x) i_s, the stator equations are, in the
//! phases and for the currents into the machine,
//!
//! ```text
//! v_abc = ra i_abc + (x / w0) d(i_abc)/dt + e_abc,
//! e_dq  = d(psi~_dq)/dt / w0 + w J psi~_dq          (J turns by 90 degrees)
//! ```
//!
//! d(psi~)/dt is k' d(psi_r)/dt - (l_sub - x) d(i_s)/dt, with the derivatives
//! of the rotor fluxes and of the phase currents that the stage formula
//! gives for these states (those of the dq currents follow from the phase
//! currents and the speed). They are derivatives of states of differential
//! equations, so the trapezoidal rule does not make them oscillate.
//!
//! The branch ra, x is a constant coupled RL branch in the phases, integrated
//! with the stage formula like every other inductance of the circuit (its
//! zero sequence has x0 and the neutral impedance): a constant conductance
//! and a history current. The rotor fluxes, psi~, the speed and the angle
//! are integrated with the same stage formula in the rotor frame. The
//! voltage e of a stage depends on the dq currents of that stage (through the
//! rotor windings, the difference of the two axes and the torque), so it is
//! evaluated at a guess of them; `update_operating_point` compares the guess
//! with the solution and moves it, and the stage is solved until the machine
//! and the circuit agree (`docs/numerical-method.md`).

use std::cell::Cell;
use std::f64::consts::PI;

use openkeito_model::{self as model, Diagnostic};

use super::{inject, phasor, volt, Element, Followed, Node, OperatingPoint, SignalResponse, Stage};
use crate::linalg::{Complex64, Stamp};
use crate::sim::Stages;

/// Change of the guess of the dq currents, in per unit, below which a stage
/// counts as solved. It is far below the error of the integration.
const TOLERANCE: f64 = 1e-9;
/// The same as a voltage across the subtransient reactance: a change of the
/// currents by d moves the terminal voltages by about (alpha x / w0) d, which
/// is what counts in the short stages at t = 0+.
const VOLTAGE_TOLERANCE: f64 = 1e-7;
/// Below this change, a stage whose change no longer shrinks has reached
/// the rounding of the circuit solution and counts as solved.
const STALL: f64 = 1e-6;
/// The smallest move of the guess from which the sensitivity is learned.
const LEARN: f64 = 1e-7;
/// Unbalance of the steady-state terminal voltages or currents (negative
/// and zero sequence against rated) above which W280 is given.
const UNBALANCE_WARNING: f64 = 1e-4;

const FD: usize = 0;
const D1: usize = 1;
const Q1: usize = 2;
const Q2: usize = 3;
const W: usize = 4;
const DELTA: usize = 5;
/// Phase currents into the machine.
const IA: usize = 6;
const STATES: usize = 9;
const SIGNALS: usize = model::SynchronousMachine::SIGNALS.len();

/// Phase angles of the phases against phase a.
const SHIFT: [f64; 3] = [0.0, -2.0 * PI / 3.0, 2.0 * PI / 3.0];

type Vec2 = [f64; 2];
type Mat2 = [[f64; 2]; 2];

#[inline]
fn dot(a: Vec2, b: Vec2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

#[inline]
fn mul(m: &Mat2, v: Vec2) -> Vec2 {
    [
        m[0][0] * v[0] + m[0][1] * v[1],
        m[1][0] * v[0] + m[1][1] * v[1],
    ]
}

#[inline]
fn inverse(m: &Mat2) -> Mat2 {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    [
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ]
}

/// `(sin, cos)` of `theta + SHIFT[p]` for the three phases.
#[inline]
fn sin_cos_phases(theta: f64) -> [(f64, f64); 3] {
    SHIFT.map(|shift| (theta + shift).sin_cos())
}

/// The dq components of the phase values `v` for precomputed phase `(sin, cos)`
/// pairs (amplitude-invariant, q leading d).
#[inline]
fn park_sc(sc: [(f64, f64); 3], v: [f64; 3]) -> Vec2 {
    let (mut d, mut q) = (0.0, 0.0);
    for p in 0..3 {
        let (s, c) = sc[p];
        d += v[p] * c;
        q -= v[p] * s;
    }
    [2.0 / 3.0 * d, 2.0 / 3.0 * q]
}

/// The phase values of the dq components `x` for precomputed phase `(sin, cos)`
/// pairs.
#[inline]
fn unpark_sc(sc: [(f64, f64); 3], x: Vec2) -> [f64; 3] {
    [0, 1, 2].map(|p| {
        let (s, c) = sc[p];
        x[0] * c - x[1] * s
    })
}

/// The phase values of the dq components `x`.
#[inline]
fn unpark(theta: f64, x: Vec2) -> [f64; 3] {
    unpark_sc(sin_cos_phases(theta), x)
}

#[inline]
fn mean(v: [f64; 3]) -> f64 {
    (v[0] + v[1] + v[2]) / 3.0
}

/// The rotor windings of one axis.
#[derive(Debug, Clone, Copy)]
struct Axis {
    /// Inductance matrix of the rotor windings and its inverse (zero for a
    /// winding that does not exist).
    l_rr: Mat2,
    gamma: Mat2,
    /// Resistances of the rotor windings.
    r: Vec2,
    /// Mutual inductance of each rotor winding with the stator.
    m: Vec2,
    /// gamma m: psi_s = -l_sub i_s + k' psi_r.
    k: Vec2,
    /// Subtransient inductance.
    l_sub: f64,
    /// Synchronous inductance.
    l_sync: f64,
}

impl Axis {
    fn new(w: &model::AxisWindings, leakage: f64) -> Axis {
        let lm = w.mutual_pu;
        let (l_rr, gamma, m) = if w.count == 2 {
            let l = [[lm + w.leakage_pu[0], lm], [lm, lm + w.leakage_pu[1]]];
            (l, inverse(&l), [lm, lm])
        } else {
            let l = lm + w.leakage_pu[0];
            (
                [[l, 0.0], [0.0, 0.0]],
                [[1.0 / l, 0.0], [0.0, 0.0]],
                [lm, 0.0],
            )
        };
        let k = mul(&gamma, m);
        Axis {
            l_rr,
            gamma,
            r: w.resistance_pu,
            m,
            k,
            l_sub: lm + leakage - dot(m, k),
            l_sync: lm + leakage,
        }
    }

    /// The currents of the rotor windings.
    #[inline]
    fn currents(&self, psi_r: Vec2, i_s: f64) -> Vec2 {
        mul(
            &self.gamma,
            [psi_r[0] + self.m[0] * i_s, psi_r[1] + self.m[1] * i_s],
        )
    }

    /// psi_r = L_rr i_r - m i_s.
    #[inline]
    fn fluxes(&self, i_r: Vec2, i_s: f64) -> Vec2 {
        let psi = mul(&self.l_rr, i_r);
        [psi[0] - self.m[0] * i_s, psi[1] - self.m[1] * i_s]
    }

    /// The rotor fluxes of a stage with kappa = w0 / alpha are
    /// psi_r = n (psi_bar + kappa e_r) - b i_s, from
    /// psi_r = psi_bar + kappa (e_r - R gamma (psi_r + m i_s)).
    fn reduced(&self, kappa: f64) -> ReducedAxis {
        let a = [
            [
                1.0 + kappa * self.r[0] * self.gamma[0][0],
                kappa * self.r[0] * self.gamma[0][1],
            ],
            [
                kappa * self.r[1] * self.gamma[1][0],
                1.0 + kappa * self.r[1] * self.gamma[1][1],
            ],
        ];
        let n = inverse(&a);
        let b = mul(
            &n,
            [kappa * self.r[0] * self.k[0], kappa * self.r[1] * self.k[1]],
        );
        ReducedAxis { n, b }
    }
}

#[derive(Debug, Clone, Copy)]
struct ReducedAxis {
    n: Mat2,
    b: Vec2,
}

/// What belongs to a stage coefficient.
#[derive(Debug, Clone, Copy)]
struct Reduced {
    alpha: f64,
    d: ReducedAxis,
    q: ReducedAxis,
    /// The branch ra, x: i = g (v - e) + h i_bar, per unit.
    g: f64,
    h: f64,
    /// The same for the zero sequence (0 for an ungrounded star point).
    g0: f64,
    h0: f64,
}

/// The solution of the machine equations of a stage for a guess of the dq
/// currents.
#[derive(Debug, Clone, Copy)]
struct Eval {
    t: f64,
    alpha: f64,
    c: f64,
    /// The guess: stator currents in dq, generator convention.
    i: Vec2,
    /// The states of the stage, without the phase currents.
    y: [f64; 6],
    torque: f64,
    sc: [(f64, f64); 3],
    field_voltage: f64,
    mechanical_torque: f64,
    /// The voltage behind the branch ra, x in the phases, per unit.
    e: [f64; 3],
}

/// An input of the machine: a control signal, or its value at t = 0.
enum Input {
    Constant(f64),
    Signal(Followed),
}

impl Input {
    #[inline]
    fn at(&self, t: f64) -> f64 {
        match self {
            Input::Constant(v) => *v,
            Input::Signal(f) => f.at(t),
        }
    }

    /// The value at t = 0.
    fn set(&mut self, value: f64) {
        match self {
            Input::Constant(v) => *v = value,
            Input::Signal(f) => f.set(value),
        }
    }
}

/// The dq currents of the last three solutions of one stage of the step
/// (the newest first), for the first guess of the same stage of the next
/// step.
#[derive(Debug, Clone, Copy, Default)]
struct Memory {
    points: [Option<(f64, Vec2)>; 3],
}

pub(crate) struct SynchronousMachine {
    name: String,
    bus: [Node; 3],
    /// Rated angular frequency.
    omega0: f64,
    /// Peak of the rated phase voltage, peak of the rated current, and
    /// their ratio.
    v_base: f64,
    i_base: f64,
    z_base: f64,
    ra: f64,
    /// Mean of the subtransient inductances of the two axes.
    x: f64,
    d: Axis,
    q: Axis,
    /// Field voltage of the reciprocal system per unit of the input (1 pu
    /// of the input gives rated voltage at no load): r_fd / l_ad.
    field_scale: f64,
    inertia_2h: f64,
    damping: f64,
    /// Resistance and reactance of the zero-sequence path, with three times
    /// the neutral impedance; `None` for an ungrounded star point.
    zero: Option<(f64, f64)>,
    /// Internal voltage of phase a (rms, cosine reference) behind ra + j x
    /// in the phasor solution.
    internal: Complex64,
    field_voltage: Input,
    mechanical_torque: Input,
    /// The states at the start of the step and the derivatives of the
    /// previous stage.
    y: [f64; STATES],
    f_prev: [f64; STATES],
    saved: ([f64; STATES], [f64; STATES], [f64; SIGNALS]),
    /// The guess of the dq currents of the stage `pinned` (time, alpha, c).
    guess: Vec2,
    pinned: (f64, f64, f64),
    /// The last change of the guess of that stage.
    last_change: f64,
    /// The dq currents of the last solution with that guess. They are
    /// closer to the solution of the stage than the guess they agree with.
    found: Vec2,
    /// The guess before this one and the dq currents its solution gave.
    tried: Option<(Vec2, Vec2)>,
    /// Estimate of how the dq currents of a solution change with the guess
    /// (2 x 2, learned from the iterations by Broyden's update). It belongs
    /// to the circuit around the machine and the stage coefficient, so it
    /// is kept from stage to stage.
    sensitivity: Mat2,
    /// By the kind of stage: the first of a step (c = 0) and the others.
    memory: [Memory; 2],
    h: f64,
    /// The values of the signals at the last grid point.
    out: [f64; SIGNALS],
    /// Unbalance of the phasor solution the run started from.
    unbalance: Option<f64>,
    reduced: Cell<Option<Reduced>>,
    eval: Cell<Option<Eval>>,
}

impl SynchronousMachine {
    pub fn new(m: &model::SynchronousMachine, bus: [Node; 3], signals: [Option<usize>; 2]) -> Self {
        let w = m.windings();
        let omega0 = 2.0 * PI * m.frequency_hz;
        let v_ll = m.rated_voltage_kv * 1e3;
        let s = m.rated_power_mva * 1e6;
        let z_base = v_ll * v_ll / s;
        let zero = match m.neutral {
            model::MachineNeutral::Grounded {
                resistance_ohm,
                inductance_h,
            } => Some((
                w.resistance_pu + 3.0 * resistance_ohm / z_base,
                m.x0_pu.unwrap_or(m.xl_pu) + 3.0 * omega0 * inductance_h / z_base,
            )),
            model::MachineNeutral::Ungrounded => None,
        };
        let input = |signal: Option<usize>| match signal {
            Some(k) => Input::Signal(Followed::new(k)),
            None => Input::Constant(0.0),
        };
        let d = Axis::new(&w.d, w.leakage_pu);
        let q = Axis::new(&w.q, w.leakage_pu);
        let mut machine = SynchronousMachine {
            name: m.name.clone(),
            bus,
            omega0,
            v_base: (2.0_f64 / 3.0).sqrt() * v_ll,
            i_base: (2.0_f64 / 3.0).sqrt() * s / v_ll,
            z_base,
            ra: w.resistance_pu,
            x: 0.5 * (d.l_sub + q.l_sub),
            d,
            q,
            field_scale: w.d.resistance_pu[0] / w.d.mutual_pu,
            inertia_2h: 2.0 * m.inertia_h_s,
            damping: m.damping_pu,
            zero,
            internal: Complex64::ZERO,
            field_voltage: input(signals[0]),
            mechanical_torque: input(signals[1]),
            y: [0.0; STATES],
            f_prev: [0.0; STATES],
            saved: ([0.0; STATES], [0.0; STATES], [0.0; SIGNALS]),
            guess: [0.0; 2],
            pinned: (f64::NAN, 0.0, 0.0),
            last_change: f64::INFINITY,
            found: [0.0; 2],
            tried: None,
            sensitivity: [[0.0; 2]; 2],
            memory: [Memory::default(); 2],
            h: 1.0,
            out: [0.0; SIGNALS],
            unbalance: None,
            reduced: Cell::new(None),
            eval: Cell::new(None),
        };
        // The operating point of the model: the source of the phasor
        // solution, and the state at t = 0 of a run that does not start
        // from one.
        let o = m.initial_operating_point();
        let v = Complex64::from_polar(o.voltage_pu, o.angle_deg.to_radians());
        let power = Complex64::new(o.active_power_mw, o.reactive_power_mvar) / m.rated_power_mva;
        let i = (power / v).conj();
        machine.internal =
            (v + machine.subtransient_impedance(omega0) * i) * (v_ll / 3.0_f64.sqrt());
        machine.initialize(v, i);
        machine
    }

    /// ra + j x at the angular frequency `omega`, per unit.
    #[inline]
    fn subtransient_impedance(&self, omega: f64) -> Complex64 {
        Complex64::new(self.ra, self.x * omega / self.omega0)
    }

    /// Positive- and zero-sequence admittances of the phasor solution, in
    /// siemens.
    fn phasor_admittances(&self, omega: f64) -> (Complex64, Complex64) {
        let y1 = (self.subtransient_impedance(omega) * self.z_base).inv();
        let y0 = self.zero.map_or(Complex64::ZERO, |(r, x)| {
            (Complex64::new(r, x * omega / self.omega0) * self.z_base).inv()
        });
        (y1, y0)
    }

    /// True for the frequency of the machine: only there it is a source.
    #[inline]
    fn is_rated(&self, omega: f64) -> bool {
        (omega - self.omega0).abs() <= 1e-9 * self.omega0
    }

    /// Internal voltage of phase `p` of the phasor solution.
    #[inline]
    fn internal_phase(&self, p: usize) -> Complex64 {
        self.internal * Complex64::from_polar(1.0, SHIFT[p])
    }

    /// Sets every state to the balanced steady state at rated speed with
    /// the terminal voltage phasor `v` and the current phasor `i` out of
    /// the machine, both per unit (Kundur, section 3.6).
    fn initialize(&mut self, v: Complex64, i: Complex64) {
        // The q axis lies along E = V + (ra + j xq) I.
        let e = v + Complex64::new(self.ra, self.q.l_sync) * i;
        let delta = if e.norm() > 1e-12 { e.arg() } else { v.arg() };
        let theta = delta - PI / 2.0;
        let frame = Complex64::from_polar(1.0, -theta);
        let (vdq, idq) = (v * frame, i * frame);
        let (vd, vq, id, iq) = (vdq.re, vdq.im, idq.re, idq.im);
        let psi_d = vq + self.ra * iq;
        let psi_q = -vd - self.ra * id;
        // Without damper currents, psi_d = -l_d i_d + l_ad i_fd.
        let i_fd = (psi_d + self.d.l_sync * id) / self.d.m[0];
        let field_voltage = self.d.m[0] * i_fd;
        let torque = psi_d * iq - psi_q * id;
        let rotor_d = self.d.fluxes([i_fd, 0.0], id);
        let rotor_q = self.q.fluxes([0.0, 0.0], iq);
        // The currents into the machine and their derivatives: the phases
        // of the dq values (-id, -iq) turning at rated speed.
        let i_abc = unpark(theta, [-id, -iq]);
        let di_abc = unpark(theta, [iq, -id]).map(|x| self.omega0 * x);
        self.y = [
            rotor_d[0], rotor_d[1], rotor_q[0], rotor_q[1], 1.0, delta, i_abc[0], i_abc[1],
            i_abc[2],
        ];
        self.f_prev = [0.0; STATES];
        self.f_prev[IA..].copy_from_slice(&di_abc);
        self.field_voltage.set(field_voltage);
        self.mechanical_torque.set(torque);
        self.guess = [id, iq];
        self.found = [id, iq];
        self.pinned = (f64::NAN, 0.0, 0.0);
        self.memory = [Memory::default(); 2];
        self.eval.set(None);
        self.out = [
            1.0,
            delta,
            torque,
            vd * id + vq * iq,
            vq * id - vd * iq,
            vd.hypot(vq),
            vd,
            vq,
            id,
            iq,
            field_voltage,
            field_voltage,
            torque,
            field_voltage,
            torque,
        ];
    }

    /// What belongs to the stage coefficient `alpha`.
    #[inline]
    fn reduced(&self, alpha: f64) -> Reduced {
        if let Some(r) = self.reduced.get() {
            if r.alpha == alpha {
                return r;
            }
        }
        let kappa = self.omega0 / alpha;
        // (l / w0) (alpha (i - i_start) - c f_prev) = v - e - r i gives
        // i = (kappa (v - e) + l i_bar) / (l + kappa r).
        let branch = |r: f64, l: f64| (kappa / (l + kappa * r), l / (l + kappa * r));
        let (g, h) = branch(self.ra, self.x);
        let (g0, h0) = self.zero.map_or((0.0, 0.0), |(r, x)| branch(r, x));
        let r = Reduced {
            alpha,
            d: self.d.reduced(kappa),
            q: self.q.reduced(kappa),
            g,
            h,
            g0,
            h0,
        };
        self.reduced.set(Some(r));
        r
    }

    #[inline]
    fn slot(stage: &Stage) -> usize {
        usize::from(stage.c != 0.0)
    }

    /// The guess of the dq currents for a stage at time `t`: the one the
    /// iteration of this stage has reached, or the currents of the same
    /// stage of the steps before, continued in a straight line.
    #[inline]
    fn guess(&self, t: f64, stage: &Stage) -> Vec2 {
        if self.pinned == (t, stage.alpha, stage.c) {
            return self.guess;
        }
        // Points one time step apart, the stage one step after the newest:
        // the parabola through three of them, or the line through two.
        let step = |a: f64, b: f64| ((a - b) / self.h - 1.0).abs() < 1e-6;
        match self.memory[Self::slot(stage)].points {
            [Some((t2, u2)), Some((t1, u1)), Some((t0, u0))]
                if step(t, t2) && step(t2, t1) && step(t1, t0) =>
            {
                [0, 1].map(|k| 3.0 * (u2[k] - u1[k]) + u0[k])
            }
            [Some((t2, u2)), Some((t1, u1)), _] if step(t, t2) && step(t2, t1) => {
                [0, 1].map(|k| 2.0 * u2[k] - u1[k])
            }
            [Some((_, u2)), ..] => u2,
            _ => self.guess,
        }
    }

    /// Solves the machine equations of a stage at time `t` for the dq
    /// currents `i` (generator convention).
    fn solve(&self, t: f64, stage: &Stage, red: &Reduced, i: Vec2) -> Eval {
        if let Some(e) = self.eval.get() {
            if e.t == t && e.alpha == stage.alpha && e.c == stage.c && e.i == i {
                return e;
            }
        }
        let kappa = self.omega0 / stage.alpha;
        let ca = stage.c / stage.alpha;
        let bar = |k: usize| self.y[k] + ca * self.f_prev[k];
        let field_voltage = self.field_voltage.at(t);
        let mechanical_torque = self.mechanical_torque.at(t);
        let [id, iq] = i;
        let a_d = mul(
            &red.d.n,
            [bar(FD) + kappa * field_voltage * self.field_scale, bar(D1)],
        );
        let a_q = mul(&red.q.n, [bar(Q1), bar(Q2)]);
        let rotor_d = [a_d[0] - red.d.b[0] * id, a_d[1] - red.d.b[1] * id];
        let rotor_q = [a_q[0] - red.q.b[0] * iq, a_q[1] - red.q.b[1] * iq];
        let (salient_d, salient_q) = (self.d.l_sub - self.x, self.q.l_sub - self.x);
        let pt_d = dot(self.d.k, rotor_d) - salient_d * id;
        let pt_q = dot(self.q.k, rotor_q) - salient_q * iq;
        let torque = pt_d * iq - pt_q * id;
        let inertia = self.inertia_2h * stage.alpha;
        let w = (bar(W) + (mechanical_torque - torque + self.damping) / inertia)
            / (1.0 + self.damping / inertia);
        let delta = bar(DELTA) + kappa * (w - 1.0);
        let theta = self.omega0 * t + delta - PI / 2.0;
        let sc = sin_cos_phases(theta);
        // d(psi~)/dt / w0: the rotor fluxes by the stage formula, and the dq
        // currents from the phase currents of the stage, -i in the rotor
        // frame, whose derivative the stage formula gives in the phases.
        let before = park_sc(sc, [bar(IA), bar(IA + 1), bar(IA + 2)]);
        let did = (id + before[0]) / kappa + w * iq;
        let diq = (iq + before[1]) / kappa - w * id;
        let f_d =
            (dot(self.d.k, [rotor_d[0] - bar(FD), rotor_d[1] - bar(D1)])) / kappa - salient_d * did;
        let f_q =
            (dot(self.q.k, [rotor_q[0] - bar(Q1), rotor_q[1] - bar(Q2)])) / kappa - salient_q * diq;
        let e = Eval {
            t,
            alpha: stage.alpha,
            c: stage.c,
            i,
            y: [rotor_d[0], rotor_d[1], rotor_q[0], rotor_q[1], w, delta],
            torque,
            sc,
            field_voltage,
            mechanical_torque,
            e: unpark_sc(sc, [f_d - w * pt_q, f_q + w * pt_d]),
        };
        self.eval.set(Some(e));
        e
    }

    /// The phase voltages in per unit.
    #[inline]
    fn voltages(&self, x: &[f64]) -> [f64; 3] {
        self.bus.map(|n| volt(x, n) / self.v_base)
    }

    /// The current sources of the phases, per unit: what the currents of a
    /// stage are besides g (v - mean v) + g0 mean v.
    #[inline]
    fn sources(&self, red: &Reduced, e: &Eval, stage: &Stage) -> [f64; 3] {
        let ca = stage.c / stage.alpha;
        let bar = [0, 1, 2].map(|p| self.y[IA + p] + ca * self.f_prev[IA + p]);
        let zero = mean(bar);
        [0, 1, 2].map(|p| red.h * (bar[p] - zero) + red.h0 * zero - red.g * e.e[p])
    }

    /// The phase currents into the machine in the solution `x`, per unit.
    #[inline]
    fn currents(&self, red: &Reduced, e: &Eval, stage: &Stage, x: &[f64]) -> [f64; 3] {
        let v = self.voltages(x);
        let zero = mean(v);
        let j = self.sources(red, e, stage);
        [0, 1, 2].map(|p| red.g * (v[p] - zero) + red.g0 * zero + j[p])
    }
}

/// Adds y1 (I - 1/3) + y0 / 3 between the phases and gnd.
fn stamp_sequences<T: crate::linalg::Scalar>(m: &mut dyn Stamp<T>, bus: &[Node; 3], y1: T, y0: T) {
    let mutual = (y0 - y1).scale(1.0 / 3.0);
    for j in 0..3 {
        for k in 0..3 {
            let v = if j == k { y1 + mutual } else { mutual };
            super::add(m, bus[j], bus[k], v);
        }
    }
}

impl Element for SynchronousMachine {
    fn name(&self) -> &str {
        &self.name
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        self.h = stages.time_step_s;
        for input in [&mut self.field_voltage, &mut self.mechanical_torque] {
            if let Input::Signal(f) = input {
                f.prepare(stages);
            }
        }
        Ok(())
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        let red = self.reduced(alpha);
        stamp_sequences(m, &self.bus, red.g / self.z_base, red.g0 / self.z_base);
    }
    fn structure(&self, m: &mut dyn Stamp) {
        stamp_sequences(m, &self.bus, 2.0, 1.0);
    }
    fn rhs(&self, t: f64, stage: &Stage, b: &mut [f64]) {
        let red = self.reduced(stage.alpha);
        let e = self.solve(t, stage, &red, self.guess(t, stage));
        let j = self.sources(&red, &e, stage);
        for p in 0..3 {
            inject(b, self.bus[p], None, self.i_base * j[p]);
        }
    }
    fn current(&self, k: usize, t: f64, stage: &Stage, x: &[f64]) -> f64 {
        let red = self.reduced(stage.alpha);
        let e = self.solve(t, stage, &red, self.guess(t, stage));
        self.i_base * self.currents(&red, &e, stage, x)[k]
    }
    fn has_operating_point(&self) -> bool {
        true
    }
    fn update_operating_point(&mut self, t: f64, stage: &Stage, x: &[f64]) -> OperatingPoint {
        let guess = self.guess(t, stage);
        let key = (t, stage.alpha, stage.c);
        if self.pinned != key {
            // The guess now belongs to this stage.
            self.pinned = key;
            self.guess = guess;
            self.last_change = f64::INFINITY;
            self.tried = None;
        }
        let red = self.reduced(stage.alpha);
        let e = self.solve(t, stage, &red, guess);
        let i = park_sc(e.sc, self.currents(&red, &e, stage, x));
        let found = [-i[0], -i[1]];
        self.found = found;
        let residual = [found[0] - guess[0], found[1] - guess[1]];
        let change = residual[0].abs().max(residual[1].abs());
        // What the last move of the guess did to the solution corrects the
        // estimate of the sensitivity (when the move is above rounding).
        if let Some((x0, f0)) = self.tried {
            let dx = [guess[0] - x0[0], guess[1] - x0[1]];
            let df = [found[0] - f0[0], found[1] - f0[1]];
            let norm = dot(dx, dx);
            if norm > LEARN * LEARN {
                let predicted = mul(&self.sensitivity, dx);
                for (r, row) in self.sensitivity.iter_mut().enumerate() {
                    let miss = (df[r] - predicted[r]) / norm;
                    row[0] += miss * dx[0];
                    row[1] += miss * dx[1];
                }
            }
        }
        let stalled = change < STALL && change >= 0.5 * self.last_change;
        let kappa = self.omega0 / stage.alpha;
        if change <= TOLERANCE.min(VOLTAGE_TOLERANCE * kappa / self.x) || stalled {
            return OperatingPoint::Consistent;
        }
        if change > self.last_change {
            // The estimate misled: start again from plain substitution.
            self.sensitivity = [[0.0; 2]; 2];
        }
        // The solution is found + B (next - guess) to first order; the next
        // guess is where it equals the guess: (1 - B) (next - guess) =
        // found - guess. Without an estimate (B = 0) it is `found`.
        let b = &self.sensitivity;
        let m = [[1.0 - b[0][0], -b[0][1]], [-b[1][0], 1.0 - b[1][1]]];
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        let step = if det > 0.25 {
            mul(&inverse(&m), residual)
        } else {
            self.sensitivity = [[0.0; 2]; 2];
            residual
        };
        self.tried = Some((guess, found));
        self.last_change = change;
        self.guess = [guess[0] + step[0], guess[1] + step[1]];
        OperatingPoint::RhsChanged
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        // `update_operating_point` precedes every `advance`: the guess and
        // its time are those of this stage.
        let t = self.pinned.0;
        let red = self.reduced(stage.alpha);
        let e = self.solve(t, stage, &red, self.guess);
        let i = self.currents(&red, &e, stage, x);
        let ca = stage.c / stage.alpha;
        let mut y = [0.0; STATES];
        y[..6].copy_from_slice(&e.y);
        y[IA..].copy_from_slice(&i);
        for k in 0..STATES {
            self.f_prev[k] = stage.alpha * (y[k] - self.y[k] - ca * self.f_prev[k]);
        }
        if stage.commit {
            self.y = y;
        }
        // The signals belong to grid points: not to the stages of negligible
        // length at t = 0+, where they keep the values of the initial state.
        if stage.commit && !stage.is_initial() {
            let [vd, vq] = park_sc(e.sc, self.voltages(x));
            let [id, iq] = e.i;
            let i_fd = self.d.currents([y[FD], y[D1]], id)[0];
            self.out[..13].copy_from_slice(&[
                y[W],
                y[DELTA],
                e.torque,
                vd * id + vq * iq,
                vq * id - vd * iq,
                vd.hypot(vq),
                vd,
                vq,
                id,
                iq,
                self.d.m[0] * i_fd,
                e.field_voltage,
                e.mechanical_torque,
            ]);
        }
        let points = &mut self.memory[Self::slot(stage)].points;
        if points[0].is_none_or(|(last, _)| t != last) {
            points.rotate_right(1);
        }
        points[0] = Some((t, self.found));
        self.eval.set(None);
    }
    fn reads_signals(&self) -> bool {
        matches!(self.field_voltage, Input::Signal(_))
            || matches!(self.mechanical_torque, Input::Signal(_))
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        let mut changed = false;
        for input in [&mut self.field_voltage, &mut self.mechanical_torque] {
            if let Input::Signal(f) = input {
                changed |= f.follow(signals);
            }
        }
        self.eval.set(None);
        if changed {
            SignalResponse::ValueChanged
        } else {
            SignalResponse::Unchanged
        }
    }
    fn signal_names(&self) -> Vec<String> {
        model::SynchronousMachine::SIGNALS
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
    fn signal(&self, k: usize) -> f64 {
        self.out[k]
    }
    fn save(&mut self) {
        self.saved = (self.y, self.f_prev, self.out);
    }
    fn restore(&mut self) {
        (self.y, self.f_prev, self.out) = self.saved;
        self.eval.set(None);
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        let (y1, y0) = self.phasor_admittances(omega);
        stamp_sequences(m, &self.bus, y1, y0);
    }
    fn rhs_phasor(&self, omega: f64, b: &mut [Complex64]) {
        if !self.is_rated(omega) {
            return;
        }
        let (y1, _) = self.phasor_admittances(omega);
        for p in 0..3 {
            inject(b, self.bus[p], None, -y1 * self.internal_phase(p));
        }
    }
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        let (y1, y0) = self.phasor_admittances(omega);
        let v = self.bus.map(|n| phasor(x, n));
        let mean = (v[0] + v[1] + v[2]) / 3.0;
        let source = if self.is_rated(omega) {
            y1 * self.internal_phase(k)
        } else {
            Complex64::ZERO
        };
        y1 * (v[k] - mean) + y0 * mean - source
    }
    fn init_from_phasor(&mut self, omega: f64, x: &[Complex64]) {
        let v = self.bus.map(|n| phasor(x, n));
        let i = [0, 1, 2].map(|k| self.current_phasor(k, omega, x));
        let a = Complex64::from_polar(1.0, 2.0 * PI / 3.0);
        // Zero, positive and negative sequence.
        let sequences = |x: &[Complex64; 3]| {
            [
                (x[0] + x[1] + x[2]) / 3.0,
                (x[0] + a * x[1] + a * a * x[2]) / 3.0,
                (x[0] + a * a * x[1] + a * x[2]) / 3.0,
            ]
        };
        // Rms values of the bases.
        let (v_base, i_base) = (
            self.v_base / std::f64::consts::SQRT_2,
            self.i_base / std::f64::consts::SQRT_2,
        );
        let (vs, is) = (sequences(&v), sequences(&i));
        self.initialize(vs[1] / v_base, -is[1] / i_base);
        // The branch ra, x is linear: its currents start on their phasors,
        // whatever their sequences.
        for p in 0..3 {
            let phase = i[p] / i_base;
            self.y[IA + p] = phase.re;
            self.f_prev[IA + p] = (Complex64::new(0.0, omega) * phase).re;
        }
        self.unbalance = Some(
            ((vs[0].norm() + vs[2].norm()) / v_base).max((is[0].norm() + is[2].norm()) / i_base),
        );
    }
    fn steady_state_warning(&self) -> Option<Diagnostic> {
        let unbalance = self.unbalance.filter(|u| *u > UNBALANCE_WARNING)?;
        Some(Diagnostic::warning(
            "W280",
            self.name.clone(),
            format!(
                "the steady state at the terminals of \"{}\" is unbalanced: negative- and \
                 zero-sequence components of {:.2e} per unit",
                self.name, unbalance
            ),
            "The rotor starts from the positive-sequence voltage and current alone. Its \
             response to the negative sequence (currents of twice the frequency in the rotor) \
             is not part of the phasor solution, which takes the machine as a source behind its \
             subtransient impedance, so the run does not start in its steady state. Run until \
             the transient has decayed, or balance the loads and lines around the machine.",
        ))
    }
}
