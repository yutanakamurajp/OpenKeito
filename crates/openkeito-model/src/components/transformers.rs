//! Transformers: the single-phase unit, the three-phase bank with its
//! vector groups and the ideal three-phase transformer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::nonlinear::Saturation;
use super::three_phase::{ground, phase_pairs, BusRef};
use super::{Branch, ComponentInitialization, ComponentSpec};
use crate::catalog::{
    self, p, ComponentInfo, IMPEDANCE_PERCENT, NO_LOAD_CURRENT, RESISTANCE_PERCENT,
};
use crate::check::{
    bus_ref as bus, node_name as node, order_error, positive, range_error, same_node as same,
};
use crate::diagnostic::Diagnostic;

/// A scheduled tap change of a `transformer` or `three_phase_transformer`
/// (format 0.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformerTapChange {
    /// Time at which the tap takes these values, in seconds (>= 0).
    pub time_s: f64,
    /// Rated voltages [side 0, side 1] in kV, > 0.
    pub rated_voltage_kv: [f64; 2],
    /// Short-circuit impedance |Z| (u_k) in percent, > 0. Defaults to the
    /// component's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance_percent: Option<f64>,
    /// Resistive part (u_r) in percent, >= 0 and < impedance_percent.
    /// Defaults to the component's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resistance_percent: Option<f64>,
}

/// Single-phase two-winding transformer: an ideal transformer with the
/// leakage impedance split equally between the two windings and the
/// magnetizing branch (resistance in parallel with inductance) across the
/// ideal transformer on the side of winding 0 (T equivalent circuit).
/// Percentages are on the rated power and the rated winding voltages.
///
/// The first node of each winding carries the polarity dot: without load,
/// V(w0[0]) - V(w0[1]) = (v0/v1) * (V(w1[0]) - V(w1[1])). The winding
/// currents I(<name>.0) and I(<name>.1) flow into the first node of each
/// winding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transformer {
    pub name: String,
    /// Nodes of winding 0 and of winding 1, each [dot, other].
    pub windings: [[String; 2]; 2],
    /// Rated voltages of winding 0 and winding 1, rms, in kV. Must be > 0.
    pub rated_voltage_kv: [f64; 2],
    /// Must be > 0.
    pub rated_power_mva: f64,
    /// Frequency at which impedance_percent holds. Must be > 0.
    pub rated_frequency_hz: f64,
    /// Short-circuit impedance |Z| (u_k) in percent. Must be > 0.
    pub impedance_percent: f64,
    /// Resistive part of the short-circuit impedance (u_r, load loss) in
    /// percent, >= 0 and less than impedance_percent. Default 0.
    #[serde(default)]
    pub resistance_percent: f64,
    /// Magnitude of the magnetizing admittance (no-load current i_0) in
    /// percent, >= 0. Default 0: no magnetizing branch.
    #[serde(default)]
    pub no_load_current_percent: f64,
    /// Core loss at rated voltage in kW, >= 0. It must not exceed
    /// no_load_current_percent of the rated power. Default 0.
    #[serde(default)]
    pub no_load_loss_kw: f64,
    /// Saturation of the magnetizing branch (format 0.5). Without it the
    /// branch is linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<Saturation>,
    /// Scheduled tap changes at times >= 0 s, in strictly ascending order
    /// (format 0.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tap_changes: Vec<TransformerTapChange>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// The branches of one single-phase unit as the solver builds it: the
/// leakage impedance of winding 0 from its dotted node to the inner node m,
/// the magnetizing branch from m to the other node, the ideal transformer
/// (m, other node) : (x, other node of winding 1) and the leakage impedance
/// of winding 1 from its dotted node to the inner node x. `label` names the
/// inner nodes, `nodes` are [dot 0, other 0, dot 1, other 1], `ratio` is the
/// ratio of the winding voltages and `no_load` is [no_load_current_percent,
/// no_load_loss_kw / rated_power_mva]: without either (and without
/// saturation) there is no magnetizing branch.
fn unit_branches(
    label: &str,
    nodes: [&str; 4],
    ratio: f64,
    no_load: [f64; 2],
    saturation: bool,
    v: &mut Vec<Branch>,
) {
    let [hp, hn, lp, ln] = nodes.map(str::to_string);
    let (m, x) = (format!("{label}m"), format!("{label}x"));
    // Per unit: the admittance and the conductance of the core loss.
    let (y, g) = (no_load[0] / 100.0, no_load[1] * 1e-3);
    v.push(Branch::Conductance(hp, m.clone()));
    if saturation || g > 0.0 || y * y - g * g > 0.0 {
        v.push(Branch::Conductance(m.clone(), hn.clone()));
    }
    v.push(Branch::Constraint(vec![
        (m, 1.0),
        (hn, -1.0),
        (x.clone(), -ratio),
        (ln, ratio),
    ]));
    v.push(Branch::Conductance(lp, x));
}

/// An end of a winding of the unit of phase p of a three-phase transformer:
/// `Some(o)` is the phase p + o, `None` the star point.
type End = Option<usize>;

/// The ends of the windings of the unit of phase p, as (dotted end, other
/// end) of the high- and the low-voltage side. With positive-sequence
/// voltages, a delta winding from p to the next phase carries sqrt(3) times
/// the phase voltage advanced by 30 degrees, one from p to the previous
/// phase the same delayed by 30 degrees, and reversing a winding adds 180
/// degrees: the ends make the low-voltage side lag by clock * 30 degrees
/// (the solver has the same table). `None` for a group that is not
/// supported.
fn winding_ends(g: &VectorGroup) -> Option<[(End, End); 2]> {
    const STAR: End = None;
    let phase = Some;
    let star = |w| matches!(w, WindingConnection::Star { .. });
    Some(match (star(g.hv), star(g.lv), g.clock) {
        (true, true, 0) => [(phase(0), STAR), (phase(0), STAR)],
        (true, true, 6) => [(phase(0), STAR), (STAR, phase(0))],
        (true, false, 1) => [(phase(0), STAR), (phase(0), phase(1))],
        (true, false, 11) => [(phase(0), STAR), (phase(0), phase(2))],
        (true, false, 5) => [(phase(0), STAR), (phase(2), phase(0))],
        (true, false, 7) => [(phase(0), STAR), (phase(1), phase(0))],
        (false, true, 11) => [(phase(0), phase(1)), (phase(0), STAR)],
        (false, true, 5) => [(phase(0), phase(1)), (STAR, phase(0))],
        (false, true, 1) => [(phase(0), phase(2)), (phase(0), STAR)],
        (false, true, 7) => [(phase(0), phase(2)), (STAR, phase(0))],
        (false, false, 0) => [(phase(0), phase(1)), (phase(0), phase(1))],
        (false, false, 6) => [(phase(0), phase(1)), (phase(1), phase(0))],
        _ => return None,
    })
}

impl ComponentSpec for Transformer {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(vec![".0".into(), ".1".into()])
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "transformer"
    }
    fn signal_names(&self) -> Vec<&'static str> {
        if self.saturation.is_some() {
            vec!["mag.flux_wb"]
        } else {
            Vec::new()
        }
    }
    fn since(&self) -> &'static str {
        if !self.tap_changes.is_empty() {
            "0.6"
        } else if self.saturation.is_some() {
            "0.5"
        } else {
            "0.3"
        }
    }
    fn node_names(&self) -> Vec<String> {
        self.windings.iter().flatten().cloned().collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        let ends = |w: usize| (0..2).map(move |k| (w, k));
        (0..2)
            .flat_map(ends)
            .map(|(w, k)| {
                super::Terminal::node(format!("windings[{w}][{k}]"), &self.windings[w][k])
            })
            .collect()
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (w, nodes) in t.windings.iter().enumerate() {
            for (k, n) in nodes.iter().enumerate() {
                node(d, n, format!("{p}.windings[{w}][{k}]"));
            }
            if nodes[0] == nodes[1] {
                same(
                    d,
                    format!("{p}.windings[{w}]"),
                    format!(
                        "both ends of winding {w} of \"{name}\" are node \"{}\"",
                        nodes[0]
                    ),
                );
            }
        }
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (k, v) in t.rated_voltage_kv.iter().enumerate() {
            if !positive(*v) {
                range_error(
                    d,
                    p,
                    name,
                    &format!("rated_voltage_kv[{k}]"),
                    *v,
                    "Rated voltages must be > 0 kV.",
                );
            }
        }
        check_transformer_ratings(
            p,
            name,
            [
                t.rated_power_mva,
                t.rated_frequency_hz,
                t.impedance_percent,
                t.resistance_percent,
                t.no_load_current_percent,
                t.no_load_loss_kw,
            ],
            d,
        );
        if let Some(s) = &t.saturation {
            s.check(
                &format!("{p}.saturation"),
                name,
                [
                    t.no_load_current_percent,
                    t.no_load_loss_kw,
                    t.rated_power_mva,
                ],
                d,
            );
        }
        check_tap_changes(
            p,
            name,
            &t.tap_changes,
            t.impedance_percent,
            t.resistance_percent,
            d,
        );
    }
    fn connections(&self) -> Vec<(String, String)> {
        self.windings
            .iter()
            .map(|w| (w[0].clone(), w[1].clone()))
            .collect()
    }
    fn has_branch_voltage(&self) -> bool {
        false
    }
    fn has_separate_windings(&self) -> bool {
        true
    }
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let [[hp, hn], [lp, ln]] = &self.windings;
        let mut v = Vec::new();
        unit_branches(
            &format!("{}:", self.name),
            [hp, hn, lp, ln],
            self.rated_voltage_kv[0] / self.rated_voltage_kv[1],
            [
                self.no_load_current_percent,
                self.no_load_loss_kw / self.rated_power_mva,
            ],
            self.saturation.is_some(),
            &mut v,
        );
        v
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "transformer",
            category: "circuit",
            summary: "Single-phase two-winding transformer: ideal transformer, leakage impedance split equally between the windings, magnetizing branch (R parallel L) across the ideal transformer on the side of winding 0 (T equivalent). Percentages are on the rated power and the rated winding voltages.",
            sign_convention: "The first node of each winding carries the polarity dot: V(w0[0]) - V(w0[1]) = (v0/v1)(V(w1[0]) - V(w1[1])) without load. Winding currents I(<name>.0) and I(<name>.1) flow into the first node of each winding.",
            parameters: vec![
                catalog::name(),
                p("windings", "", true, "[[dot, other] of winding 0, [dot, other] of winding 1]."),
                p("rated_voltage_kv", "kV", true, "[winding 0, winding 1], rms, > 0."),
                p("rated_power_mva", "MVA", true, "Rating, > 0."),
                p("rated_frequency_hz", "Hz", true, "Frequency at which impedance_percent holds, > 0."),
                p("impedance_percent", "%", true, IMPEDANCE_PERCENT),
                p("resistance_percent", "%", false, RESISTANCE_PERCENT),
                p("no_load_current_percent", "%", false, NO_LOAD_CURRENT),
                p("no_load_loss_kw", "kW", false, "Core loss at rated voltage, >= 0, at most no_load_current_percent of the rating. Default 0."),
                p("saturation", "pu", false, "Saturation of the magnetizing branch (format 0.5): {\"current_pu\":[..],\"flux_pu\":[..]}, points of flux linkage against magnetizing current, > 0 and strictly increasing, at least 2, per unit of the peak values at the ratings of winding 0 (flux sqrt(2) V / (2 pi f), current sqrt(2) S / V). The origin is implied, the characteristic is symmetric and has no hysteresis. flux_pu[0] / current_pu[0] must be the unsaturated magnetizing reactance in per unit (within 1 percent); the last segment continues as the air-core inductance. The flux linkage is the signal <name>.mag.flux_wb. Default: linear."),
                p("tap_changes", "s, kV, %", false, "Scheduled tap changes (format 0.6): [{\"time_s\": .., \"rated_voltage_kv\": [v0, v1], ...}] in strictly ascending order of time_s."),
            ],
            example: json!({"type": "transformer", "name": "T1", "windings": [["p1", "gnd"], ["s1", "gnd"]], "rated_voltage_kv": [6.6, 0.2], "rated_power_mva": 0.1,
                "rated_frequency_hz": 50.0, "impedance_percent": 4.0, "resistance_percent": 1.0, "no_load_current_percent": 1.0, "no_load_loss_kw": 0.2,
                "saturation": {"current_pu": [0.010778, 0.2], "flux_pu": [1.1, 1.3]},
                "tap_changes": [{"time_s": 0.1, "rated_voltage_kv": [6.765, 0.2]}]}),
        }
    }
}

/// Three-phase two-winding transformer, a bank of three single-phase units
/// (the zero-sequence coupling of a three-limb core is not modelled). Each
/// unit is a `transformer`; the windings are connected as `vector_group`
/// says. Rated power is the three-phase value, rated voltages are
/// line-to-line. The clock number n of the vector group means that the
/// low-voltage side lags the high-voltage side by n * 30 degrees (IEC
/// 60076-1). Supported: Yy0, Yy6, Yd1, Yd5, Yd7, Yd11, Dy1, Dy5, Dy7, Dy11,
/// Dd0, Dd6, each with an N (high-voltage side) or n (low-voltage side)
/// for a star point that is brought out.
///
/// The line currents I(<name>.0.a) and I(<name>.1.a) flow from the bus into
/// the transformer at buses[0] (high voltage) and buses[1] (low voltage).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseTransformer {
    pub name: String,
    /// [high-voltage bus, low-voltage bus].
    pub buses: [BusRef; 2],
    /// Such as "YNd11" or "Dyn11".
    pub vector_group: String,
    /// Rated line-to-line voltages of the high- and low-voltage sides, rms,
    /// in kV. Must be > 0.
    pub rated_voltage_kv: [f64; 2],
    /// Three-phase rating. Must be > 0.
    pub rated_power_mva: f64,
    /// Must be > 0.
    pub rated_frequency_hz: f64,
    /// Short-circuit impedance |Z| (u_k) in percent. Must be > 0.
    pub impedance_percent: f64,
    /// Resistive part (u_r) in percent, >= 0 and less than
    /// impedance_percent. Default 0.
    #[serde(default)]
    pub resistance_percent: f64,
    /// Magnitude of the magnetizing admittance (i_0) in percent. Default 0.
    #[serde(default)]
    pub no_load_current_percent: f64,
    /// Three-phase core loss in kW. Default 0.
    #[serde(default)]
    pub no_load_loss_kw: f64,
    /// Node of the high-voltage star point, for YN only. Default "gnd".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv_neutral: Option<String>,
    /// Node of the low-voltage star point, for yn only. Default "gnd".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lv_neutral: Option<String>,
    /// Saturation of the magnetizing branch of each of the three units
    /// (format 0.5), per unit of the ratings of one unit: a third of the
    /// rated power and the voltage of a high-voltage winding. Without it
    /// the branches are linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<Saturation>,
    /// Scheduled tap changes at times >= 0 s, in strictly ascending order
    /// (format 0.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tap_changes: Vec<TransformerTapChange>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ThreePhaseTransformer {
    /// Star-point nodes of the high- and low-voltage sides: the given node or
    /// gnd for YN/yn, `None` for an isolated star point or a delta.
    pub fn neutral_nodes(&self) -> [Option<String>; 2] {
        let group = VectorGroup::parse(&self.vector_group);
        let pick = |w: Option<WindingConnection>, given: &Option<String>| match w {
            Some(WindingConnection::Star { neutral: true }) => {
                Some(given.clone().unwrap_or_else(ground))
            }
            _ => None,
        };
        [
            pick(group.map(|g| g.hv), &self.hv_neutral),
            pick(group.map(|g| g.lv), &self.lv_neutral),
        ]
    }
}

impl ComponentSpec for ThreePhaseTransformer {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[".0", ".1"]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_transformer"
    }
    fn signal_names(&self) -> Vec<&'static str> {
        if self.saturation.is_some() {
            vec!["mag.a.flux_wb", "mag.b.flux_wb", "mag.c.flux_wb"]
        } else {
            Vec::new()
        }
    }
    fn since(&self) -> &'static str {
        if !self.tap_changes.is_empty() {
            "0.6"
        } else if self.saturation.is_some() {
            "0.5"
        } else {
            "0.3"
        }
    }
    fn node_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.buses.iter().flat_map(BusRef::nodes).collect();
        v.extend(self.hv_neutral.iter().cloned());
        v.extend(self.lv_neutral.iter().cloned());
        v
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        let mut v = super::series_terminals(&self.buses);
        // The star points brought out (YN, yn): at the node given, or at
        // gnd. A node given for another winding is an error, but is there.
        let star = self.neutral_nodes();
        let given = [&self.hv_neutral, &self.lv_neutral];
        for (k, field) in ["hv_neutral", "lv_neutral"].into_iter().enumerate() {
            let node = star[k].as_ref().or(given[k].as_ref());
            v.extend(node.map(|n| super::Terminal::node(field, n)));
        }
        v
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (k, b) in t.buses.iter().enumerate() {
            bus(d, b, format!("{p}.buses[{k}]"));
        }
        for (field, n) in [("hv_neutral", &t.hv_neutral), ("lv_neutral", &t.lv_neutral)] {
            if let Some(n) = n {
                node(d, n, format!("{p}.{field}"));
            }
        }
        let neutrals = t.neutral_nodes();
        for (k, b) in t.buses.iter().enumerate() {
            let nodes = b.nodes();
            let distinct = nodes[0] != nodes[1] && nodes[1] != nodes[2] && nodes[0] != nodes[2];
            let on_neutral = neutrals[k].as_ref().is_some_and(|n| nodes.contains(n));
            if !distinct || on_neutral {
                same(
                    d,
                    format!("{p}.buses[{k}]"),
                    format!(
                        "the phases of side {k} of \"{name}\" are not three different nodes \
                     apart from the star point: {}",
                        nodes.join(", ")
                    ),
                );
            }
        }
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (k, v) in t.rated_voltage_kv.iter().enumerate() {
            if !positive(*v) {
                range_error(
                    d,
                    p,
                    name,
                    &format!("rated_voltage_kv[{k}]"),
                    *v,
                    "Rated line-to-line voltages must be > 0 kV.",
                );
            }
        }
        check_transformer_ratings(
            p,
            name,
            [
                t.rated_power_mva,
                t.rated_frequency_hz,
                t.impedance_percent,
                t.resistance_percent,
                t.no_load_current_percent,
                t.no_load_loss_kw,
            ],
            d,
        );
        if let Some(s) = &t.saturation {
            s.check(
                &format!("{p}.saturation"),
                name,
                [
                    t.no_load_current_percent,
                    t.no_load_loss_kw,
                    t.rated_power_mva,
                ],
                d,
            );
        }
        match VectorGroup::parse(&t.vector_group) {
            None => d.push(Diagnostic::error(
                "E135",
                format!("{p}.vector_group"),
                format!(
                    "vector group \"{}\" of \"{name}\" is not supported",
                    t.vector_group
                ),
                format!(
                    "Write the high-voltage letter in upper case (Y, YN or D), the low-voltage \
                 letter in lower case (y, yn or d) and the clock number, such as YNd11 or Dyn11. \
                 Supported (with or without N/n): {}.",
                    VECTOR_GROUPS.join(", ")
                ),
            )),
            Some(g) => {
                for (field, given, side) in [
                    ("hv_neutral", &t.hv_neutral, g.hv),
                    ("lv_neutral", &t.lv_neutral, g.lv),
                ] {
                    if given.is_some() && side != (WindingConnection::Star { neutral: true }) {
                        d.push(Diagnostic::error(
                            "E136",
                            format!("{p}.{field}"),
                            format!(
                                "{field} is given, but vector group \"{}\" has no star point \
                             brought out on that side",
                                t.vector_group
                            ),
                            "Write YN (high voltage) or yn (low voltage) in the vector group to \
                         bring the star point out, or remove the field.",
                        ));
                    }
                }
                let isolated = |w| w == WindingConnection::Star { neutral: false };
                if isolated(g.hv) && isolated(g.lv) && t.no_load_current_percent == 0.0 {
                    d.push(Diagnostic::error(
                        "E137",
                        format!("{p}.vector_group"),
                        format!(
                            "both star points of \"{name}\" ({}) are isolated and there is no \
                         magnetizing branch, so the zero-sequence voltage of the windings is \
                         not determined",
                            t.vector_group
                        ),
                        "Bring a star point out (YNy0, Yyn0) or give no_load_current_percent > 0.",
                    ));
                }
            }
        }
        check_tap_changes(
            p,
            name,
            &t.tap_changes,
            t.impedance_percent,
            t.resistance_percent,
            d,
        );
    }
    fn connections(&self) -> Vec<(String, String)> {
        let t = self;
        let neutrals = t.neutral_nodes();
        let mut v = Vec::new();
        for (k, b) in t.buses.iter().enumerate() {
            let n = b.nodes();
            match &neutrals[k] {
                // Phases to the brought-out star point.
                Some(star) => v.extend(n.iter().map(|p| (p.clone(), star.clone()))),
                // Isolated star point or delta: the phases are connected
                // to each other through the windings.
                None => {
                    v.push((n[0].clone(), n[1].clone()));
                    v.push((n[1].clone(), n[2].clone()));
                }
            }
        }
        v
    }
    fn has_branch_voltage(&self) -> bool {
        false
    }
    fn has_separate_windings(&self) -> bool {
        true
    }
    // Three units between the phases and the star points of the two sides.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let group = VectorGroup::parse(&self.vector_group);
        let Some((g, ends)) = group.and_then(|g| Some((g, winding_ends(&g)?))) else {
            // The vector group is an error of its own.
            let held = |(a, b)| Branch::Conductance(a, b);
            return self.connections().into_iter().map(held).collect();
        };
        let given = self.neutral_nodes();
        let sides = [self.buses[0].nodes(), self.buses[1].nodes()];
        let mut stars = [String::new(), String::new()];
        let mut winding_voltage = [0.0; 2];
        for (k, w) in [g.hv, g.lv].into_iter().enumerate() {
            let line = self.rated_voltage_kv[k];
            (stars[k], winding_voltage[k]) = match w {
                WindingConnection::Star { neutral: true } => (
                    given[k].clone().unwrap_or_else(ground),
                    line / 3.0_f64.sqrt(),
                ),
                WindingConnection::Star { neutral: false } => (
                    format!("{}:{}n", self.name, ["hv.", "lv."][k]),
                    line / 3.0_f64.sqrt(),
                ),
                WindingConnection::Delta => (String::new(), line),
            };
        }
        let node = |side: usize, end: End, p: usize| match end {
            Some(o) => sides[side][(p + o) % 3].as_str(),
            None => stars[side].as_str(),
        };
        let mut v = Vec::new();
        for (p, ph) in super::PHASES.iter().enumerate() {
            unit_branches(
                &format!("{}:{ph}.", self.name),
                [
                    node(0, ends[0].0, p),
                    node(0, ends[0].1, p),
                    node(1, ends[1].0, p),
                    node(1, ends[1].1, p),
                ],
                winding_voltage[0] / winding_voltage[1],
                [
                    self.no_load_current_percent,
                    self.no_load_loss_kw / self.rated_power_mva,
                ],
                self.saturation.is_some(),
                &mut v,
            );
        }
        v
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_transformer",
            category: "circuit",
            summary: "Three-phase two-winding transformer as a bank of three single-phase units (no three-limb zero-sequence coupling), connected by vector_group. The clock number n means the low-voltage side lags by n*30 degrees (IEC 60076-1). Supported: Yy0, Yy6, Yd1, Yd5, Yd7, Yd11, Dy1, Dy5, Dy7, Dy11, Dd0, Dd6, with N/n for a star point brought out.",
            sign_convention: "Line currents I(<name>.0.a) etc. flow from the high-voltage bus into the transformer, I(<name>.1.a) etc. from the low-voltage bus. Percentages are on the three-phase rating and the rated line-to-line voltages.",
            parameters: vec![
                catalog::name(),
                p("buses", "", true, "[high-voltage bus, low-voltage bus]; each a bus or three node names."),
                p("vector_group", "", true, "Such as \"YNd11\" or \"Dyn11\": HV letter in upper case, LV letter in lower case, clock number."),
                p("rated_voltage_kv", "kV", true, "[high, low] line-to-line, rms, > 0."),
                p("rated_power_mva", "MVA", true, "Three-phase rating, > 0."),
                p("rated_frequency_hz", "Hz", true, "Frequency at which impedance_percent holds, > 0."),
                p("impedance_percent", "%", true, IMPEDANCE_PERCENT),
                p("resistance_percent", "%", false, RESISTANCE_PERCENT),
                p("no_load_current_percent", "%", false, NO_LOAD_CURRENT),
                p("no_load_loss_kw", "kW", false, "Three-phase core loss, >= 0, at most no_load_current_percent of the rating. Default 0."),
                p("hv_neutral", "", false, "Node of the high-voltage star point, only with YN. Default \"gnd\"."),
                p("lv_neutral", "", false, "Node of the low-voltage star point, only with yn. Default \"gnd\"."),
                p("saturation", "pu", false, "Saturation of the magnetizing branch (format 0.5): {\"current_pu\":[..],\"flux_pu\":[..]}, points of flux linkage against magnetizing current, > 0 and strictly increasing, at least 2, per unit of the peak values at the ratings of winding 0 of one unit (a third of the rating, the voltage of a high-voltage winding) (flux sqrt(2) V / (2 pi f), current sqrt(2) S / V). The origin is implied, the characteristic is symmetric and has no hysteresis. flux_pu[0] / current_pu[0] must be the unsaturated magnetizing reactance in per unit (within 1 percent); the last segment continues as the air-core inductance. The flux linkage is the signal <name>.mag.a.flux_wb (b, c). Default: linear."),
                p("tap_changes", "s, kV, %", false, "Scheduled tap changes (format 0.6): [{\"time_s\": .., \"rated_voltage_kv\": [hv, lv], ...}] in strictly ascending order of time_s."),
            ],
            example: json!({"type": "three_phase_transformer", "name": "T1", "buses": ["b2", "b3"], "vector_group": "YNyn0", "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0,
                "rated_frequency_hz": 50.0, "impedance_percent": 10.0, "resistance_percent": 0.5, "no_load_current_percent": 0.5, "no_load_loss_kw": 20.0, "hv_neutral": "gnd", "lv_neutral": "n3",
                "saturation": {"current_pu": [0.005451, 0.25], "flux_pu": [1.1, 1.35]},
                "tap_changes": [{"time_s": 0.1, "rated_voltage_kv": [67.65, 6.6]}]}),
        }
    }
}

/// Connection of one side of a three-phase transformer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindingConnection {
    /// Star, with the star point brought out (N) or isolated.
    Star {
        neutral: bool,
    },
    Delta,
}

/// A parsed vector group such as "YNd11".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorGroup {
    pub hv: WindingConnection,
    pub lv: WindingConnection,
    /// The low-voltage side lags by clock * 30 degrees.
    pub clock: u8,
}

/// Vector groups OpenKeito supports, written without N/n.
pub const VECTOR_GROUPS: [&str; 12] = [
    "Yy0", "Yy6", "Yd1", "Yd5", "Yd7", "Yd11", "Dy1", "Dy5", "Dy7", "Dy11", "Dd0", "Dd6",
];

impl VectorGroup {
    /// Parses "YNd11", "Dyn11", "Yy0" and so on: the high-voltage letter in
    /// upper case (Y, YN or D), the low-voltage letter in lower case (y, yn
    /// or d), then the clock number. `None` for anything else, including
    /// combinations not in VECTOR_GROUPS.
    pub fn parse(s: &str) -> Option<VectorGroup> {
        let (hv, rest) = if let Some(r) = s.strip_prefix("YN") {
            (WindingConnection::Star { neutral: true }, r)
        } else if let Some(r) = s.strip_prefix('Y') {
            (WindingConnection::Star { neutral: false }, r)
        } else {
            (WindingConnection::Delta, s.strip_prefix('D')?)
        };
        let (lv, digits) = if let Some(r) = rest.strip_prefix("yn") {
            (WindingConnection::Star { neutral: true }, r)
        } else if let Some(r) = rest.strip_prefix('y') {
            (WindingConnection::Star { neutral: false }, r)
        } else {
            (WindingConnection::Delta, rest.strip_prefix('d')?)
        };
        if digits.is_empty() || digits.len() > 2 || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let clock: u8 = digits.parse().ok()?;
        let letter = |w: WindingConnection| match w {
            WindingConnection::Star { .. } => 'y',
            WindingConnection::Delta => 'd',
        };
        let key = format!("{}{}{clock}", letter(hv).to_ascii_uppercase(), letter(lv));
        VECTOR_GROUPS
            .contains(&key.as_str())
            .then_some(VectorGroup { hv, lv, clock })
    }
}

/// Ratings of a transformer: [rated_power_mva, rated_frequency_hz,
/// impedance_percent, resistance_percent, no_load_current_percent,
/// no_load_loss_kw].
fn check_transformer_ratings(p: &str, name: &str, v: [f64; 6], d: &mut Vec<Diagnostic>) {
    let [power, frequency, z, r, i0, loss] = v;
    for (field, value, hint) in [
        ("rated_power_mva", power, "Rated power must be > 0 MVA."),
        (
            "rated_frequency_hz",
            frequency,
            "The frequency at which impedance_percent holds, such as 50 or 60.",
        ),
        (
            "impedance_percent",
            z,
            "The short-circuit impedance u_k must be > 0 percent, typically 4 to 20.",
        ),
    ] {
        if !positive(value) {
            range_error(d, p, name, field, value, hint);
        }
    }
    for (field, value) in [
        ("resistance_percent", r),
        ("no_load_current_percent", i0),
        ("no_load_loss_kw", loss),
    ] {
        if !value.is_finite() || value < 0.0 {
            range_error(d, p, name, field, value, "Use a finite value >= 0.");
        }
    }
    if positive(z) && r.is_finite() && r >= z {
        order_error(
            d,
            format!("{p}.resistance_percent"),
            format!("resistance_percent ({r}) of \"{name}\" is not less than impedance_percent ({z})"),
            "The resistive part u_r is part of the short-circuit impedance u_k and must be smaller; \
             the leakage reactance is sqrt(u_k^2 - u_r^2).",
        );
    }
    // Core loss in percent of the rating is the conductance of the
    // magnetizing branch; it cannot exceed the admittance magnitude.
    if positive(power) && i0.is_finite() && loss.is_finite() && loss > 0.0 {
        let loss_percent = 100.0 * loss / (1000.0 * power);
        if loss_percent > i0 {
            order_error(
                d,
                format!("{p}.no_load_loss_kw"),
                format!(
                    "no_load_loss_kw ({loss}) of \"{name}\" is {loss_percent:.4} percent of the rating, \
                     more than no_load_current_percent ({i0})"
                ),
                "The core loss is the resistive part of the no-load current: give \
                 no_load_current_percent >= 100 * no_load_loss_kw / (1000 * rated_power_mva), or set the \
                 loss to 0.",
            );
        }
    }
}

/// Validates scheduled tap changes of a `transformer` or
/// `three_phase_transformer`.
fn check_tap_changes(
    p: &str,
    name: &str,
    changes: &[TransformerTapChange],
    default_z: f64,
    default_r: f64,
    d: &mut Vec<Diagnostic>,
) {
    let mut prev: Option<f64> = None;
    for (k, ch) in changes.iter().enumerate() {
        let cp = format!("{p}.tap_changes[{k}]");
        if !ch.time_s.is_finite() || ch.time_s < 0.0 {
            range_error(
                d,
                &cp,
                name,
                "time_s",
                ch.time_s,
                "Tap change times must be >= 0 s.",
            );
        } else {
            if let Some(last) = prev {
                if ch.time_s <= last {
                    order_error(
                        d,
                        format!("{cp}.time_s"),
                        format!(
                            "tap_changes[{k}].time_s ({}) of \"{name}\" is not after the previous time ({last})",
                            ch.time_s
                        ),
                        "List tap_changes in strictly ascending order of time_s.",
                    );
                }
            }
            prev = Some(ch.time_s);
        }
        for (j, &v) in ch.rated_voltage_kv.iter().enumerate() {
            if !positive(v) {
                range_error(
                    d,
                    &cp,
                    name,
                    &format!("rated_voltage_kv[{j}]"),
                    v,
                    "Rated voltages must be > 0 kV.",
                );
            }
        }
        let z = ch.impedance_percent.unwrap_or(default_z);
        let r = ch.resistance_percent.unwrap_or(default_r);
        if let Some(zv) = ch.impedance_percent {
            if !positive(zv) {
                range_error(
                    d,
                    &cp,
                    name,
                    "impedance_percent",
                    zv,
                    "The short-circuit impedance u_k must be > 0 percent.",
                );
            }
        }
        if let Some(rv) = ch.resistance_percent {
            if !rv.is_finite() || rv < 0.0 {
                range_error(
                    d,
                    &cp,
                    name,
                    "resistance_percent",
                    rv,
                    "Use a finite value >= 0.",
                );
            }
        }
        if positive(z)
            && r.is_finite()
            && r >= z
            && (ch.impedance_percent.is_some() || ch.resistance_percent.is_some())
        {
            order_error(
                d,
                format!("{cp}.resistance_percent"),
                format!("resistance_percent ({r}) of \"{name}\" at tap_changes[{k}] is not less than impedance_percent ({z})"),
                "The resistive part u_r is part of the short-circuit impedance u_k and must be smaller.",
            );
        }
    }
}

/// A scheduled tap change of a `three_phase_ideal_transformer` (format 0.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdealTransformerTapChange {
    /// Time at which the ratio and phase shift take these values, in seconds (>= 0).
    pub time_s: f64,
    /// Rated line-to-line voltages [side 0, side 1], rms, in kV, > 0.
    pub rated_voltage_kv: [f64; 2],
    /// Angle by which side 1 leads side 0 in the positive sequence, in degrees. Default 0.
    #[serde(default)]
    pub phase_shift_deg: f64,
}

/// An ideal three-phase transformer, without leakage, magnetizing or losses.
/// In phase voltages to gnd, V(buses[1]) = (v1 / v0) T V(buses[0]), where T
/// turns the positive sequence by +phase_shift_deg and the negative sequence
/// by -phase_shift_deg and keeps the zero sequence (or inverts it when
/// `zero_sequence_inverted` is true): a symmetric phase shifter, and with 0
/// degrees an ideal YNyn0 (or with 180 degrees and `zero_sequence_inverted`
/// an ideal YNyn6) bank. No power is stored or lost. With a series coupled_rl
/// and a shunt three_phase_load it builds the equivalent circuits of
/// power-flow transformers, phase-shifting ones included.
///
/// Line currents I(<name>.0.a) etc. flow from buses[0] into the transformer,
/// I(<name>.1.a) etc. from buses[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseIdealTransformer {
    pub name: String,
    pub buses: [BusRef; 2],
    /// Rated line-to-line voltages of side 0 and side 1, rms, in kV; the
    /// ratio is their quotient. Must be > 0.
    pub rated_voltage_kv: [f64; 2],
    /// Angle by which side 1 leads side 0 in the positive sequence, in
    /// degrees. Default 0.
    #[serde(default)]
    pub phase_shift_deg: f64,
    /// Invert the zero-sequence ratio to -(v1 / v0) instead of +(v1 / v0),
    /// as in a grounded-star to grounded-star transformer with clock 6
    /// (YNyn6) whose windings have opposite polarity (format 0.6).
    /// Default false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zero_sequence_inverted: bool,
    /// Scheduled tap changes at times >= 0 s, in strictly ascending order
    /// (format 0.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tap_changes: Vec<IdealTransformerTapChange>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ThreePhaseIdealTransformer {
    /// N with V(buses[1]) = N V(buses[0]) in phase quantities a, b, c:
    /// N_ij = (v1 / v0) (z + 2 cos(alpha - 120 deg (i - j))) / 3,
    /// with z = -1 when `zero_sequence_inverted` is true and +1 otherwise.
    pub fn ratio_matrix_for(
        rated_voltage_kv: [f64; 2],
        phase_shift_deg: f64,
        zero_sequence_inverted: bool,
    ) -> [[f64; 3]; 3] {
        let rho = rated_voltage_kv[1] / rated_voltage_kv[0];
        let alpha = phase_shift_deg.to_radians();
        let zero = if zero_sequence_inverted { -1.0 } else { 1.0 };
        let step = 2.0 * std::f64::consts::PI / 3.0;
        let mut n = [[0.0; 3]; 3];
        for (i, row) in n.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x = rho * (zero + 2.0 * (alpha - step * (i as f64 - j as f64)).cos()) / 3.0;
            }
        }
        n
    }

    /// Ratio matrix of the initial tap position.
    pub fn ratio_matrix(&self) -> [[f64; 3]; 3] {
        Self::ratio_matrix_for(
            self.rated_voltage_kv,
            self.phase_shift_deg,
            self.zero_sequence_inverted,
        )
    }

    /// Ratio matrix at time `t` (taking the last scheduled tap with
    /// `time_s <= t`).
    pub fn ratio_matrix_at(&self, t: f64) -> [[f64; 3]; 3] {
        let mut v = self.rated_voltage_kv;
        let mut alpha = self.phase_shift_deg;
        for ch in &self.tap_changes {
            if ch.time_s <= t {
                v = ch.rated_voltage_kv;
                alpha = ch.phase_shift_deg;
            }
        }
        Self::ratio_matrix_for(v, alpha, self.zero_sequence_inverted)
    }
}

impl ComponentSpec for ThreePhaseIdealTransformer {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[".0", ".1"]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_ideal_transformer"
    }
    fn since(&self) -> &'static str {
        if self.zero_sequence_inverted || !self.tap_changes.is_empty() {
            "0.6"
        } else {
            "0.4"
        }
    }
    fn node_names(&self) -> Vec<String> {
        self.buses.iter().flat_map(BusRef::nodes).collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::series_terminals(&self.buses)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (k, b) in t.buses.iter().enumerate() {
            bus(d, b, format!("{p}.buses[{k}]"));
            let nodes = b.nodes();
            if nodes[0] == nodes[1] || nodes[1] == nodes[2] || nodes[0] == nodes[2] {
                same(
                    d,
                    format!("{p}.buses[{k}]"),
                    format!("the phases of side {k} of \"{name}\" are not three different nodes"),
                );
            }
        }
        let [a, b] = [t.buses[0].nodes(), t.buses[1].nodes()];
        if a.iter().any(|n| b.contains(n)) {
            same(
                d,
                format!("{p}.buses[1]"),
                format!("the two sides of \"{name}\" share a node"),
            );
        }
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (t, name) = (self, self.name.as_str());
        for (k, &v) in t.rated_voltage_kv.iter().enumerate() {
            if !(v.is_finite() && v > 0.0) {
                range_error(
                    d,
                    p,
                    name,
                    &format!("rated_voltage_kv[{k}]"),
                    v,
                    "Rated line-to-line voltages must be > 0 kV; their quotient is the ratio.",
                );
            }
        }
        if !t.phase_shift_deg.is_finite() {
            range_error(
                d,
                p,
                name,
                "phase_shift_deg",
                t.phase_shift_deg,
                "Use a finite angle in degrees.",
            );
        }
        let mut prev: Option<f64> = None;
        for (k, ch) in t.tap_changes.iter().enumerate() {
            let cp = format!("{p}.tap_changes[{k}]");
            if !ch.time_s.is_finite() || ch.time_s < 0.0 {
                range_error(
                    d,
                    &cp,
                    name,
                    "time_s",
                    ch.time_s,
                    "Tap change times must be >= 0 s.",
                );
            } else {
                if let Some(last) = prev {
                    if ch.time_s <= last {
                        order_error(
                            d,
                            format!("{cp}.time_s"),
                            format!(
                                "tap_changes[{k}].time_s ({}) of \"{name}\" is not after the previous time ({last})",
                                ch.time_s
                            ),
                            "List tap_changes in strictly ascending order of time_s.",
                        );
                    }
                }
                prev = Some(ch.time_s);
            }
            for (j, &v) in ch.rated_voltage_kv.iter().enumerate() {
                if !(v.is_finite() && v > 0.0) {
                    range_error(
                        d,
                        &cp,
                        name,
                        &format!("rated_voltage_kv[{j}]"),
                        v,
                        "Rated line-to-line voltages must be > 0 kV; their quotient is the ratio.",
                    );
                }
            }
            if !ch.phase_shift_deg.is_finite() {
                range_error(
                    d,
                    &cp,
                    name,
                    "phase_shift_deg",
                    ch.phase_shift_deg,
                    "Use a finite angle in degrees.",
                );
            }
        }
    }
    // V(buses[1]) = N V(buses[0]) with N invertible: either side fixes the
    // other.
    fn connections(&self) -> Vec<(String, String)> {
        phase_pairs(&self.buses)
    }
    fn has_branch_voltage(&self) -> bool {
        false
    }
    fn has_separate_windings(&self) -> bool {
        true
    }
    // One equation per phase of side 1: V(side 1) - N V(side 0) = 0.
    fn branches(&self, t: f64) -> Vec<Branch> {
        let [side0, side1] = [self.buses[0].nodes(), self.buses[1].nodes()];
        let n = self.ratio_matrix_at(t);
        let largest = n.iter().flatten().fold(1.0_f64, |m, x| m.max(x.abs()));
        let mut v = Vec::new();
        for (r, row) in n.iter().enumerate() {
            let mut terms = vec![(side1[r].clone(), 1.0)];
            for (c, x) in row.iter().enumerate() {
                // Without a phase shift the entries off the diagonal are 0
                // to rounding: they join no phases.
                if x.abs() > 1e-13 * largest {
                    terms.push((side0[c].clone(), -x));
                }
            }
            v.push(Branch::Constraint(terms));
        }
        v
    }
    fn switching_times(&self) -> Vec<f64> {
        self.tap_changes.iter().map(|c| c.time_s).collect()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_ideal_transformer",
            category: "circuit",
            summary: "Ideal three-phase transformer without leakage, magnetizing or losses: V(buses[1]) = (v1/v0) T V(buses[0]) in phase voltages to ground, T turning the positive sequence by +phase_shift_deg, the negative sequence by -phase_shift_deg and keeping (or, with zero_sequence_inverted, inverting) the zero sequence (a symmetric phase shifter; with 0 degrees an ideal YNyn0 bank, with 180 degrees and zero_sequence_inverted an ideal YNyn6 bank). With a series coupled_rl and a shunt three_phase_load it builds the equivalent circuits of power-flow transformers.",
            sign_convention: "Line currents I(<name>.0.a) etc. flow from buses[0] into the transformer, I(<name>.1.a) etc. from buses[1]. Side 1 leads side 0 by phase_shift_deg in the positive sequence.",
            parameters: vec![
                catalog::name(),
                p("buses", "", true, "[side 0, side 1], each a bus or three node names."),
                p("rated_voltage_kv", "kV", true, "[side 0, side 1] line-to-line voltages, rms, > 0; the ratio is their quotient."),
                p("phase_shift_deg", "deg", false, "Angle by which side 1 leads side 0 in the positive sequence. Default 0."),
                p("zero_sequence_inverted", "", false, "Invert the zero-sequence ratio to -(v1/v0), as in YNyn6 (format 0.6). Default false."),
                p("tap_changes", "s, kV, deg", false, "Scheduled tap changes (format 0.6): [{\"time_s\": .., \"rated_voltage_kv\": [v0, v1], \"phase_shift_deg\": ..}] in strictly ascending order of time_s."),
            ],
            example: json!({"type": "three_phase_ideal_transformer", "name": "PST1", "buses": ["b4", "b4i"],
                "rated_voltage_kv": [400.0, 110.0], "phase_shift_deg": 2.15, "zero_sequence_inverted": true,
                "tap_changes": [{"time_s": 0.1, "rated_voltage_kv": [400.0, 112.0], "phase_shift_deg": 4.0}]}),
        }
    }
}
