//! Converters: the averaged model of a grid-following converter.
//!
//! The component is a three-phase voltage source behind its reactor and the
//! control blocks that drive it. `GridFollowingConverter::controls` writes
//! those blocks, and [`expanded`] adds them to the controls of a model: the
//! solver sees an ordinary set of blocks whose signals are named
//! `<component>.<...>`, so outputs and the blocks of the model can read them.

use std::borrow::Cow;
use std::f64::consts::{PI, SQRT_2};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{
    bus_and_neutral, check_bus_and_neutral, ground, neutral_pairs, sequence_matrix, BusRef,
    PhaseMatrix, PHASES,
};
use super::{Component, ComponentInitialization, ComponentSpec, SteadyState};
use crate::catalog::{p, ComponentInfo, BUS};
use crate::check::{positive, range_error};
use crate::controls::{
    AbcToDq0, Constant, Control, Dq0ToAbc, DqLimiter, DqPriority, Gain, Lag, LimitValue, Limiter,
    Measurement, PiDq, Pll, Product, Quantity, Quotient, StartValue, Sum, ThreePhaseMeasurement,
    ThreePhaseQuantity,
};
use crate::diagnostic::Diagnostic;
use crate::model::{Initialization, Model, Simulation};

/// Zero-sequence impedance of the reactor as a multiple of its
/// positive-sequence impedance: a converter with three wires carries no
/// zero-sequence current. 150 per unit for the default reactor is open for
/// every purpose.
const ZERO_SEQUENCE_RATIO: f64 = 1e3;

/// Above this ratio of the time step to the time constant of the current
/// control, the dead time of one step makes the step response overshoot:
/// tau s + e^{-s h} has real roots only up to h / tau = 1 / e.
const DEAD_TIME_RATIO: f64 = 0.36;

/// A reference of a converter: a number, or the name of a control signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, expecting = "a number or the name of a control signal")]
pub enum Setpoint {
    /// A constant.
    Value(f64),
    /// Name of a control signal.
    Signal(String),
}

impl Default for Setpoint {
    fn default() -> Self {
        Setpoint::Value(0.0)
    }
}

impl Setpoint {
    fn value(&self) -> Option<f64> {
        match self {
            Setpoint::Value(v) => Some(*v),
            Setpoint::Signal(_) => None,
        }
    }

    fn signal(&self) -> Option<&str> {
        match self {
            Setpoint::Signal(s) => Some(s),
            Setpoint::Value(_) => None,
        }
    }

    /// The block `name` that gives factor * the reference.
    fn block(&self, name: String, factor: f64) -> Control {
        match self {
            Setpoint::Value(v) => Control::Constant(Constant {
                name,
                value: factor * v,
            }),
            Setpoint::Signal(s) => Control::Gain(Gain {
                name,
                input: s.clone(),
                gain: factor,
            }),
        }
    }
}

/// What a grid-following converter is told to deliver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConverterReference {
    /// Active and reactive power injected into the bus. The current
    /// references are i_d = P / v and i_q = -Q / v in per unit, with v the
    /// d component of the terminal voltage behind a first-order lag.
    Power {
        /// Three-phase active power injected into the bus, in MW: a number
        /// or the name of a signal.
        active_power_mw: Setpoint,
        /// Three-phase reactive power injected into the bus, in Mvar
        /// (positive: the converter acts as a capacitor): a number or the
        /// name of a signal. Default 0.
        #[serde(default)]
        reactive_power_mvar: Setpoint,
        /// Time constant of the lag of the voltage that the powers are
        /// divided by, in seconds; 0 for none. Default 0.02.
        #[serde(default = "default_voltage_filter")]
        voltage_filter_time_constant_s: f64,
        /// Smallest voltage that the powers are divided by, in per unit.
        /// Must be > 0. Default 0.1.
        #[serde(default = "default_minimum_voltage")]
        minimum_voltage_pu: f64,
    },
    /// The currents in the frame of the phase-locked loop, in per unit of
    /// the rated current.
    Current {
        /// Current along the terminal voltage (active current, positive
        /// into the bus): a number or the name of a signal.
        d_pu: Setpoint,
        /// Current 90 degrees ahead of the terminal voltage: a number or
        /// the name of a signal. A positive value absorbs reactive power
        /// (Q = -v i_q). Default 0.
        #[serde(default)]
        q_pu: Setpoint,
    },
}

/// The component of the current that keeps its value at the current limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentPriority {
    /// The active current (d) is limited to current_limit_pu, the reactive
    /// current to what is left.
    Active,
    /// The reactive current (q) is limited to current_limit_pu, the active
    /// current to what is left.
    #[default]
    Reactive,
}

/// The operating point of a converter in the steady-state initialization:
/// the powers it injects at a terminal voltage, as a power flow gives them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConverterOperatingPoint {
    /// Three-phase active power injected into the bus, in MW.
    pub active_power_mw: f64,
    /// Three-phase reactive power injected into the bus, in Mvar.
    #[serde(default)]
    pub reactive_power_mvar: f64,
    /// Line-to-line voltage of the bus, rms, in kV. Must be > 0. When
    /// omitted: rated_voltage_kv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage_kv: Option<f64>,
    /// Angle of the phase-a voltage of the bus (cosine reference). Default 0.
    #[serde(default)]
    pub angle_deg: f64,
}

fn default_voltage_filter() -> f64 {
    0.02
}
fn default_minimum_voltage() -> f64 {
    0.1
}
fn default_reactor_resistance() -> f64 {
    0.005
}
fn default_reactor_reactance() -> f64 {
    0.15
}
fn default_current_limit() -> f64 {
    1.1
}
fn default_current_time_constant() -> f64 {
    1e-3
}
fn default_pll_frequency() -> f64 {
    20.0
}
fn default_pll_damping() -> f64 {
    std::f64::consts::FRAC_1_SQRT_2
}
fn default_modulation_limit() -> f64 {
    1.0
}

/// Averaged model of a grid-following converter (an inverter of a
/// photovoltaic plant, a battery or a wind turbine): a three-phase voltage
/// source behind its reactor, driven by a phase-locked loop on the terminal
/// voltage and a current control in the rotating frame. The switching is not
/// represented, and the dc side is a constant voltage that only limits the
/// voltage the converter can produce.
///
/// Per unit refers to rated_power_mva and rated_voltage_kv: the base of the
/// voltages is the peak phase voltage sqrt(2/3) rated_voltage_kv, that of the
/// currents the peak rated current, so that p = v_d i_d + v_q i_q.
///
/// The phase currents I(<name>.a) etc. flow from the converter into the bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GridFollowingConverter {
    pub name: String,
    pub bus: BusRef,
    /// Star point of the source behind the reactor. Default "gnd". The
    /// converter carries no zero-sequence current.
    #[serde(default = "ground")]
    pub neutral: String,
    /// Rated apparent power in MVA. Must be > 0.
    pub rated_power_mva: f64,
    /// Rated line-to-line voltage, rms, in kV. Must be > 0.
    pub rated_voltage_kv: f64,
    /// Rated frequency in Hz. Must be > 0.
    pub rated_frequency_hz: f64,
    /// Resistance of the reactor in per unit. Must be > 0. Default 0.005.
    #[serde(default = "default_reactor_resistance")]
    pub reactor_resistance_pu: f64,
    /// Reactance of the reactor at the rated frequency in per unit. Must be
    /// > 0. Default 0.15.
    #[serde(default = "default_reactor_reactance")]
    pub reactor_reactance_pu: f64,
    /// What the converter is told to deliver.
    pub reference: ConverterReference,
    /// Largest magnitude of the current reference in per unit. Must be > 0.
    /// Default 1.1.
    #[serde(default = "default_current_limit")]
    pub current_limit_pu: f64,
    /// The current that keeps its value at the limit: "active" or
    /// "reactive" (default).
    #[serde(default)]
    pub priority: CurrentPriority,
    /// Time constant tau of the closed current loop in seconds: the gains
    /// are Kp = L / tau and Ki = R / tau. Must be > 0. Default 1e-3.
    #[serde(default = "default_current_time_constant")]
    pub current_time_constant_s: f64,
    /// Natural frequency of the phase-locked loop in Hz. Must be > 0.
    /// Default 20.
    #[serde(default = "default_pll_frequency")]
    pub pll_natural_frequency_hz: f64,
    /// Damping of the phase-locked loop. Must be > 0. Default 0.707.
    #[serde(default = "default_pll_damping")]
    pub pll_damping: f64,
    /// Time constant of the lag of the terminal voltage that is fed forward
    /// to the voltage reference, in seconds; 0 (default) for none. A lag
    /// lets a jump of the voltage (the clearing of a fault) through to the
    /// current for its time constant.
    #[serde(default)]
    pub voltage_feedforward_time_constant_s: f64,
    /// Voltage of the dc side in kV: the peak phase voltage of the source is
    /// limited to modulation_limit * dc_voltage_kv / 2. Must be > 0. When
    /// omitted: 2.4 times the rated peak phase voltage (a limit of 1.2 per
    /// unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc_voltage_kv: Option<f64>,
    /// Largest peak phase voltage as a multiple of dc_voltage_kv / 2: 1 for
    /// sinusoidal modulation, 1.155 with third-harmonic injection. Must be
    /// > 0. Default 1.
    #[serde(default = "default_modulation_limit")]
    pub modulation_limit: f64,
    /// The operating point in the steady-state initialization. When
    /// omitted: the references, if they are numbers, at the rated voltage
    /// and the angle 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_point: Option<ConverterOperatingPoint>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl GridFollowingConverter {
    /// Peak phase voltage at the rated voltage, in volt: 1 per unit.
    pub fn voltage_base_v(&self) -> f64 {
        (2.0_f64 / 3.0).sqrt() * self.rated_voltage_kv * 1e3
    }

    /// Peak phase current at the rated power and voltage, in ampere: 1 per
    /// unit.
    pub fn current_base_a(&self) -> f64 {
        SQRT_2 * self.rated_power_mva * 1e6 / (3.0_f64.sqrt() * self.rated_voltage_kv * 1e3)
    }

    /// Rated angular frequency in rad/s.
    fn omega(&self) -> f64 {
        2.0 * PI * self.rated_frequency_hz
    }

    /// Resistance in ohm and inductance in henry of a phase of the reactor.
    pub fn reactor(&self) -> (f64, f64) {
        let z_base = self.rated_voltage_kv * self.rated_voltage_kv / self.rated_power_mva;
        (
            self.reactor_resistance_pu * z_base,
            self.reactor_reactance_pu * z_base / self.omega(),
        )
    }

    /// (R, L) phase matrices of the reactor in ohm and henry.
    pub fn reactor_matrices(&self) -> (PhaseMatrix, PhaseMatrix) {
        let (r, l) = self.reactor();
        (
            sequence_matrix(r, ZERO_SEQUENCE_RATIO * r),
            sequence_matrix(l, ZERO_SEQUENCE_RATIO * l),
        )
    }

    /// Largest magnitude of the voltage of the source in per unit.
    pub fn voltage_limit_pu(&self) -> f64 {
        match self.dc_voltage_kv {
            Some(dc) => self.modulation_limit * dc * 1e3 / 2.0 / self.voltage_base_v(),
            None => 1.2 * self.modulation_limit,
        }
    }

    /// The current the converter injects in the steady-state
    /// initialization: rms value in ampere and angle in degrees of the
    /// phasor of phase a (cosine reference). `None` when the model gives no
    /// operating point and the references are signals.
    pub fn steady_current(&self) -> Option<(f64, f64)> {
        if let Some(op) = &self.operating_point {
            let v_kv = op.voltage_kv.unwrap_or(self.rated_voltage_kv);
            return Some(injection(
                op.active_power_mw,
                op.reactive_power_mvar,
                v_kv,
                op.angle_deg,
            ));
        }
        match &self.reference {
            ConverterReference::Power {
                active_power_mw,
                reactive_power_mvar,
                ..
            } => Some(injection(
                active_power_mw.value()?,
                reactive_power_mvar.value()?,
                self.rated_voltage_kv,
                0.0,
            )),
            ConverterReference::Current { d_pu, q_pu } => {
                // d + j q = sqrt(2) X e^{-j theta0} with the angle 0 of the
                // voltage.
                let (d, q) = (d_pu.value()?, q_pu.value()?);
                Some((
                    d.hypot(q) * self.current_base_a() / SQRT_2,
                    q.atan2(d).to_degrees(),
                ))
            }
        }
    }

    /// Names of the signals of the phases [a, b, c] that the source behind
    /// the reactor follows, in volt.
    pub fn source_signals(&self) -> [String; 3] {
        PHASES.map(|ph| format!("{}.e_abc.{ph}", self.name))
    }

    /// The control blocks of the converter for a run with the time step
    /// `time_step_s`. Every block is named `<name>.<...>`.
    pub fn controls(&self, time_step_s: f64) -> Vec<Control> {
        let n = |suffix: &str| format!("{}.{suffix}", self.name);
        let gain = |name: &str, input: &str, gain: f64| {
            Control::Gain(Gain {
                name: n(name),
                input: n(input),
                gain,
            })
        };
        let sum = |name: &str, inputs: [&str; 2], signs: [f64; 2]| {
            Control::Sum(Sum {
                name: n(name),
                inputs: inputs.iter().map(|s| n(s)).collect(),
                signs: signs.to_vec(),
            })
        };
        let product = |name: &str, inputs: [&str; 2]| {
            Control::Product(Product {
                name: n(name),
                inputs: inputs.iter().map(|s| n(s)).collect(),
            })
        };
        let lag = |name: &str, input: &str, time_constant_s: f64| {
            Control::Lag(Lag {
                name: n(name),
                input: n(input),
                gain: 1.0,
                time_constant_s,
                minimum: None,
                maximum: None,
                initial_input: None,
            })
        };
        let (v_base, i_base) = (self.voltage_base_v(), self.current_base_a());
        let omega = self.omega();
        let mut c = Vec::with_capacity(48);

        // The terminal quantities and the frame of the phase-locked loop.
        for ph in PHASES {
            c.push(Control::Measurement(Measurement {
                name: n(&format!("v_{ph}")),
                quantity: Quantity::BranchVoltage {
                    component: self.name.clone(),
                    phase: Some(ph.to_string()),
                },
            }));
        }
        c.push(Control::ThreePhaseMeasurement(ThreePhaseMeasurement {
            name: n("i_abc"),
            quantity: ThreePhaseQuantity::BranchCurrent {
                component: self.name.clone(),
                terminal: None,
            },
        }));
        let omega_n = 2.0 * PI * self.pll_natural_frequency_hz;
        c.push(Control::Pll(Pll {
            name: n("pll"),
            inputs: PHASES.map(|ph| n(&format!("v_{ph}"))),
            nominal_frequency_hz: self.rated_frequency_hz,
            rated_amplitude: v_base,
            proportional_gain: 2.0 * self.pll_damping * omega_n,
            integral_gain_per_s: omega_n * omega_n,
            initial_angle_rad: None,
        }));
        c.push(Control::AbcToDq0(AbcToDq0 {
            name: n("i_dq"),
            inputs: PHASES.map(|ph| n(&format!("i_abc.{ph}"))),
            angle: n("pll.theta"),
        }));
        c.push(gain("v_d", "pll.d", 1.0 / v_base));
        c.push(gain("v_q", "pll.q", 1.0 / v_base));
        c.push(gain("i_d", "i_dq.d", 1.0 / i_base));
        c.push(gain("i_q", "i_dq.q", 1.0 / i_base));

        // The powers injected into the bus, in per unit.
        c.push(product("v_d_i_d", ["v_d", "i_d"]));
        c.push(product("v_q_i_q", ["v_q", "i_q"]));
        c.push(product("v_q_i_d", ["v_q", "i_d"]));
        c.push(product("v_d_i_q", ["v_d", "i_q"]));
        c.push(sum("p", ["v_d_i_d", "v_q_i_q"], [1.0, 1.0]));
        c.push(sum("q", ["v_q_i_d", "v_d_i_q"], [1.0, -1.0]));

        // The current references.
        match &self.reference {
            ConverterReference::Power {
                active_power_mw,
                reactive_power_mvar,
                voltage_filter_time_constant_s,
                minimum_voltage_pu,
            } => {
                c.push(active_power_mw.block(n("p_ref"), 1.0 / self.rated_power_mva));
                c.push(reactive_power_mvar.block(n("q_ref"), 1.0 / self.rated_power_mva));
                let filtered = if *voltage_filter_time_constant_s > 0.0 {
                    c.push(lag("v_filtered", "v_d", *voltage_filter_time_constant_s));
                    "v_filtered"
                } else {
                    "v_d"
                };
                c.push(Control::Limiter(Limiter {
                    name: n("v_divisor"),
                    input: n(filtered),
                    minimum: Some(LimitValue::Value(*minimum_voltage_pu)),
                    maximum: None,
                }));
                let quotient = |name: &str, numerator: &str| {
                    Control::Quotient(Quotient {
                        name: n(name),
                        inputs: [n(numerator), n("v_divisor")],
                        minimum_denominator: None,
                    })
                };
                c.push(quotient("i_d_cmd", "p_ref"));
                c.push(quotient("q_over_v", "q_ref"));
                c.push(gain("i_q_cmd", "q_over_v", -1.0));
            }
            ConverterReference::Current { d_pu, q_pu } => {
                c.push(d_pu.block(n("i_d_cmd"), 1.0));
                c.push(q_pu.block(n("i_q_cmd"), 1.0));
            }
        }
        c.push(Control::DqLimiter(DqLimiter {
            name: n("i_ref"),
            d: n("i_d_cmd"),
            q: n("i_q_cmd"),
            maximum: self.current_limit_pu,
            priority: match self.priority {
                CurrentPriority::Active => DqPriority::D,
                CurrentPriority::Reactive => DqPriority::Q,
            },
        }));

        // The current control: PI on each axis, the terminal voltage fed
        // forward and the voltage across the reactance compensated. In per
        // unit the reactor is (x / omega) di/dt + r i = e - v.
        c.push(sum("i_error_d", ["i_ref.d", "i_d"], [1.0, -1.0]));
        c.push(sum("i_error_q", ["i_ref.q", "i_q"], [1.0, -1.0]));
        let (r, x) = (self.reactor_resistance_pu, self.reactor_reactance_pu);
        let feedforward = if self.voltage_feedforward_time_constant_s > 0.0 {
            let t = self.voltage_feedforward_time_constant_s;
            c.push(lag("v_ff_d", "v_d", t));
            c.push(lag("v_ff_q", "v_q", t));
            ["v_ff_d", "v_ff_q"]
        } else {
            ["v_d", "v_q"]
        };
        c.push(gain("decoupling_d", "i_q", -x));
        c.push(gain("decoupling_q", "i_d", x));
        c.push(sum("ff_d", [feedforward[0], "decoupling_d"], [1.0, 1.0]));
        c.push(sum("ff_q", [feedforward[1], "decoupling_q"], [1.0, 1.0]));
        // In the steady state the integrals hold the voltage across the
        // resistance.
        c.push(gain("r_i_d", "i_d", r));
        c.push(gain("r_i_q", "i_q", r));
        let tau = self.current_time_constant_s;
        c.push(Control::PiDq(PiDq {
            name: n("e"),
            inputs: [n("i_error_d"), n("i_error_q")],
            feedforward: Some([n("ff_d"), n("ff_q")]),
            proportional_gain: x / (omega * tau),
            integral_gain_per_s: r / tau,
            initial_integral: [
                StartValue::Signal(n("r_i_d")),
                StartValue::Signal(n("r_i_q")),
            ],
            output_max: Some(self.voltage_limit_pu()),
        }));

        // Back to phase voltages in volt, with the angle of the next grid
        // point: the source follows its signals one time step late.
        c.push(gain("e_d_v", "e.d", v_base));
        c.push(gain("e_q_v", "e.q", v_base));
        c.push(Control::Constant(Constant {
            name: n("one_step"),
            value: omega * time_step_s,
        }));
        c.push(sum("theta_e", ["pll.theta", "one_step"], [1.0, 1.0]));
        c.push(Control::Dq0ToAbc(Dq0ToAbc {
            name: n("e_abc"),
            d: n("e_d_v"),
            q: n("e_q_v"),
            zero: None,
            angle: n("theta_e"),
        }));
        c
    }
}

/// Rms value in ampere and angle in degrees of the phase current that
/// injects p + j q (MW, Mvar) at the line-to-line voltage `v_kv` with the
/// angle `angle_deg`: I = conj(S / (3 V_phase)).
fn injection(p_mw: f64, q_mvar: f64, v_kv: f64, angle_deg: f64) -> (f64, f64) {
    let rms = p_mw.hypot(q_mvar) * 1e6 / (3.0_f64.sqrt() * v_kv * 1e3);
    (rms, angle_deg - q_mvar.atan2(p_mw).to_degrees())
}

/// The model with the control blocks of its converters added after its own:
/// what the solver builds its blocks and signals from. A model without
/// converters is returned as it is. The blocks of the model keep their
/// indices.
pub fn expanded(model: &Model) -> Cow<'_, Model> {
    let converters = || {
        model.components.iter().filter_map(|c| match c {
            Component::GridFollowingConverter(g) => Some(g),
            _ => None,
        })
    };
    if converters().next().is_none() {
        return Cow::Borrowed(model);
    }
    let mut m = model.clone();
    for g in converters() {
        m.controls.extend(g.controls(model.simulation.time_step_s));
    }
    Cow::Owned(m)
}

/// The converter whose signals a block named `name` would collide with: its
/// blocks are named `<converter>.<...>`.
pub(crate) fn owner_of_block<'a>(model: &'a Model, name: &str) -> Option<&'a str> {
    model.components.iter().find_map(|c| match c {
        Component::GridFollowingConverter(g)
            if name
                .strip_prefix(g.name.as_str())
                .is_some_and(|rest| rest.starts_with('.')) =>
        {
            Some(g.name.as_str())
        }
        _ => None,
    })
}

impl ComponentSpec for GridFollowingConverter {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "grid_following_converter"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn node_names(&self) -> Vec<String> {
        bus_and_neutral(&self.bus, &self.neutral)
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::bus_and_neutral_terminals(&self.bus, &self.neutral)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_bus_and_neutral(&self.name, &self.bus, &self.neutral, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        let must_be_positive = |d: &mut Vec<Diagnostic>, field: &str, v: f64, hint: &str| {
            if !positive(v) {
                range_error(d, p, name, field, v, hint);
            }
        };
        must_be_positive(
            d,
            "rated_power_mva",
            self.rated_power_mva,
            "The rated apparent power must be > 0 MVA.",
        );
        must_be_positive(
            d,
            "rated_voltage_kv",
            self.rated_voltage_kv,
            "The rated line-to-line voltage must be > 0 kV.",
        );
        must_be_positive(
            d,
            "rated_frequency_hz",
            self.rated_frequency_hz,
            "Frequency must be > 0 Hz, for example 50 or 60.",
        );
        must_be_positive(
            d,
            "reactor_resistance_pu",
            self.reactor_resistance_pu,
            "The resistance of the reactor must be > 0 per unit (it sets the integral gain R / tau \
             of the current control), for example 0.005.",
        );
        must_be_positive(
            d,
            "reactor_reactance_pu",
            self.reactor_reactance_pu,
            "The reactance of the reactor must be > 0 per unit, for example 0.15.",
        );
        must_be_positive(
            d,
            "current_limit_pu",
            self.current_limit_pu,
            "The current limit must be > 0 per unit, for example 1.1.",
        );
        must_be_positive(
            d,
            "current_time_constant_s",
            self.current_time_constant_s,
            "The time constant of the current control must be > 0 s, for example 1e-3.",
        );
        must_be_positive(
            d,
            "pll_natural_frequency_hz",
            self.pll_natural_frequency_hz,
            "The natural frequency of the phase-locked loop must be > 0 Hz, for example 20.",
        );
        must_be_positive(
            d,
            "pll_damping",
            self.pll_damping,
            "The damping of the phase-locked loop must be > 0, for example 0.707.",
        );
        must_be_positive(
            d,
            "modulation_limit",
            self.modulation_limit,
            "Use 1 for sinusoidal modulation or 1.155 with third-harmonic injection.",
        );
        if let Some(dc) = self.dc_voltage_kv {
            must_be_positive(
                d,
                "dc_voltage_kv",
                dc,
                "The dc voltage must be > 0 kV; omit the field for a voltage limit of 1.2 per unit.",
            );
        }
        let not_negative = |d: &mut Vec<Diagnostic>, field: &str, v: f64| {
            if !v.is_finite() || v < 0.0 {
                range_error(
                    d,
                    p,
                    name,
                    field,
                    v,
                    "A time constant must be >= 0 s; 0 leaves the lag out.",
                );
            }
        };
        not_negative(
            d,
            "voltage_feedforward_time_constant_s",
            self.voltage_feedforward_time_constant_s,
        );
        let number = |d: &mut Vec<Diagnostic>, field: &str, v: Option<f64>| {
            if v.is_some_and(|v| !v.is_finite()) {
                range_error(
                    d,
                    p,
                    name,
                    field,
                    v.unwrap_or(f64::NAN),
                    "Use a finite number, or the name of a control signal.",
                );
            }
        };
        match &self.reference {
            ConverterReference::Power {
                active_power_mw,
                reactive_power_mvar,
                voltage_filter_time_constant_s,
                minimum_voltage_pu,
            } => {
                number(d, "reference.active_power_mw", active_power_mw.value());
                number(
                    d,
                    "reference.reactive_power_mvar",
                    reactive_power_mvar.value(),
                );
                not_negative(
                    d,
                    "reference.voltage_filter_time_constant_s",
                    *voltage_filter_time_constant_s,
                );
                must_be_positive(
                    d,
                    "reference.minimum_voltage_pu",
                    *minimum_voltage_pu,
                    "The smallest voltage that the powers are divided by must be > 0 per unit, \
                     for example 0.1.",
                );
            }
            ConverterReference::Current { d_pu, q_pu } => {
                number(d, "reference.d_pu", d_pu.value());
                number(d, "reference.q_pu", q_pu.value());
            }
        }
        if let Some(op) = &self.operating_point {
            number(
                d,
                "operating_point.active_power_mw",
                Some(op.active_power_mw),
            );
            number(
                d,
                "operating_point.reactive_power_mvar",
                Some(op.reactive_power_mvar),
            );
            number(d, "operating_point.angle_deg", Some(op.angle_deg));
            if let Some(v) = op.voltage_kv {
                must_be_positive(
                    d,
                    "operating_point.voltage_kv",
                    v,
                    "The line-to-line voltage of the bus at the operating point must be > 0 kV.",
                );
            }
        }
        if d.iter().any(|e| e.path.starts_with(p)) {
            return;
        }
        let limit = self.voltage_limit_pu();
        if limit < 1.0 {
            d.push(Diagnostic::warning(
                "W293",
                format!("{p}.dc_voltage_kv"),
                format!(
                    "\"{name}\" can produce at most {limit:.3} per unit of its rated voltage: \
                     the dc voltage limits it below the voltage of the bus"
                ),
                format!(
                    "The peak phase voltage is limited to modulation_limit * dc_voltage_kv / 2. \
                     For a limit of 1.2 per unit use dc_voltage_kv = {:.4} (or omit the field).",
                    2.4 * self.voltage_base_v() / 1e3 / self.modulation_limit
                ),
            ));
        }
        // An operating point that the constant references do not hold: the
        // run leaves it at once.
        if let (
            Some(op),
            ConverterReference::Power {
                active_power_mw: Setpoint::Value(p_ref),
                reactive_power_mvar: Setpoint::Value(q_ref),
                ..
            },
        ) = (&self.operating_point, &self.reference)
        {
            let differs = |a: f64, b: f64| (a - b).abs() > 0.01 * self.rated_power_mva;
            if differs(op.active_power_mw, *p_ref) || differs(op.reactive_power_mvar, *q_ref) {
                d.push(Diagnostic::warning(
                    "W292",
                    format!("{p}.operating_point"),
                    format!(
                        "the operating point of \"{name}\" ({} MW, {} Mvar) is not its reference \
                         ({p_ref} MW, {q_ref} Mvar): the converter moves to the reference from \
                         t = 0 on",
                        op.active_power_mw, op.reactive_power_mvar
                    ),
                    "Give the operating point the powers of the reference, or omit its powers' \
                     difference on purpose: the phasor solution uses the operating point, the \
                     run the reference. For a change during the run, make the reference a signal \
                     (a step block).",
                ));
            }
        }
    }
    fn check_simulation(&self, sim: &Simulation, p: &str, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        let (h, tau) = (sim.time_step_s, self.current_time_constant_s);
        if positive(tau) && h / tau > DEAD_TIME_RATIO {
            d.push(Diagnostic::warning(
                "W290",
                format!("{p}.current_time_constant_s"),
                format!(
                    "the time step ({h:e} s) is {:.2} of the time constant of the current \
                     control of \"{name}\" ({tau:e} s): the dead time of one step makes the \
                     current overshoot",
                    h / tau
                ),
                "The controls reach the circuit one time step late. Keep time_step_s below 0.2 \
                 current_time_constant_s for a response close to the first-order lag; above \
                 0.37 it overshoots, and from 1.57 on the loop is unstable. Reduce time_step_s \
                 or raise current_time_constant_s.",
            ));
        }
        if sim.initialization == Initialization::SteadyState && self.steady_current().is_none() {
            d.push(Diagnostic::warning(
                "W291",
                p.to_string(),
                format!(
                    "\"{name}\" has no operating_point and its references are signals: it \
                     injects no current in the phasor solution"
                ),
                "Give \"operating_point\" the powers the converter injects at t = 0 and the \
                 voltage and angle of its bus, as a power flow gives them. If the converter \
                 starts without current, the warning can be ignored.",
            ));
        }
    }
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        // Ratings that are wrong are reported on their own.
        let ratings = [
            self.rated_power_mva,
            self.rated_voltage_kv,
            self.rated_frequency_hz,
            self.reactor_resistance_pu,
        ];
        if !ratings.into_iter().all(positive) {
            return None;
        }
        let (r, l) = self.reactor_matrices();
        Some((r, l, "reactor_reactance_pu"))
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        fn named<'a>(field: &'static str, s: &'a Setpoint) -> Option<(&'static str, &'a str)> {
            s.signal().map(|s| (field, s))
        }
        match &self.reference {
            ConverterReference::Power {
                active_power_mw,
                reactive_power_mvar,
                ..
            } => [
                named("reference.active_power_mw", active_power_mw),
                named("reference.reactive_power_mvar", reactive_power_mvar),
            ],
            ConverterReference::Current { d_pu, q_pu } => {
                [named("reference.d_pu", d_pu), named("reference.q_pu", q_pu)]
            }
        }
        .into_iter()
        .flatten()
        .collect()
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::Sine(self.rated_frequency_hz)
    }
    fn sine_frequency(&self) -> Option<f64> {
        Some(self.rated_frequency_hz)
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "grid_following_converter",
            category: "circuit",
            summary: "Averaged grid-following converter (inverter of a photovoltaic plant, a battery, a wind turbine): a three-phase voltage source behind its reactor, with a phase-locked loop on the terminal voltage, a current limit with priority and a PI current control in the rotating frame (decoupled axes, voltage feedforward, voltage limit of the dc side). No switching; the dc voltage is constant. Its control blocks are the signals <name>.<...>: pll.theta, pll.frequency_hz, v_d, v_q, i_d, i_q, p, q, i_d_cmd, i_q_cmd, i_ref.d, i_ref.q, e.d, e.q (per unit of the ratings) and more; list them in outputs or read them in control blocks.",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the converter into the bus (as those of a three_phase_current_source); branch voltages U(<name>.a) are V(bus.p) - V(neutral). Powers are injected into the bus; positive reactive power is that of a capacitor. Per unit: voltages on the peak phase voltage sqrt(2/3) rated_voltage_kv, currents on the peak rated current, so p = v_d i_d + v_q i_q and q = v_q i_d - v_d i_q. The d axis is the terminal voltage (v_q = 0 when the loop is locked), the q axis 90 degrees ahead.",
            parameters: vec![
                p("name", "", true, "Unique name: letters, digits, '_', '-', '.'. Also the prefix of the names of its control signals."),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node of the star point of the source behind the reactor. Default \"gnd\". The converter carries no zero-sequence current."),
                p("rated_power_mva", "MVA", true, "Rated apparent power, > 0: the base of the per-unit values."),
                p("rated_voltage_kv", "kV", true, "Rated line-to-line voltage, rms, > 0."),
                p("rated_frequency_hz", "Hz", true, "Rated frequency, > 0."),
                p("reactor_resistance_pu", "pu", false, "Resistance of the reactor, > 0. Default 0.005."),
                p("reactor_reactance_pu", "pu", false, "Reactance of the reactor at the rated frequency, > 0. Default 0.15."),
                p("reference", "MW, Mvar or pu", true, "{\"type\":\"power\",\"active_power_mw\":..,\"reactive_power_mvar\":..,\"voltage_filter_time_constant_s\":..,\"minimum_voltage_pu\":..} (powers injected into the bus; i_d = P/v, i_q = -Q/v with v the d component of the terminal voltage behind a lag of voltage_filter_time_constant_s, default 0.02 s, and at least minimum_voltage_pu, default 0.1) | {\"type\":\"current\",\"d_pu\":..,\"q_pu\":..} (currents in the frame of the phase-locked loop). Each of the powers and currents is a number or the name of a control signal; reactive_power_mvar and q_pu default to 0."),
                p("current_limit_pu", "pu", false, "Largest magnitude of the current reference, > 0. Default 1.1."),
                p("priority", "", false, "active | reactive: the current that keeps its value at the limit; the other one gets sqrt(limit^2 - it^2). Default reactive."),
                p("current_time_constant_s", "s", false, "Time constant tau of the closed current loop, > 0: Kp = L/tau, Ki = R/tau. Keep time_step_s below 0.2 tau. Default 1e-3."),
                p("pll_natural_frequency_hz", "Hz", false, "Natural frequency of the phase-locked loop, > 0. Default 20."),
                p("pll_damping", "", false, "Damping of the phase-locked loop, > 0. Default 0.707."),
                p("voltage_feedforward_time_constant_s", "s", false, "Time constant of the lag of the terminal voltage fed forward to the voltage reference; 0 for none. Default 0."),
                p("dc_voltage_kv", "kV", false, "Voltage of the dc side, > 0: the peak phase voltage of the source is limited to modulation_limit * dc_voltage_kv / 2. When omitted: a limit of 1.2 per unit."),
                p("modulation_limit", "", false, "Largest peak phase voltage as a multiple of dc_voltage_kv / 2: 1 for sinusoidal modulation, 1.155 with third-harmonic injection. Default 1."),
                p("operating_point", "MW, Mvar, kV, deg", false, "{\"active_power_mw\":..,\"reactive_power_mvar\":..,\"voltage_kv\":..,\"angle_deg\":..}. The powers injected at t = 0 and the line-to-line voltage and angle of the bus, as a power flow gives them: in the steady-state initialization the converter injects the current conj(S / (3 V)). When omitted: the references, if they are numbers, at the rated voltage and the angle 0 (otherwise no current, warning W291)."),
            ],
            example: json!({"type": "grid_following_converter", "name": "PV1", "bus": "b3", "neutral": "gnd",
                "rated_power_mva": 2.0, "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
                "reactor_resistance_pu": 0.005, "reactor_reactance_pu": 0.15,
                "reference": {"type": "power", "active_power_mw": 1.5, "reactive_power_mvar": "q_command",
                    "voltage_filter_time_constant_s": 0.02, "minimum_voltage_pu": 0.1},
                "current_limit_pu": 1.1, "priority": "reactive", "current_time_constant_s": 0.001,
                "pll_natural_frequency_hz": 20.0, "pll_damping": 0.707,
                "voltage_feedforward_time_constant_s": 0.0, "dc_voltage_kv": 12.0, "modulation_limit": 1.155,
                "operating_point": {"active_power_mw": 1.5, "reactive_power_mvar": 0.2, "voltage_kv": 6.55, "angle_deg": -3.2}}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{evaluation_plan, signal_names};

    fn converter(extra: serde_json::Value) -> GridFollowingConverter {
        let mut v = json!({"name": "PV1", "bus": "b1", "rated_power_mva": 2.0, "rated_voltage_kv": 6.6,
            "rated_frequency_hz": 50.0,
            "reference": {"type": "power", "active_power_mw": 1.5, "reactive_power_mvar": 0.5}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_bases_give_the_rated_power_at_one_per_unit() {
        let c = converter(json!({}));
        // 3/2 of the product of the peak values is the three-phase power.
        let s = 1.5 * c.voltage_base_v() * c.current_base_a();
        assert!((s - 2.0e6).abs() < 1e-6, "{s}");
        // 0.15 per unit of 6.6^2 / 2 ohm at 50 Hz.
        let (r, l) = c.reactor();
        assert!((r - 0.005 * 21.78).abs() < 1e-12);
        assert!((l - 0.15 * 21.78 / (100.0 * PI)).abs() < 1e-15);
        assert!((c.voltage_limit_pu() - 1.2).abs() < 1e-15);
        let c = converter(json!({"dc_voltage_kv": 12.0, "modulation_limit": 1.155}));
        let limit = 1.155 * 6000.0 / c.voltage_base_v();
        assert!((c.voltage_limit_pu() - limit).abs() < 1e-12);
    }

    #[test]
    fn the_steady_current_is_that_of_the_powers_at_the_voltage() {
        // 1.5 MW + j 0.5 Mvar at 6.6 kV and the angle 0: I = conj(S / 3V),
        // |I| = |S| / (sqrt(3) V), behind the voltage by atan(Q / P).
        let (rms, angle) = converter(json!({})).steady_current().unwrap();
        let s = (1.5_f64 * 1.5 + 0.25).sqrt() * 1e6;
        assert!((rms - s / (3.0_f64.sqrt() * 6600.0)).abs() < 1e-9);
        assert!((angle + (0.5_f64 / 1.5).atan().to_degrees()).abs() < 1e-12);
        // The operating point takes precedence over the references.
        let c = converter(json!({"operating_point":
            {"active_power_mw": -1.0, "voltage_kv": 6.0, "angle_deg": 10.0}}));
        let (rms, angle) = c.steady_current().unwrap();
        assert!((rms - 1e6 / (3.0_f64.sqrt() * 6000.0)).abs() < 1e-9);
        assert!((angle - 190.0).abs() < 1e-12 || (angle + 170.0).abs() < 1e-12);
        // A current reference: d + j q in the frame of the voltage.
        let c = converter(json!({"reference": {"type": "current", "d_pu": 0.0, "q_pu": -0.5}}));
        let (rms, angle) = c.steady_current().unwrap();
        assert!((rms - 0.5 * c.current_base_a() / SQRT_2).abs() < 1e-9);
        assert!((angle + 90.0).abs() < 1e-12);
        // A signal leaves the current unknown.
        let c = converter(json!({"reference": {"type": "power", "active_power_mw": "p_cmd"}}));
        assert_eq!(c.steady_current(), None);
    }

    #[test]
    fn the_blocks_carry_the_name_and_have_an_order() {
        let c = converter(json!({}));
        let blocks = c.controls(5e-5);
        assert!(blocks.iter().all(|b| b.name().starts_with("PV1.")));
        // No loop among them: the loop closes through the circuit.
        let plan = evaluation_plan(&blocks, 5e-5).unwrap();
        assert!(plan.iter().all(|g| !g.iterate));
        let names = signal_names(&blocks);
        for s in [
            "PV1.pll.theta",
            "PV1.pll.frequency_hz",
            "PV1.v_d",
            "PV1.v_q",
            "PV1.i_d",
            "PV1.i_q",
            "PV1.p",
            "PV1.q",
            "PV1.p_ref",
            "PV1.q_ref",
            "PV1.i_d_cmd",
            "PV1.i_q_cmd",
            "PV1.i_ref.d",
            "PV1.i_ref.q",
            "PV1.e.d",
            "PV1.e.q",
        ] {
            assert!(names.iter().any(|n| n == s), "{s} is missing");
        }
        for s in c.source_signals() {
            assert!(names.contains(&s), "{s} is missing");
        }
        // Every input is a signal of the converter.
        for b in &blocks {
            for (_, input) in b.inputs() {
                assert!(names.iter().any(|n| n == input), "{input} of {}", b.name());
            }
        }
        // The lags can be left out, and a current reference needs no
        // division.
        let c = converter(json!({"voltage_feedforward_time_constant_s": 1e-3}));
        assert!(signal_names(&c.controls(5e-5))
            .iter()
            .any(|n| n == "PV1.v_ff_q"));
        let c = converter(json!({"reference": {"type": "current", "d_pu": "id", "q_pu": 0.1}}));
        let names = signal_names(&c.controls(5e-5));
        assert!(!names
            .iter()
            .any(|n| n.contains("v_ff") || n.contains("p_ref")));
        assert!(names.iter().any(|n| n == "PV1.i_d_cmd"));
    }
}
