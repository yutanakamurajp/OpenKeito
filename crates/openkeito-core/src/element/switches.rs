//! Switching elements: the contact that opens at a current zero, the
//! time-controlled switch, the fault and the resistor with two values (diode,
//! controlled switch and thyristor).

use super::{
    add, instant, phasor, stamp_branch_current, stamp_conductance, volt, Element, Node,
    SignalResponse, Stage, SwitchAction, Tolerance,
};
use crate::linalg::{Complex64, Stamp};

/// State of an ideal contact that closes at once and opens either at once
/// or at the first current zero after the command.
#[derive(Debug, Clone, Default)]
pub(crate) struct Breaker {
    pub closed: bool,
    /// Current at the end of the last completed step.
    pub i_last: f64,
    /// Largest |current| since the contact closed.
    pub i_peak: f64,
    /// Sign of the current when the opening command was first seen.
    pub pending_sign: Option<f64>,
    /// (i_last, i_peak) stored by `save`.
    pub saved: (f64, f64),
}

impl Breaker {
    pub fn new(closed: bool) -> Self {
        Breaker {
            closed,
            ..Breaker::default()
        }
    }

    /// Moves towards the commanded state at a grid point; returns the action
    /// taken. An opening waits for a current zero when `at_current_zero`:
    /// the current has changed sign since the command, or is negligible
    /// (below 1e-9 of its peak since closing, or 1e-12 A).
    pub fn command(&mut self, closed: bool, at_current_zero: bool) -> Option<SwitchAction> {
        if closed == self.closed {
            self.pending_sign = None;
            return None;
        }
        if closed {
            self.closed = true;
            self.pending_sign = None;
            self.i_last = 0.0;
            self.i_peak = 0.0;
            return Some(SwitchAction::Close);
        }
        let i = self.i_last;
        let open_now = if at_current_zero {
            let negligible = (1e-9 * self.i_peak).max(1e-12);
            i.abs() <= negligible || self.pending_sign.is_some_and(|s| s * i < 0.0)
        } else {
            true
        };
        if open_now {
            self.closed = false;
            self.pending_sign = None;
            Some(SwitchAction::Open(i))
        } else {
            if self.pending_sign.is_none() {
                self.pending_sign = Some(i.signum());
            }
            None
        }
    }

    /// Records the current at the end of a step.
    pub fn commit(&mut self, i: f64) {
        self.i_last = i;
        self.i_peak = self.i_peak.max(i.abs());
    }

    pub fn save(&mut self) {
        self.saved = (self.i_last, self.i_peak);
    }

    pub fn restore(&mut self) {
        (self.i_last, self.i_peak) = self.saved;
    }

    /// The zero-crossing function of a contact that waits for a current
    /// zero: the current `i`, counted in the direction it had when the
    /// opening was commanded. Infinity for a contact that is not waiting.
    pub fn event_function(&self, i: f64) -> [f64; 2] {
        match self.pending_sign {
            Some(sign) if self.closed => [sign * i, 0.0],
            _ => [f64::INFINITY, 0.0],
        }
    }

    /// Opens the contact at the current zero it waited for; the solver has
    /// brought the circuit there.
    pub fn open_at_zero(&mut self) -> SwitchAction {
        self.closed = false;
        self.pending_sign = None;
        SwitchAction::Open(self.i_last)
    }
}

/// Ideal switch: V(a) - V(b) = 0 when closed, i = 0 when open.
pub(crate) struct Switch {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub k: usize,
    pub initially_closed: bool,
    pub close_time: Option<f64>,
    pub open_time: Option<f64>,
    pub open_at_current_zero: bool,
    pub breaker: Breaker,
}

impl Switch {
    fn commanded(&self, t: f64) -> bool {
        let mut events = [(self.close_time, true), (self.open_time, false)];
        events.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut state = self.initially_closed;
        for (time, closed) in events {
            if let Some(time) = time {
                if time <= t {
                    state = closed;
                }
            }
        }
        state
    }
}

impl Element for Switch {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.a, self.b, self.k, self.breaker.closed);
        if !self.breaker.closed {
            m.add(self.k, self.k, 1.0);
        }
    }
    fn structure(&self, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.a, self.b, self.k, true);
        m.add(self.k, self.k, 1.0);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        x[self.k]
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        if stage.commit {
            self.breaker.commit(x[self.k]);
        }
    }
    fn save(&mut self) {
        self.breaker.save();
    }
    fn restore(&mut self) {
        self.breaker.restore();
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        x[self.k]
    }
    fn init_from_phasor(&mut self, _omega: f64, x: &[Complex64]) {
        // The current at t = 0 and its peak, for an opening at a current zero.
        self.breaker.i_last = instant(x[self.k]);
        self.breaker.i_peak = std::f64::consts::SQRT_2 * x[self.k].norm();
    }
    fn pre_step(&mut self, t: f64, tol: f64, actions: &mut Vec<(String, SwitchAction)>) {
        let command = self.commanded(t + tol);
        if let Some(a) = self.breaker.command(command, self.open_at_current_zero) {
            actions.push((self.name.clone(), a));
        }
    }
    fn waiting_to_open(&self) -> bool {
        self.breaker.pending_sign.is_some()
    }
    fn action_times(&self) -> Vec<f64> {
        self.close_time.into_iter().chain(self.open_time).collect()
    }
    fn event_count(&self) -> usize {
        usize::from(self.open_at_current_zero)
    }
    fn event_functions(&self, x: &[f64], _tol: &Tolerance, g: &mut [[f64; 2]]) {
        g[0] = self.breaker.event_function(x[self.k]);
    }
    fn take_event(&mut self, _k: usize, actions: &mut Vec<(String, SwitchAction)>) {
        actions.push((self.name.clone(), self.breaker.open_at_zero()));
    }
}

/// A fault on a bus: each faulted phase is a contact in series with
/// `r_f` from its node to the fault point; the fault point is gnd (`point`
/// is `None`), or an inner node connected to gnd through `g_ground`, or a
/// floating star point (no ground). Unknowns: one current per phase.
///
/// Closed phase: V(p) - V(F) - r_f i = 0. Open phase: i = 0. While every
/// phase of an ungrounded fault is open, the fault point is held at 0 V, as
/// nothing else determines its voltage.
pub(crate) struct Fault {
    pub name: String,
    /// Event name ("F1.a") and node of each faulted phase.
    pub phases: Vec<(String, Node)>,
    pub point: Node,
    pub r_f: f64,
    /// Conductance from the fault point to gnd (ground through a resistance).
    pub g_ground: Option<f64>,
    pub grounded: bool,
    pub k: usize,
    pub start: f64,
    pub clear: Option<f64>,
    pub clear_at_current_zero: bool,
    pub breakers: Vec<Breaker>,
}

impl Fault {
    fn commanded(&self, t: f64) -> bool {
        t >= self.start && self.clear.is_none_or(|c| t < c)
    }
}

impl Element for Fault {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        self.phases.len()
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn aux_label(&self, k: usize) -> String {
        format!("I({})", self.phases[k].0)
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        for (i, ((_, p), b)) in self.phases.iter().zip(&self.breakers).enumerate() {
            let k = self.k + i;
            stamp_branch_current(m, *p, self.point, k, b.closed);
            if !b.closed {
                m.add(k, k, 1.0);
            } else if self.r_f > 0.0 {
                m.add(k, k, -self.r_f);
            }
        }
        if let Some(g) = self.g_ground {
            add(m, self.point, self.point, g);
        }
        if !self.grounded && self.breakers.iter().all(|b| !b.closed) {
            add(m, self.point, self.point, 1.0);
        }
    }
    fn structure(&self, m: &mut dyn Stamp) {
        for (i, (_, p)) in self.phases.iter().enumerate() {
            let k = self.k + i;
            stamp_branch_current(m, *p, self.point, k, true);
            m.add(k, k, 1.0);
        }
        add(m, self.point, self.point, 1.0);
    }
    fn current(&self, k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        x[self.k + k]
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        if stage.commit {
            for (i, b) in self.breakers.iter_mut().enumerate() {
                b.commit(x[self.k + i]);
            }
        }
    }
    fn save(&mut self) {
        self.breakers.iter_mut().for_each(Breaker::save);
    }
    fn restore(&mut self) {
        self.breakers.iter_mut().for_each(Breaker::restore);
    }
    fn current_phasor(&self, k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        x[self.k + k]
    }
    fn pre_step(&mut self, t: f64, tol: f64, actions: &mut Vec<(String, SwitchAction)>) {
        let command = self.commanded(t + tol);
        for ((name, _), b) in self.phases.iter().zip(self.breakers.iter_mut()) {
            if let Some(a) = b.command(command, self.clear_at_current_zero) {
                actions.push((name.clone(), a));
            }
        }
    }
    fn waiting_to_open(&self) -> bool {
        self.breakers.iter().any(|b| b.pending_sign.is_some())
    }
    fn action_times(&self) -> Vec<f64> {
        std::iter::once(self.start).chain(self.clear).collect()
    }
    fn event_count(&self) -> usize {
        if self.clear_at_current_zero && self.clear.is_some() {
            self.breakers.len()
        } else {
            0
        }
    }
    fn event_functions(&self, x: &[f64], _tol: &Tolerance, g: &mut [[f64; 2]]) {
        for (i, (b, g)) in self.breakers.iter().zip(g.iter_mut()).enumerate() {
            *g = b.event_function(x[self.k + i]);
        }
    }
    fn take_event(&mut self, k: usize, actions: &mut Vec<(String, SwitchAction)>) {
        actions.push((self.phases[k].0.clone(), self.breakers[k].open_at_zero()));
    }
}

/// Resistor with two values: a diode (state follows from the solution), a
/// switch driven by a gate signal, or a thyristor (a diode that may only
/// turn on while its gate signal fires it).
pub(crate) struct TwoState {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub g_on: f64,
    pub g_off: f64,
    pub on: bool,
    /// Index of the gate signal; `None` for a diode.
    pub gate: Option<usize>,
    /// A thyristor: the gate only allows the turn-on (`fired`); otherwise
    /// the state follows from the solution as that of a diode.
    pub latching: bool,
    /// For a thyristor: the gate signal last read was >= 0.5.
    pub fired: bool,
    /// Current at the end of the last completed step.
    pub i_last: f64,
    /// `i_last` stored by `save`.
    pub saved: f64,
    /// What the element blocked and carried, kept when the model leaves a
    /// resistance to its default.
    pub stress: Option<DefaultStress>,
}

/// The largest voltage blocked and the largest current carried by a diode or
/// controlled switch whose resistances are (in part) the defaults, at the
/// ends of the steps of the run so far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultStress {
    /// The model does not give on_resistance_ohm.
    pub default_on: bool,
    /// The model does not give off_resistance_ohm.
    pub default_off: bool,
    pub on_resistance_ohm: f64,
    pub off_resistance_ohm: f64,
    /// Largest |voltage| while blocking, in volt.
    pub blocked_v: f64,
    /// Largest |current| while conducting, in ampere.
    pub carried_a: f64,
}

impl DefaultStress {
    /// `None` when the model gives both resistances.
    pub(crate) fn of(
        on: openkeito_model::TwoStateResistance,
        off: openkeito_model::TwoStateResistance,
    ) -> Option<DefaultStress> {
        (on.is_default() || off.is_default()).then(|| DefaultStress {
            default_on: on.is_default(),
            default_off: off.is_default(),
            on_resistance_ohm: on.ohm(),
            off_resistance_ohm: off.ohm(),
            blocked_v: 0.0,
            carried_a: 0.0,
        })
    }
}

impl TwoState {
    fn g(&self) -> f64 {
        if self.on {
            self.g_on
        } else {
            self.g_off
        }
    }
}

impl Element for TwoState {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, self.g());
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        self.conduction_current(x)
    }
    fn conduction_current(&self, x: &[f64]) -> f64 {
        self.g() * (volt(x, self.a) - volt(x, self.b))
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        if stage.commit {
            self.i_last = self.conduction_current(x);
            if let Some(s) = self.stress.as_mut() {
                if self.on {
                    s.carried_a = s.carried_a.max(self.i_last.abs());
                } else {
                    s.blocked_v = s.blocked_v.max((self.i_last / self.g_off).abs());
                }
            }
        }
    }
    fn default_stress(&self) -> Option<DefaultStress> {
        self.stress
    }
    fn save(&mut self) {
        self.saved = self.i_last;
    }
    fn restore(&mut self) {
        self.i_last = self.saved;
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        self.g() * (phasor(x, self.a) - phasor(x, self.b))
    }
    fn reads_signals(&self) -> bool {
        self.gate.is_some()
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        let Some(gate) = self.gate else {
            return SignalResponse::Unchanged;
        };
        let on = signals[gate] >= 0.5;
        if self.latching {
            // A thyristor fired while it blocks a forward voltage turns on
            // now: the diode logic (`settle`, `inconsistent`) turns it on
            // and records the event. Fired while reverse-biased, it turns on
            // when its voltage becomes positive while the gate still fires
            // it. A change of the gate alone is a changed value: at t = 0,
            // where the voltage before the gate is not known, the states are
            // then settled again.
            let changed = on != self.fired;
            self.fired = on;
            if on && !self.on && self.i_last > 0.0 {
                return SignalResponse::Fired;
            }
            return if changed {
                SignalResponse::ValueChanged
            } else {
                SignalResponse::Unchanged
            };
        }
        if on == self.on {
            return SignalResponse::Unchanged;
        }
        self.on = on;
        SignalResponse::Switched(if on {
            SwitchAction::Close
        } else {
            SwitchAction::Open(self.i_last)
        })
    }
    fn inconsistent(&self, x: &[f64], tol: &Tolerance) -> bool {
        if self.gate.is_some() && !self.latching {
            return false;
        }
        let v = volt(x, self.a) - volt(x, self.b);
        if self.on {
            self.g_on * v < -tol.current
        } else {
            // A thyristor that its gate does not fire keeps blocking.
            (!self.latching || self.fired) && v > tol.voltage
        }
    }
    fn flip(&mut self) {
        self.on = !self.on;
    }
    fn would_switch(&self, signals: &[f64]) -> bool {
        self.gate.is_some_and(|gate| {
            let on = signals[gate] >= 0.5;
            if self.latching {
                // The instants at which the gate starts or stops firing,
                // also while the thyristor conducts: one that turns off
                // later in the same step must find `fired` up to date.
                on != self.fired
            } else {
                on != self.on
            }
        })
    }
    fn event_count(&self) -> usize {
        usize::from(self.gate.is_none() || self.latching)
    }
    fn event_functions(&self, x: &[f64], tol: &Tolerance, g: &mut [[f64; 2]]) {
        // As `inconsistent`: the current while conducting, the reverse
        // voltage while blocking (never crossed by a thyristor that its gate
        // does not fire).
        let v = volt(x, self.a) - volt(x, self.b);
        g[0] = if self.on {
            [self.g_on * v, tol.current]
        } else if self.latching && !self.fired {
            [f64::INFINITY, 0.0]
        } else {
            [-v, tol.voltage]
        };
    }
    fn take_event(&mut self, _k: usize, _actions: &mut Vec<(String, SwitchAction)>) {
        self.flip();
    }
    fn conducting(&self) -> Option<bool> {
        Some(self.on)
    }
}
