//! Rotating machines: the synchronous machine in dq axes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::BusRef;
use super::{ComponentInitialization, ComponentSpec, SteadyState};
use crate::catalog::{self, p, ComponentInfo, BUS};
use crate::check::{self, positive, range_error};
use crate::diagnostic::Diagnostic;
use crate::model::GROUND;

/// Connection of the star point of the stator winding.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MachineNeutral {
    /// Connected to gnd through an impedance (solidly when both values are
    /// 0, the default). The zero-sequence current flows through
    /// ra + 3 resistance_ohm and x0 + 3 w inductance_h.
    Grounded {
        /// Must be >= 0. Default 0.
        #[serde(default)]
        resistance_ohm: f64,
        /// Must be >= 0. Default 0.
        #[serde(default)]
        inductance_h: f64,
    },
    /// Not connected: the machine carries no zero-sequence current.
    Ungrounded,
}

impl Default for MachineNeutral {
    fn default() -> Self {
        MachineNeutral::Grounded {
            resistance_ohm: 0.0,
            inductance_h: 0.0,
        }
    }
}

/// Operating point at the terminals of a machine at t = 0, as a power flow
/// gives it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineOperatingPoint {
    /// Active power delivered to the bus, in MW (negative for a motor).
    #[serde(default)]
    pub active_power_mw: f64,
    /// Reactive power delivered to the bus, in Mvar.
    #[serde(default)]
    pub reactive_power_mvar: f64,
    /// Magnitude of the terminal voltage, in per unit of rated_voltage_kv.
    /// Must be > 0. Default 1.
    #[serde(default = "one")]
    pub voltage_pu: f64,
    /// Angle of the phase-a terminal voltage phasor (cosine reference), as
    /// a power-flow voltage angle. Default 0.
    #[serde(default)]
    pub angle_deg: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for MachineOperatingPoint {
    fn default() -> Self {
        MachineOperatingPoint {
            active_power_mw: 0.0,
            reactive_power_mvar: 0.0,
            voltage_pu: 1.0,
            angle_deg: 0.0,
        }
    }
}

/// Synchronous machine in the dq axes of its rotor (Park's equations): on
/// the d axis the field winding and one damper winding, on the q axis two
/// damper windings (model 2.2 of IEEE Std 1110), or one when
/// `xq_transient_pu` and `tq0_transient_s` are omitted (model 2.1, a
/// salient-pole machine). With the swing equation
/// 2 H d(speed)/dt = Tm - Te - D (speed - 1), d(angle)/dt = w0 (speed - 1).
///
/// The parameters are the standard ones, in per unit of the machine ratings
/// (rated_power_mva, rated_voltage_kv), with open-circuit time constants in
/// seconds. Magnetic saturation is not modelled.
///
/// The phase currents I(<name>.a) etc. flow from the bus into the machine,
/// as those of a three_phase_source: a generator has currents opposite to
/// its voltages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SynchronousMachine {
    pub name: String,
    pub bus: BusRef,
    /// Star point of the stator winding. Solidly grounded when omitted.
    #[serde(default)]
    pub neutral: MachineNeutral,
    /// Rated apparent power in MVA: the base of every per-unit value.
    pub rated_power_mva: f64,
    /// Rated line-to-line voltage, rms, in kV.
    pub rated_voltage_kv: f64,
    /// Rated frequency in Hz. Must be > 0.
    pub frequency_hz: f64,
    /// Synchronous reactance of the d axis, Xd.
    pub xd_pu: f64,
    /// Synchronous reactance of the q axis, Xq.
    pub xq_pu: f64,
    /// Transient reactance of the d axis, Xd'.
    pub xd_transient_pu: f64,
    /// Transient reactance of the q axis, Xq'. Omit it together with
    /// tq0_transient_s for a machine with one damper winding on the q axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xq_transient_pu: Option<f64>,
    /// Subtransient reactance of the d axis, Xd''.
    pub xd_subtransient_pu: f64,
    /// Subtransient reactance of the q axis, Xq''.
    pub xq_subtransient_pu: f64,
    /// Leakage reactance of the stator, Xl.
    pub xl_pu: f64,
    /// Resistance of the stator, Ra. Must be >= 0. Default 0.
    #[serde(default)]
    pub ra_pu: f64,
    /// Zero-sequence reactance, X0. Must be > 0. Default xl_pu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x0_pu: Option<f64>,
    /// Open-circuit transient time constant of the d axis, Td0'.
    pub td0_transient_s: f64,
    /// Open-circuit subtransient time constant of the d axis, Td0''.
    pub td0_subtransient_s: f64,
    /// Open-circuit transient time constant of the q axis, Tq0'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tq0_transient_s: Option<f64>,
    /// Open-circuit subtransient time constant of the q axis, Tq0''.
    pub tq0_subtransient_s: f64,
    /// Inertia constant H in MW s / MVA (seconds). Must be > 0.
    pub inertia_h_s: f64,
    /// Damping torque coefficient D, in per unit torque per per unit speed
    /// deviation. Must be >= 0. Default 0.
    #[serde(default)]
    pub damping_pu: f64,
    /// Control signal of the field voltage Efd in per unit: 1 gives rated
    /// voltage at no load and rated speed. Without it the field voltage
    /// keeps its value at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_voltage_signal: Option<String>,
    /// Control signal of the mechanical torque Tm in per unit of
    /// rated_power_mva at rated speed, positive for a generator. Without it
    /// the torque keeps its value at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanical_torque_signal: Option<String>,
    /// Operating point at the terminals at t = 0. No load at rated voltage
    /// and angle 0 when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_point: Option<MachineOperatingPoint>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// The windings of one axis of the rotor in per unit (reciprocal system,
/// base inductance and resistance of the stator), in the generator
/// convention of the stator currents:
/// flux_s = -(l_m + l_leak_s) i_s + l_m (i_1 + i_2),
/// flux_k = (l_m + leakage[k]) i_k + l_m i_other - l_m i_s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisWindings {
    /// Mutual (magnetizing) inductance L_ad or L_aq.
    pub mutual_pu: f64,
    /// Number of rotor windings on the axis: 1 or 2.
    pub count: usize,
    /// Leakage inductances of the rotor windings; on the d axis the field
    /// first.
    pub leakage_pu: [f64; 2],
    /// Resistances of the rotor windings.
    pub resistance_pu: [f64; 2],
}

/// Fundamental parameters of a machine, in per unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineWindings {
    /// Leakage inductance of the stator.
    pub leakage_pu: f64,
    /// Resistance of the stator.
    pub resistance_pu: f64,
    /// d axis: the field winding (index 0) and the damper winding.
    pub d: AxisWindings,
    /// q axis: one or two damper windings.
    pub q: AxisWindings,
}

/// The rotor windings of an axis from its reactances and open-circuit time
/// constants, by the classical expressions (Kundur, section 4.2): the
/// transient values are those of the first winding alone, the subtransient
/// values those of the second winding with the first one short-circuited
/// and without resistance.
///
/// X' = Xl + Lm L1 / (Lm + L1), T0' = (Lm + L1) / (w0 R1),
/// X'' = Xl + 1 / (1/Lm + 1/L1 + 1/L2), T0'' = (L2 + Lm L1 / (Lm + L1)) / (w0 R2).
fn two_windings(
    x: f64,
    x_transient: f64,
    x_subtransient: f64,
    xl: f64,
    t0_transient: f64,
    t0_subtransient: f64,
    omega: f64,
) -> AxisWindings {
    let lm = x - xl;
    let l1 = lm * (x_transient - xl) / (x - x_transient);
    let l2 = (x_transient - xl) * (x_subtransient - xl) / (x_transient - x_subtransient);
    AxisWindings {
        mutual_pu: lm,
        count: 2,
        leakage_pu: [l1, l2],
        resistance_pu: [
            (lm + l1) / (omega * t0_transient),
            (l2 + x_transient - xl) / (omega * t0_subtransient),
        ],
    }
}

/// One rotor winding: X'' = Xl + Lm L1 / (Lm + L1), T0'' = (Lm + L1) / (w0 R1).
fn one_winding(x: f64, x_subtransient: f64, xl: f64, t0: f64, omega: f64) -> AxisWindings {
    let lm = x - xl;
    let l1 = lm * (x_subtransient - xl) / (x - x_subtransient);
    AxisWindings {
        mutual_pu: lm,
        count: 1,
        leakage_pu: [l1, 0.0],
        resistance_pu: [(lm + l1) / (omega * t0), 0.0],
    }
}

impl SynchronousMachine {
    /// Names of the signals the machine offers, in the order of its
    /// element.
    pub const SIGNALS: [&'static str; 15] = [
        "speed_pu",
        "angle_rad",
        "electrical_torque_pu",
        "active_power_pu",
        "reactive_power_pu",
        "terminal_voltage_pu",
        "vd_pu",
        "vq_pu",
        "id_pu",
        "iq_pu",
        "field_current_pu",
        "field_voltage_pu",
        "mechanical_torque_pu",
        "initial_field_voltage_pu",
        "initial_mechanical_torque_pu",
    ];

    /// True for a machine with two damper windings on the q axis.
    pub fn has_two_q_windings(&self) -> bool {
        self.xq_transient_pu.is_some()
    }

    /// The windings of a validated machine.
    pub fn windings(&self) -> MachineWindings {
        let omega = 2.0 * std::f64::consts::PI * self.frequency_hz;
        let d = two_windings(
            self.xd_pu,
            self.xd_transient_pu,
            self.xd_subtransient_pu,
            self.xl_pu,
            self.td0_transient_s,
            self.td0_subtransient_s,
            omega,
        );
        let q = match (self.xq_transient_pu, self.tq0_transient_s) {
            (Some(x), Some(t)) => two_windings(
                self.xq_pu,
                x,
                self.xq_subtransient_pu,
                self.xl_pu,
                t,
                self.tq0_subtransient_s,
                omega,
            ),
            _ => one_winding(
                self.xq_pu,
                self.xq_subtransient_pu,
                self.xl_pu,
                self.tq0_subtransient_s,
                omega,
            ),
        };
        MachineWindings {
            leakage_pu: self.xl_pu,
            resistance_pu: self.ra_pu,
            d,
            q,
        }
    }

    /// The operating point at t = 0.
    pub fn initial_operating_point(&self) -> MachineOperatingPoint {
        self.operating_point.unwrap_or_default()
    }
}

/// E280 unless the reactances `values` (name, value) decrease strictly in
/// the order given and the last is > 0.
fn check_reactance_order(
    values: &[(&str, f64)],
    axis: &str,
    name: &str,
    p: &str,
    d: &mut Vec<Diagnostic>,
) {
    if values.iter().any(|(_, v)| !v.is_finite()) {
        return;
    }
    let ordered = values.windows(2).all(|w| w[0].1 > w[1].1) && values[values.len() - 1].1 > 0.0;
    if !ordered {
        let order: Vec<&str> = values.iter().map(|(f, _)| *f).collect();
        let given: Vec<String> = values.iter().map(|(f, v)| format!("{f} = {v}")).collect();
        d.push(Diagnostic::error(
            "E280",
            format!("{p}.{}", values[0].0),
            format!(
                "the {axis}-axis reactances of \"{name}\" are not in the order {} > 0: {}",
                order.join(" > "),
                given.join(", ")
            ),
            "Each reactance is that of the windings of the one before with one more rotor winding \
             short-circuited, so it is smaller; the leakage reactance is the smallest. Check the \
             values and that all are on the machine base (rated_power_mva, rated_voltage_kv), \
             unsaturated. For a machine without a second q-axis damper winding, omit \
             xq_transient_pu and tq0_transient_s.",
        ));
    }
}

/// E281 unless transient > subtransient > 0.
fn check_time_constants(
    transient: Option<(&str, f64)>,
    subtransient: (&str, f64),
    name: &str,
    p: &str,
    d: &mut Vec<Diagnostic>,
) {
    let (field, t2) = subtransient;
    let ok = match transient {
        Some((_, t1)) => positive(t2) && t1.is_finite() && t1 > t2,
        None => positive(t2),
    };
    if !ok {
        d.push(Diagnostic::error(
            "E281",
            format!("{p}.{field}"),
            match transient {
                Some((f1, t1)) => format!(
                    "the time constants of \"{name}\" are not in the order {f1} > {field} > 0: \
                     {f1} = {t1}, {field} = {t2}"
                ),
                None => format!("{field} of \"{name}\" is {t2}"),
            },
            "Give open-circuit time constants in seconds: the transient one (seconds) is longer \
             than the subtransient one (some hundredths of a second). Short-circuit time \
             constants are converted first: Td0' = Td' Xd / Xd', Td0'' = Td'' Xd' / Xd''.",
        ));
    }
}

impl ComponentSpec for SynchronousMachine {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "synchronous_machine"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn node_names(&self) -> Vec<String> {
        let mut v = Vec::from(self.bus.nodes());
        if matches!(self.neutral, MachineNeutral::Grounded { .. }) {
            v.push(GROUND.to_string());
        }
        v
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        let mut v = vec![super::Terminal::bus("bus", &self.bus)];
        if matches!(self.neutral, MachineNeutral::Grounded { .. }) {
            v.push(super::Terminal::node("neutral", GROUND));
        }
        v
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check::bus_ref(d, &self.bus, format!("{p}.bus"));
        let nodes = self.bus.nodes();
        if nodes.iter().any(|n| n == GROUND) || nodes[0] == nodes[1] || nodes[1] == nodes[2] {
            check::same_node(
                d,
                format!("{p}.bus"),
                format!(
                    "the phases of \"{}\" are not three nodes other than gnd",
                    self.name
                ),
            );
        }
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        for (field, v, hint) in [
            (
                "rated_power_mva",
                self.rated_power_mva,
                "The rated apparent power must be > 0 MVA.",
            ),
            (
                "rated_voltage_kv",
                self.rated_voltage_kv,
                "The rated line-to-line voltage must be > 0 kV.",
            ),
            (
                "frequency_hz",
                self.frequency_hz,
                "Frequency must be > 0 Hz, for example 50 or 60.",
            ),
            (
                "inertia_h_s",
                self.inertia_h_s,
                "The inertia constant H must be > 0 s (2 to 9 s for most generators). For a \
                 machine that should hold its speed, use a large value such as 1e6.",
            ),
        ] {
            if !positive(v) {
                range_error(d, p, name, field, v, hint);
            }
        }
        for (field, v) in [("ra_pu", self.ra_pu), ("damping_pu", self.damping_pu)] {
            if !v.is_finite() || v < 0.0 {
                range_error(d, p, name, field, v, "Use a finite value >= 0.");
            }
        }
        for (field, v) in [
            ("xd_pu", self.xd_pu),
            ("xq_pu", self.xq_pu),
            ("xd_transient_pu", self.xd_transient_pu),
            ("xd_subtransient_pu", self.xd_subtransient_pu),
            ("xq_subtransient_pu", self.xq_subtransient_pu),
            ("xl_pu", self.xl_pu),
            ("x0_pu", self.x0_pu.unwrap_or(1.0)),
            ("xq_transient_pu", self.xq_transient_pu.unwrap_or(1.0)),
        ] {
            if !positive(v) {
                range_error(d, p, name, field, v, "Reactances must be > 0 per unit.");
            }
        }
        check_reactance_order(
            &[
                ("xd_pu", self.xd_pu),
                ("xd_transient_pu", self.xd_transient_pu),
                ("xd_subtransient_pu", self.xd_subtransient_pu),
                ("xl_pu", self.xl_pu),
            ],
            "d",
            name,
            p,
            d,
        );
        check_time_constants(
            Some(("td0_transient_s", self.td0_transient_s)),
            ("td0_subtransient_s", self.td0_subtransient_s),
            name,
            p,
            d,
        );
        match (self.xq_transient_pu, self.tq0_transient_s) {
            (Some(x), Some(t)) => {
                check_reactance_order(
                    &[
                        ("xq_pu", self.xq_pu),
                        ("xq_transient_pu", x),
                        ("xq_subtransient_pu", self.xq_subtransient_pu),
                        ("xl_pu", self.xl_pu),
                    ],
                    "q",
                    name,
                    p,
                    d,
                );
                check_time_constants(
                    Some(("tq0_transient_s", t)),
                    ("tq0_subtransient_s", self.tq0_subtransient_s),
                    name,
                    p,
                    d,
                );
            }
            (None, None) => {
                check_reactance_order(
                    &[
                        ("xq_pu", self.xq_pu),
                        ("xq_subtransient_pu", self.xq_subtransient_pu),
                        ("xl_pu", self.xl_pu),
                    ],
                    "q",
                    name,
                    p,
                    d,
                );
                check_time_constants(
                    None,
                    ("tq0_subtransient_s", self.tq0_subtransient_s),
                    name,
                    p,
                    d,
                );
            }
            (x, _) => {
                let (given, missing) = if x.is_some() {
                    ("xq_transient_pu", "tq0_transient_s")
                } else {
                    ("tq0_transient_s", "xq_transient_pu")
                };
                d.push(Diagnostic::error(
                    "E282",
                    format!("{p}.{given}"),
                    format!("\"{name}\" has {given} but not {missing}"),
                    "Give both xq_transient_pu and tq0_transient_s for a machine with two damper \
                     windings on the q axis (a round rotor), or neither for a machine with one (a \
                     salient-pole machine: xq_subtransient_pu and tq0_subtransient_s describe \
                     it).",
                ));
            }
        }
        if let MachineNeutral::Grounded {
            resistance_ohm,
            inductance_h,
        } = self.neutral
        {
            for (field, v) in [
                ("neutral.resistance_ohm", resistance_ohm),
                ("neutral.inductance_h", inductance_h),
            ] {
                if !v.is_finite() || v < 0.0 {
                    range_error(d, p, name, field, v, "Use a finite value >= 0.");
                }
            }
        }
        if let Some(o) = &self.operating_point {
            if !positive(o.voltage_pu) {
                range_error(
                    d,
                    p,
                    name,
                    "operating_point.voltage_pu",
                    o.voltage_pu,
                    "The terminal voltage must be > 0 per unit of rated_voltage_kv.",
                );
            }
            for (field, v) in [
                ("operating_point.active_power_mw", o.active_power_mw),
                ("operating_point.reactive_power_mvar", o.reactive_power_mvar),
                ("operating_point.angle_deg", o.angle_deg),
            ] {
                if !v.is_finite() {
                    range_error(d, p, name, field, v, "Use a finite value.");
                }
            }
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        let [a, b, c] = self.bus.nodes();
        match self.neutral {
            MachineNeutral::Grounded { .. } => [a, b, c]
                .into_iter()
                .map(|n| (n, GROUND.to_string()))
                .collect(),
            // The windings join the phases, not the phases and gnd.
            MachineNeutral::Ungrounded => vec![(a, b.clone()), (b, c)],
        }
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        let mut v = Vec::new();
        if let Some(s) = &self.field_voltage_signal {
            v.push(("field_voltage_signal", s.as_str()));
        }
        if let Some(s) = &self.mechanical_torque_signal {
            v.push(("mechanical_torque_signal", s.as_str()));
        }
        v
    }
    fn signal_names(&self) -> Vec<&'static str> {
        Self::SIGNALS.to_vec()
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::Sine(self.frequency_hz)
    }
    fn sine_frequency(&self) -> Option<f64> {
        Some(self.frequency_hz)
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "synchronous_machine",
            category: "circuit",
            summary: "Synchronous machine in the dq axes of its rotor: field and one damper winding on the d axis, two damper windings on the q axis (IEEE Std 1110 model 2.2), or one (model 2.1) when xq_transient_pu and tq0_transient_s are omitted; swing equation 2 H d(speed)/dt = Tm - Te - D (speed - 1). Standard parameters in per unit of the machine ratings, unsaturated; no magnetic saturation. Inputs: the signals of the field voltage and the mechanical torque (constant at their values at t = 0 without a signal). Starts from operating_point: with \"initialization\": \"steady_state\" the phasor solution takes the machine as a source behind ra + j (xd'' + xq'')/2 that gives operating_point in the network of the power flow, and the rotor angle, fluxes, field voltage and torque follow from the solved terminal voltage and current, so the run starts in an exact steady state; with \"none\" the machine starts with the fluxes of operating_point at rated speed. Signals <name>.<signal>, read by a measurement of component_signal: speed_pu, angle_rad (q axis against the reference cos(w0 t), the power-flow angle of the internal voltage), electrical_torque_pu, active_power_pu and reactive_power_pu (delivered to the bus, on rated_power_mva), terminal_voltage_pu, vd_pu, vq_pu, id_pu, iq_pu (currents out of the machine), field_current_pu (1 gives rated voltage at no load), field_voltage_pu, mechanical_torque_pu, initial_field_voltage_pu and initial_mechanical_torque_pu (the values at t = 0 that hold the initial operating point: references of an AVR and a governor).",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the bus into the machine (as those of a three_phase_source), so a generator has currents opposite to its voltages. U(<name>.a) etc. are the phase voltages against gnd. The signals use the generator convention: powers delivered to the bus and id_pu, iq_pu out of the machine are positive. dq axes: the q axis leads the d axis by 90 degrees, amplitude-invariant, as abc_to_dq0 with theta = w0 t + angle_rad - pi/2.",
            parameters: vec![
                catalog::name(),
                p("bus", "", true, BUS),
                p("neutral", "ohm, H", false, "{\"type\":\"grounded\",\"resistance_ohm\":..,\"inductance_h\":..} (star point to gnd through this impedance; both default 0) | {\"type\":\"ungrounded\"} (no zero-sequence current). Default: solidly grounded."),
                p("rated_power_mva", "MVA", true, "Rated apparent power, > 0: the base of the per-unit values."),
                p("rated_voltage_kv", "kV", true, "Rated line-to-line voltage, rms, > 0."),
                p("frequency_hz", "Hz", true, "Rated frequency, > 0."),
                p("xd_pu", "pu", true, "Synchronous reactance Xd. xd_pu > xd_transient_pu > xd_subtransient_pu > xl_pu > 0 (E280)."),
                p("xq_pu", "pu", true, "Synchronous reactance Xq. xq_pu > [xq_transient_pu >] xq_subtransient_pu > xl_pu."),
                p("xd_transient_pu", "pu", true, "Transient reactance Xd'."),
                p("xq_transient_pu", "pu", false, "Transient reactance Xq'. Omit with tq0_transient_s for one damper winding on the q axis (salient-pole machine)."),
                p("xd_subtransient_pu", "pu", true, "Subtransient reactance Xd''."),
                p("xq_subtransient_pu", "pu", true, "Subtransient reactance Xq''. It may differ from Xd''."),
                p("xl_pu", "pu", true, "Stator leakage reactance Xl."),
                p("ra_pu", "pu", false, "Stator resistance Ra, >= 0. Default 0."),
                p("x0_pu", "pu", false, "Zero-sequence reactance X0, > 0. Default xl_pu."),
                p("td0_transient_s", "s", true, "Open-circuit transient time constant Td0'. td0_transient_s > td0_subtransient_s > 0 (E281). From short-circuit values: Td0' = Td' Xd / Xd'."),
                p("td0_subtransient_s", "s", true, "Open-circuit subtransient time constant Td0''. From short-circuit values: Td0'' = Td'' Xd' / Xd''."),
                p("tq0_transient_s", "s", false, "Open-circuit transient time constant Tq0'. With xq_transient_pu (E282 when only one is given)."),
                p("tq0_subtransient_s", "s", true, "Open-circuit subtransient time constant Tq0''."),
                p("inertia_h_s", "s", true, "Inertia constant H = stored energy at rated speed / rated_power_mva, > 0."),
                p("damping_pu", "pu", false, "Damping torque coefficient D, >= 0: torque D (speed - 1). Default 0."),
                p("field_voltage_signal", "pu", false, "Signal of the field voltage Efd; 1 pu gives rated voltage at no load and rated speed (the base of an AVR). The machine follows it one time step late, as a controlled source. Without it Efd stays at <name>.initial_field_voltage_pu."),
                p("mechanical_torque_signal", "pu", false, "Signal of the mechanical torque Tm on rated_power_mva at rated speed, positive for a generator (a governor that gives power divides it by the speed). Without it Tm stays at <name>.initial_mechanical_torque_pu."),
                p("operating_point", "MW, Mvar, pu, deg", false, "{\"active_power_mw\":..,\"reactive_power_mvar\":..,\"voltage_pu\":..,\"angle_deg\":..}: powers delivered to the bus and the terminal voltage (magnitude on rated_voltage_kv, angle of phase a) at t = 0, as a power flow gives them. Default: no load, 1 pu, 0 degrees."),
            ],
            example: json!({"type": "synchronous_machine", "name": "G1", "bus": "b1", "neutral": {"type": "grounded", "resistance_ohm": 0.0, "inductance_h": 0.0},
                "rated_power_mva": 555.0, "rated_voltage_kv": 24.0, "frequency_hz": 60.0,
                "xd_pu": 1.81, "xq_pu": 1.76, "xd_transient_pu": 0.3, "xq_transient_pu": 0.65, "xd_subtransient_pu": 0.23, "xq_subtransient_pu": 0.25,
                "xl_pu": 0.15, "ra_pu": 0.003, "x0_pu": 0.15,
                "td0_transient_s": 8.0, "td0_subtransient_s": 0.03, "tq0_transient_s": 1.0, "tq0_subtransient_s": 0.07,
                "inertia_h_s": 3.5, "damping_pu": 0.0,
                "field_voltage_signal": "efd", "mechanical_torque_signal": "tm",
                "operating_point": {"active_power_mw": 499.5, "reactive_power_mvar": 241.9, "voltage_pu": 1.0, "angle_deg": 0.0}}),
        }
    }
}
