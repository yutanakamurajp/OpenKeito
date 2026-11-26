use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::components::*;
pub use crate::controls::*;

/// Value of the `format` field.
pub const FORMAT: &str = "openkeito-model";
/// Current value of the `version` field. What a model of this version may
/// use beyond the previous one is listed in `VERSION_ADDITIONS` (validation).
pub const VERSION: &str = "0.9";
/// Every `version` this crate reads. A model of an older version is also a
/// valid model of every newer one.
pub const SUPPORTED_VERSIONS: [&str; 9] = [
    "0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.7", "0.8", "0.9",
];
/// Name of the reference node (0 V).
pub const GROUND: &str = "gnd";

/// One simulation case: settings, circuit and recorded quantities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// Must be "openkeito-model".
    pub format: String,
    /// Format version: "0.9"; "0.8" for a model without the additions of
    /// 0.9 (thyristor); "0.7" for a model without the additions of
    /// 0.8 (expression); "0.6" for a model without the additions of
    /// 0.7 (piecewise_linear, signal limits on lag, pi, integrator and
    /// limiter, pi.anti_windup, initialization on components);
    /// "0.5" for a model without those of 0.6 (zero_sequence_inverted,
    /// tap_changes, impedance_changes);
    /// "0.4" for a model without those of 0.5 (simulation.switching);
    /// "0.3" for a model also without those of 0.4 (three_phase_load,
    /// three_phase_current_source, three_phase_ideal_transformer); "0.2" for
    /// a model also without those of 0.3 (three-phase components,
    /// bus_voltage outputs, initialization, linear_solver, frequency_scan);
    /// "0.1" for a model also without those of 0.2 (diode,
    /// controlled_switch, controls, signal outputs, output_interval_steps).
    pub version: String,
    /// Short name of the case.
    #[serde(default)]
    pub name: String,
    /// Free text.
    #[serde(default)]
    pub description: String,
    pub simulation: Simulation,
    /// Circuit components. The reference node is named "gnd".
    pub components: Vec<Component>,
    /// Control blocks. The name of a block is the name of its output signal.
    /// Names are unique across components and control blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
    /// Recorded quantities. When omitted, every node voltage and every branch
    /// current is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Output>>,
    /// Settings of `openkeito scan` (impedances seen from nodes and buses
    /// over frequency). `openkeito run` does not use them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_scan: Option<FrequencyScan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Simulation {
    /// Fixed time step in seconds.
    pub time_step_s: f64,
    /// End time in seconds. The run covers t = 0 .. end_time_s.
    pub end_time_s: f64,
    /// Integration method. Default "2s-dirk".
    #[serde(default)]
    pub method: Method,
    /// Record one output row every this many time steps (the row at t = 0
    /// is always recorded). Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub output_interval_steps: Option<u64>,
    /// State at t = 0. Default "none": inductor currents and capacitor
    /// voltages as given (0 unless set). "steady_state": the sinusoidal
    /// steady state of the sources, from a phasor solution at their common
    /// frequency.
    #[serde(default, skip_serializing_if = "Initialization::is_none")]
    pub initialization: Initialization,
    /// Linear solver of the circuit equations. Default "auto": dense LU for
    /// small circuits, sparse LU from AUTO_SPARSE_UNKNOWNS unknowns on.
    #[serde(default, skip_serializing_if = "LinearSolver::is_auto")]
    pub linear_solver: LinearSolver,
    /// Where switching instants lie. "grid" (default): at grid points (an
    /// instant moves to the next grid point). "interpolated": within the
    /// time step, which is split there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switching: Option<Switching>,
    /// Whether the circuit is divided at its travelling-wave lines
    /// (`bergeron_line`) into parts that are solved on their own, each with
    /// its own matrix. "auto" (default): divided where only such lines join
    /// two parts, except that small parts stay together. "every_line":
    /// divided at every such line. "off": one matrix for the whole circuit.
    /// The results agree to rounding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitioning: Option<Partitioning>,
    /// Largest number of threads that solve the parts of a divided circuit
    /// side by side. Default 1. The solver uses fewer when the parts are too
    /// few or too small to gain from more; the results do not depend on the
    /// number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub threads: Option<u32>,
}

/// Whether a circuit is divided into parts at its travelling-wave lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Partitioning {
    /// Divided where only travelling-wave lines join two parts of the
    /// circuit; parts that such a line joins stay together while they have
    /// no more than 48 named nodes between them (16 three-phase buses),
    /// because a part costs a little in every step. A circuit without such
    /// a line is one part.
    #[default]
    Auto,
    /// Divided wherever only travelling-wave lines join two parts, however
    /// small the parts.
    EveryLine,
    /// Not divided: one matrix for the whole circuit.
    Off,
}

impl Partitioning {
    pub fn as_str(self) -> &'static str {
        match self {
            Partitioning::Auto => "auto",
            Partitioning::EveryLine => "every_line",
            Partitioning::Off => "off",
        }
    }
}

/// Where the switching instants of a run lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Switching {
    /// At grid points. A switch acts at the first grid point at or after its
    /// time, a diode changes at the first grid point after its zero
    /// crossing, a gate signal acts at the grid point that follows its
    /// change.
    #[default]
    Grid,
    /// At the instant itself: the given time of a switch, a fault or a step
    /// source, the zero crossing of a diode or of a contact that opens at a
    /// current zero (by linear interpolation within the step), the instant
    /// at which a gate signal crosses its threshold. The time step is split
    /// there; the output stays on the grid.
    Interpolated,
}

impl Switching {
    pub fn as_str(self) -> &'static str {
        match self {
            Switching::Grid => "grid",
            Switching::Interpolated => "interpolated",
        }
    }
}

/// Number of unknowns from which "auto" uses the sparse LU.
pub const AUTO_SPARSE_UNKNOWNS: usize = 16;

/// Linear solver of the circuit equations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinearSolver {
    /// Dense below AUTO_SPARSE_UNKNOWNS unknowns, sparse from there on.
    #[default]
    Auto,
    /// Dense LU with scaled partial pivoting.
    Dense,
    /// Sparse LU (column order computed once, partial pivoting with row
    /// equilibration).
    Sparse,
}

impl LinearSolver {
    pub fn is_auto(&self) -> bool {
        *self == LinearSolver::Auto
    }

    /// True when a circuit with `unknowns` unknowns uses the sparse LU.
    pub fn is_sparse(self, unknowns: usize) -> bool {
        match self {
            LinearSolver::Auto => unknowns >= AUTO_SPARSE_UNKNOWNS,
            LinearSolver::Dense => false,
            LinearSolver::Sparse => true,
        }
    }
}

/// How the run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Initialization {
    /// From the initial values of the components (0 unless given).
    #[default]
    None,
    /// From the sinusoidal steady state: every source is a sine of one
    /// frequency; switches are in their state before t = 0, faults have not
    /// started. Not for diodes, controlled switches and thyristors.
    SteadyState,
}

impl Initialization {
    pub fn is_none(&self) -> bool {
        *self == Initialization::None
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Initialization::None => "none",
            Initialization::SteadyState => "steady_state",
        }
    }
}

/// Spacing of the frequencies of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Spacing {
    /// Equal differences.
    #[default]
    Linear,
    /// Equal ratios.
    Log,
}

/// Impedances over frequency: the network with every source set to zero
/// (voltage sources shorted, current sources open, three-phase sources
/// reduced to their internal impedance), switches in their state before
/// t = 0, faults not started, diodes, controlled switches and thyristors
/// off. Each
/// impedance is the voltage at a node per ampere injected into a node.
///
/// Give either start_hz, stop_hz and points, or frequencies_hz.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrequencyScan {
    /// First frequency, > 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_hz: Option<f64>,
    /// Last frequency, >= start_hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_hz: Option<f64>,
    /// Number of frequencies including both ends, >= 2 (1 when start_hz
    /// equals stop_hz).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<u32>,
    /// "linear" (default) or "log".
    #[serde(default)]
    pub spacing: Spacing,
    /// The frequencies, each > 0, instead of start_hz, stop_hz and points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequencies_hz: Option<Vec<f64>>,
    /// Nodes whose self impedances and mutual impedances (every pair) are
    /// computed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    /// Buses whose positive-, negative- and zero-sequence impedances are
    /// computed (the diagonal of the 3x3 phase impedance matrix of the bus
    /// transformed to symmetrical components).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<String>,
}

/// Largest number of frequencies of a scan.
pub const MAX_SCAN_POINTS: u32 = 1_000_000;

impl FrequencyScan {
    /// The frequencies of a validated scan, in Hz.
    pub fn frequencies(&self) -> Vec<f64> {
        if let Some(f) = &self.frequencies_hz {
            return f.clone();
        }
        let (a, b) = (self.start_hz.unwrap_or(1.0), self.stop_hz.unwrap_or(1.0));
        let n = self.points.unwrap_or(1).max(1) as usize;
        if n == 1 {
            return vec![a];
        }
        (0..n)
            .map(|k| {
                let x = k as f64 / (n - 1) as f64;
                if k == n - 1 {
                    b
                } else {
                    match self.spacing {
                        Spacing::Linear => a + (b - a) * x,
                        Spacing::Log => a * (b / a).powf(x),
                    }
                }
            })
            .collect()
    }
}

/// Shortest part of a time step that a run with "switching":
/// "interpolated" solves, as a fraction of the step: an instant closer than
/// that to a grid point, or to the instant before it, is moved there.
pub const SHORTEST_STEP_PART: f64 = 1e-6;

impl Simulation {
    /// The stage coefficients alpha (1/s) of the run. In a stage, the
    /// derivative of a state x is alpha (x - x_start) minus a term of the
    /// stage before, so an inductance L is solved as the resistance alpha L
    /// and a capacitance C as the conductance alpha C. In this order: the
    /// regular stages (2 / time_step_s for the trapezoidal rule,
    /// 3.41 / time_step_s for 2S-DIRK), the backward-Euler half steps after
    /// a switching action (2 / time_step_s), the solution at t = 0+
    /// (1e6 / time_step_s) and, with "switching": "interpolated", the
    /// largest of a part of a split step (the larger of the first two,
    /// divided by `SHORTEST_STEP_PART`; a part has any value between). The
    /// solver computes the same values (`Stages` of openkeito-core, which a
    /// test compares with these).
    pub fn stage_coefficients(&self) -> Vec<f64> {
        let h = self.time_step_s;
        let regular = match self.method {
            Method::TwoStageDirk => 1.0 / ((1.0 - std::f64::consts::FRAC_1_SQRT_2) * h),
            Method::Trapezoidal => 2.0 / h,
        };
        let half_step = 2.0 / h;
        let initial = 1.0 / (1e-6 * h);
        let mut alphas = vec![regular, half_step, initial];
        if self.switching() == Switching::Interpolated {
            alphas.push(regular.max(half_step) / SHORTEST_STEP_PART);
        }
        alphas
    }

    /// Where the switching instants lie (the default when not given).
    pub fn switching(&self) -> Switching {
        self.switching.unwrap_or_default()
    }

    /// Whether the circuit is divided at its travelling-wave lines (the
    /// default when not given).
    pub fn partitioning(&self) -> Partitioning {
        self.partitioning.unwrap_or_default()
    }

    /// Largest number of threads of a run (1 when not given).
    pub fn threads(&self) -> usize {
        self.threads.unwrap_or(1).max(1) as usize
    }

    /// Steps between output rows (1 when not given).
    pub fn output_interval(&self) -> u64 {
        self.output_interval_steps.unwrap_or(1).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum Method {
    /// Two-stage diagonally implicit Runge-Kutta, order 2, L-stable.
    #[default]
    #[serde(rename = "2s-dirk")]
    TwoStageDirk,
    /// Trapezoidal rule. The step after a switching action is replaced by two
    /// backward-Euler half steps to suppress numerical oscillation.
    #[serde(rename = "trapezoidal")]
    Trapezoidal,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::TwoStageDirk => "2s-dirk",
            Method::Trapezoidal => "trapezoidal",
        }
    }
}

/// A recorded quantity, one CSV column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Output {
    /// Voltage of a node against gnd, column `V(<node>)`.
    NodeVoltage { node: String },
    /// Current through a component from nodes[0] to nodes[1], column `I(<component>)`.
    BranchCurrent { component: String },
    /// V(nodes[0]) - V(nodes[1]) of a component, column `U(<component>)`.
    BranchVoltage { component: String },
    /// A control signal, column `S(<signal>)`.
    Signal { signal: String },
    /// The three phase voltages of a bus against gnd, columns `V(<bus>.a)`,
    /// `V(<bus>.b)` and `V(<bus>.c)`.
    BusVoltage { bus: String },
}
