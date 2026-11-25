//! Nonlinear components with piecewise-linear characteristics: the
//! nonlinear resistor (surge arrester), the nonlinear inductor (saturation)
//! and the saturation characteristic of a transformer.
//!
//! Every characteristic ends up as a list of points [x, y] with strictly
//! increasing x, joined by straight lines and continued beyond both ends with
//! the slope of the outermost segments. The solver takes the points from
//! `points()`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ComponentInitialization, ComponentSpec};
use crate::catalog::{p, with, ComponentInfo, COMMON_SIGN};
use crate::check::{positive, range_error};
use crate::diagnostic::Diagnostic;

/// Largest number of points of a characteristic, as written or after the
/// conversion of a power law.
pub const MAX_CHARACTERISTIC_POINTS: usize = 10_000;

/// Largest relative difference between the first slope of a transformer's
/// saturation characteristic and its magnetizing reactance.
const FIRST_SLOPE_TOLERANCE: f64 = 0.01;

fn default_voltage_tolerance() -> f64 {
    1e-3
}

/// Nonlinear resistor with a piecewise-linear characteristic i(v): in each
/// segment it is a conductance in parallel with a current source. The
/// segment is found within every stage of a time step, so the element
/// follows its characteristic without a delay. Beyond the outermost points
/// the characteristic continues with the slope of the outermost segments.
///
/// A metal-oxide surge arrester is written either as a table or with the
/// power-law segments i = p (v / V_ref)^q of its data sheet, which are
/// converted into a table when the model is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NonlinearResistor {
    pub name: String,
    pub nodes: [String; 2],
    pub characteristic: Characteristic,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// The characteristic i(v) of a nonlinear resistor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Characteristic {
    /// Points (voltage_v[k], current_a[k]) joined by straight lines.
    Table {
        /// Voltages of the points in volt, strictly increasing. With
        /// `symmetric`, all > 0.
        voltage_v: Vec<f64>,
        /// Currents of the points in ampere, not decreasing (a segment of
        /// constant current is an open circuit there). With `symmetric`,
        /// all >= 0.
        current_a: Vec<f64>,
        /// False (default): the points cover negative and positive voltages
        /// and include the point 0 V, 0 A. True: the points are those of
        /// positive voltages; the origin is implied and the negative side
        /// is the mirror image, i(-v) = -i(v).
        #[serde(default)]
        symmetric: bool,
    },
    /// Segments i = coefficient_a (v / reference_voltage_v)^exponent, each
    /// from its from_voltage_pu up to that of the next one; the last one up
    /// to max_current_a. Below the first from_voltage_pu the characteristic
    /// is a straight line through the origin. Symmetric: i(-v) = -i(v).
    /// Converted into a table whose voltage, at any current up to
    /// max_current_a, is at most relative_voltage_tolerance below that of
    /// the power law.
    PowerLaw {
        /// V_ref in volt (a peak value, as v is). Must be > 0.
        reference_voltage_v: f64,
        /// At least one segment, in the order of increasing from_voltage_pu.
        segments: Vec<PowerLawSegment>,
        /// Current in ampere at which the table ends. Must be above the
        /// current at the start of the last segment.
        max_current_a: f64,
        /// Largest relative error of the table in voltage, 1e-6 to 0.1.
        /// Default 1e-3.
        #[serde(default = "default_voltage_tolerance")]
        relative_voltage_tolerance: f64,
    },
}

/// One segment i = coefficient_a (v / V_ref)^exponent of a power law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PowerLawSegment {
    /// p in ampere: the current at v = V_ref. Must be > 0.
    pub coefficient_a: f64,
    /// q. Must be >= 1.
    pub exponent: f64,
    /// v / V_ref from which the segment holds. Must be > 0.
    pub from_voltage_pu: f64,
}

/// What is wrong with a list of points, for E260 and E261.
enum TableError {
    /// E260: the lists cannot be read as points.
    Shape(String),
    /// E261: the points are not in order; the field and the index.
    Order(&'static str, usize, String),
}

/// Checks two lists that give the points (x[k], y[k]) of one side of a
/// characteristic. `positive_only`: every x must be > 0 and every y >= 0
/// (or > 0 with `strict_y`). x must increase strictly; y must not decrease
/// (or increase strictly with `strict_y`).
fn check_points(
    (x_field, x): (&'static str, &[f64]),
    (y_field, y): (&'static str, &[f64]),
    positive_only: bool,
    strict_y: bool,
) -> Result<(), TableError> {
    if x.len() != y.len() {
        return Err(TableError::Shape(format!(
            "{x_field} has {} values and {y_field} has {}",
            x.len(),
            y.len()
        )));
    }
    if x.len() < 2 {
        return Err(TableError::Shape(format!(
            "the characteristic has {} point(s)",
            x.len()
        )));
    }
    if x.len() > MAX_CHARACTERISTIC_POINTS {
        return Err(TableError::Shape(format!(
            "the characteristic has {} points, more than {MAX_CHARACTERISTIC_POINTS}",
            x.len()
        )));
    }
    for (field, values) in [(x_field, x), (y_field, y)] {
        if let Some(k) = values.iter().position(|v| !v.is_finite()) {
            return Err(TableError::Shape(format!(
                "{field}[{k}] is not a finite number"
            )));
        }
    }
    if positive_only {
        if x[0] <= 0.0 {
            return Err(TableError::Order(
                x_field,
                0,
                format!("{x_field}[0] is {}, not > 0", x[0]),
            ));
        }
        if y[0] < 0.0 || (strict_y && y[0] == 0.0) {
            return Err(TableError::Order(
                y_field,
                0,
                format!(
                    "{y_field}[0] is {}, not {} 0",
                    y[0],
                    if strict_y { ">" } else { ">=" }
                ),
            ));
        }
    }
    for k in 1..x.len() {
        if x[k] <= x[k - 1] {
            return Err(TableError::Order(
                x_field,
                k,
                format!(
                    "{x_field}[{k}] = {} is not above {x_field}[{}] = {}",
                    x[k],
                    k - 1,
                    x[k - 1]
                ),
            ));
        }
        if y[k] < y[k - 1] || (strict_y && y[k] == y[k - 1]) {
            return Err(TableError::Order(
                y_field,
                k,
                format!(
                    "{y_field}[{k}] = {} is {} {y_field}[{}] = {}",
                    y[k],
                    if strict_y { "not above" } else { "below" },
                    k - 1,
                    y[k - 1]
                ),
            ));
        }
    }
    Ok(())
}

/// Reports what `check_points` found. `p` is the path of the object that
/// holds the lists.
fn report_points(result: Result<(), TableError>, p: &str, name: &str, d: &mut Vec<Diagnostic>) {
    match result {
        Ok(()) => {}
        Err(TableError::Shape(what)) => d.push(Diagnostic::error(
            "E260",
            p.to_string(),
            format!("the characteristic of \"{name}\" cannot be read: {what}"),
            format!(
                "Give two lists of the same length with 2 to {MAX_CHARACTERISTIC_POINTS} finite \
                 numbers: the points of the characteristic, one value of each list per point."
            ),
        )),
        Err(TableError::Order(field, k, what)) => d.push(Diagnostic::error(
            "E261",
            format!("{p}.{field}[{k}]"),
            format!("the characteristic of \"{name}\" is not monotonic: {what}"),
            "List the points in increasing order. A characteristic that goes back (negative \
             resistance or inductance) has no unique solution and is not supported.",
        )),
    }
}

/// The points (x, y) and their mirror images, without the origin: the
/// segment through the origin runs from (-x[0], -y[0]) to (x[0], y[0]).
fn mirrored(x: &[f64], y: &[f64]) -> Vec<[f64; 2]> {
    let negative = x.iter().zip(y).rev().map(|(x, y)| [-x, -y]);
    let positive = x.iter().zip(y).map(|(x, y)| [*x, *y]);
    negative.chain(positive).collect()
}

/// Largest relative amount by which the chord of u^q over [1, ratio] lies
/// below the curve in u at equal current: max of 1 - u / c(u)^(1/q), with
/// c(u) the chord.
fn chord_voltage_error(q: f64, ratio: f64) -> f64 {
    const SAMPLES: usize = 64;
    let slope = (ratio.powf(q) - 1.0) / (ratio - 1.0);
    (1..SAMPLES)
        .map(|k| {
            let u = 1.0 + (ratio - 1.0) * k as f64 / SAMPLES as f64;
            1.0 - u / (1.0 + slope * (u - 1.0)).powf(1.0 / q)
        })
        .fold(0.0, f64::max)
}

/// Largest ln(v[k+1] / v[k]) of the table of u^q whose chords stay within
/// `tolerance` in voltage, but not more than `span`. For small steps d the
/// error is (q - 1) d^2 / 8; the exact value is checked.
fn log_step(q: f64, tolerance: f64, span: f64) -> f64 {
    if q <= 1.0 {
        return span;
    }
    let mut d = (8.0 * tolerance / (q - 1.0)).sqrt().min(span);
    while chord_voltage_error(q, d.exp()) > tolerance {
        d *= 0.9;
    }
    d
}

impl Characteristic {
    /// The points [voltage in volt, current in ampere] of a characteristic
    /// that passed validation, over negative and positive voltages.
    pub fn points(&self) -> Vec<[f64; 2]> {
        match self {
            Characteristic::Table {
                voltage_v,
                current_a,
                symmetric: true,
            } => mirrored(voltage_v, current_a),
            Characteristic::Table {
                voltage_v,
                current_a,
                symmetric: false,
            } => voltage_v
                .iter()
                .zip(current_a)
                .map(|(v, i)| [*v, *i])
                .collect(),
            Characteristic::PowerLaw { .. } => {
                let (v, i) = self.power_law_side().unwrap_or_default();
                mirrored(&v, &i)
            }
        }
    }

    /// The positive side of the table of a power law: voltages and currents
    /// of its points. `Err` with the number of points when they are more
    /// than `MAX_CHARACTERISTIC_POINTS`. Call only with parameters in
    /// range.
    fn power_law_side(&self) -> Result<(Vec<f64>, Vec<f64>), usize> {
        let Characteristic::PowerLaw {
            reference_voltage_v,
            segments,
            max_current_a,
            relative_voltage_tolerance,
        } = self
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let mut count = 0usize;
        for (k, s) in segments.iter().enumerate() {
            let start = s.from_voltage_pu;
            let end = match segments.get(k + 1) {
                Some(next) => next.from_voltage_pu,
                None => (max_current_a / s.coefficient_a).powf(1.0 / s.exponent),
            };
            let span = (end / start).ln();
            let step = log_step(s.exponent, *relative_voltage_tolerance, span);
            let n = (span / step).ceil().max(1.0);
            count = count.saturating_add(n as usize);
            if count > MAX_CHARACTERISTIC_POINTS {
                // Count on without building the table.
                continue;
            }
            for j in 0..n as usize {
                let u = start * (span * j as f64 / n).exp();
                v.push(u * reference_voltage_v);
                i.push(s.coefficient_a * u.powf(s.exponent));
            }
            if k + 1 == segments.len() {
                v.push(end * reference_voltage_v);
                i.push(*max_current_a);
            }
        }
        if count + 1 > MAX_CHARACTERISTIC_POINTS {
            return Err(count + 1);
        }
        Ok((v, i))
    }

    /// True when the characteristic has a segment of constant current, in
    /// which the element is an open circuit.
    fn has_open_segment(&self) -> bool {
        match self {
            Characteristic::Table {
                current_a,
                symmetric,
                ..
            } => {
                (*symmetric && current_a.first() == Some(&0.0))
                    || current_a.windows(2).any(|w| w[0] == w[1])
            }
            Characteristic::PowerLaw { .. } => false,
        }
    }

    fn check(&self, p: &str, name: &str, d: &mut Vec<Diagnostic>) {
        match self {
            Characteristic::Table {
                voltage_v,
                current_a,
                symmetric,
            } => {
                let result = check_points(
                    ("voltage_v", voltage_v),
                    ("current_a", current_a),
                    *symmetric,
                    false,
                );
                let ok = result.is_ok();
                report_points(result, p, name, d);
                if ok && !symmetric {
                    let origin = voltage_v
                        .iter()
                        .zip(current_a)
                        .any(|(v, i)| *v == 0.0 && *i == 0.0);
                    if !origin {
                        d.push(Diagnostic::error(
                            "E262",
                            p.to_string(),
                            format!(
                                "the characteristic of \"{name}\" does not contain the point \
                                 0 V, 0 A"
                            ),
                            "A resistor carries no current at 0 V: add the point (0, 0) to \
                             voltage_v and current_a, with the points of negative voltages before \
                             it. For a characteristic with i(-v) = -i(v), give the points of \
                             positive voltages only and set \"symmetric\": true.",
                        ));
                    }
                }
            }
            Characteristic::PowerLaw {
                reference_voltage_v,
                segments,
                max_current_a,
                relative_voltage_tolerance,
            } => {
                let before = d.len();
                if !positive(*reference_voltage_v) {
                    range_error(
                        d,
                        p,
                        name,
                        "reference_voltage_v",
                        *reference_voltage_v,
                        "The reference voltage V_ref of i = p (v / V_ref)^q must be > 0 V.",
                    );
                }
                let tolerance = *relative_voltage_tolerance;
                if !(1e-6..=0.1).contains(&tolerance) {
                    range_error(
                        d,
                        p,
                        name,
                        "relative_voltage_tolerance",
                        tolerance,
                        "Use a value from 1e-6 to 0.1; the default 1e-3 keeps the voltage of the \
                         table within 0.1 percent of the power law.",
                    );
                }
                if segments.is_empty() {
                    d.push(Diagnostic::error(
                        "E260",
                        format!("{p}.segments"),
                        format!("the power law of \"{name}\" has no segments"),
                        "Give at least one segment {\"coefficient_a\": p, \"exponent\": q, \
                         \"from_voltage_pu\": v_min / V_ref}.",
                    ));
                }
                for (k, s) in segments.iter().enumerate() {
                    let at = format!("{p}.segments[{k}]");
                    if !positive(s.coefficient_a) {
                        range_error(
                            d,
                            &at,
                            name,
                            "coefficient_a",
                            s.coefficient_a,
                            "The coefficient p of i = p (v / V_ref)^q must be > 0 A.",
                        );
                    }
                    if !(s.exponent.is_finite() && s.exponent >= 1.0) {
                        range_error(
                            d,
                            &at,
                            name,
                            "exponent",
                            s.exponent,
                            "The exponent q of i = p (v / V_ref)^q must be >= 1.",
                        );
                    }
                    if !positive(s.from_voltage_pu) {
                        range_error(
                            d,
                            &at,
                            name,
                            "from_voltage_pu",
                            s.from_voltage_pu,
                            "The voltage v_min / V_ref from which the segment holds must be > 0.",
                        );
                    }
                }
                if !positive(*max_current_a) {
                    range_error(
                        d,
                        p,
                        name,
                        "max_current_a",
                        *max_current_a,
                        "The current at which the table ends must be > 0 A.",
                    );
                }
                if d.len() == before {
                    self.check_power_law_table(p, name, d);
                }
            }
        }
    }

    /// The order of the segments of a power law with parameters in range,
    /// and its table.
    fn check_power_law_table(&self, p: &str, name: &str, d: &mut Vec<Diagnostic>) {
        let Characteristic::PowerLaw {
            segments,
            max_current_a,
            relative_voltage_tolerance,
            ..
        } = self
        else {
            return;
        };
        let current = |s: &PowerLawSegment, u: f64| s.coefficient_a * u.powf(s.exponent);
        for (k, pair) in segments.windows(2).enumerate() {
            let (a, b) = (&pair[0], &pair[1]);
            let at = format!("{p}.segments[{}].from_voltage_pu", k + 1);
            if b.from_voltage_pu <= a.from_voltage_pu {
                d.push(Diagnostic::error(
                    "E263",
                    at,
                    format!(
                        "from_voltage_pu of segment {} of \"{name}\" ({}) is not above that of \
                         segment {k} ({})",
                        k + 1,
                        b.from_voltage_pu,
                        a.from_voltage_pu
                    ),
                    "List the segments in the order of increasing from_voltage_pu.",
                ));
                return;
            }
            // The currents of the two segments where the second one starts.
            let u = b.from_voltage_pu;
            let (below, above) = (current(a, u), current(b, u));
            // Where the two power laws meet.
            let meet = (b.exponent != a.exponent)
                .then(|| (a.coefficient_a / b.coefficient_a).powf(1.0 / (b.exponent - a.exponent)));
            let meet_hint = match meet {
                Some(m) if m.is_finite() && m > a.from_voltage_pu => format!(
                    "The two segments meet at v / V_ref = {m:.6}: set from_voltage_pu of segment \
                     {} to it.",
                    k + 1
                ),
                _ => "Choose the coefficients so that the two segments give the same current at \
                      from_voltage_pu of the second one."
                    .to_string(),
            };
            // The last point that the table takes from the first of the two.
            let span = (u / a.from_voltage_pu).ln();
            let n = (span / log_step(a.exponent, *relative_voltage_tolerance, span))
                .ceil()
                .max(1.0);
            let reached = current(a, a.from_voltage_pu * (span * (n - 1.0) / n).exp());
            if above < reached {
                d.push(Diagnostic::error(
                    "E263",
                    at,
                    format!(
                        "the current of \"{name}\" falls where segment {} starts: segment {k} \
                         has reached {reached:e} A, segment {} begins with {above:e} A",
                        k + 1,
                        k + 1
                    ),
                    meet_hint,
                ));
                return;
            }
            // As a difference in voltage at equal current.
            let jump = (above / below).ln().abs() / a.exponent.min(b.exponent);
            if jump > *relative_voltage_tolerance {
                d.push(Diagnostic::warning(
                    "W261",
                    at,
                    format!(
                        "the segments {k} and {} of \"{name}\" do not meet at v / V_ref = {u}: \
                         {below:e} A against {above:e} A; the table joins them with a straight \
                         line from the last point of segment {k}",
                        k + 1
                    ),
                    meet_hint,
                ));
            }
        }
        let last = &segments[segments.len() - 1];
        let start = current(last, last.from_voltage_pu);
        if *max_current_a <= start {
            d.push(Diagnostic::error(
                "E263",
                format!("{p}.max_current_a"),
                format!(
                    "max_current_a of \"{name}\" ({max_current_a}) is not above the current at \
                     the start of the last segment ({start:e} A)"
                ),
                "max_current_a is where the table ends: give the largest current the element is \
                 expected to carry, such as twice the nominal discharge current of an arrester.",
            ));
            return;
        }
        if let Err(points) = self.power_law_side() {
            d.push(Diagnostic::error(
                "E263",
                format!("{p}.relative_voltage_tolerance"),
                format!(
                    "the table of the power law of \"{name}\" would have {points} points or \
                     more, above the limit of {MAX_CHARACTERISTIC_POINTS}"
                ),
                "Raise relative_voltage_tolerance (the default is 1e-3), or lower max_current_a.",
            ));
        }
    }
}

impl ComponentSpec for NonlinearResistor {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "nonlinear_resistor"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        self.characteristic
            .check(&format!("{p}.characteristic"), &self.name, d);
    }
    // In a segment of constant current the element is an open circuit: it
    // does not hold a node that nothing else connects to gnd.
    fn connections(&self) -> Vec<(String, String)> {
        if self.characteristic.has_open_segment() {
            return Vec::new();
        }
        let [a, b] = &self.nodes;
        vec![(a.clone(), b.clone())]
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "nonlinear_resistor",
            category: "circuit",
            summary: "Nonlinear resistor with a piecewise-linear characteristic i(v), such as a metal-oxide surge arrester. The segment is found within each stage of a step (no delay); beyond the outermost points the outermost segments continue. Steady-state initialization and frequency scans use the segment through the origin (warning W260 when the steady state leaves it).",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![p(
                "characteristic",
                "V, A",
                true,
                "{\"type\":\"table\",\"voltage_v\":[..],\"current_a\":[..],\"symmetric\":false} (voltages strictly increasing, currents not decreasing; with symmetric true only the points of positive voltages, the origin implied and i(-v) = -i(v); without it the points must include 0 V, 0 A) | {\"type\":\"power_law\",\"reference_voltage_v\":..,\"segments\":[{\"coefficient_a\":p,\"exponent\":q,\"from_voltage_pu\":..}],\"max_current_a\":..,\"relative_voltage_tolerance\":1e-3} (i = p (v/V_ref)^q per segment, a straight line through the origin below the first from_voltage_pu, symmetric; converted into a table whose voltage is within relative_voltage_tolerance of the power law up to max_current_a).",
            )]),
            example: json!({"type": "nonlinear_resistor", "name": "MOV1", "nodes": ["n1", "gnd"],
                "characteristic": {"type": "table", "voltage_v": [200e3, 250e3, 300e3], "current_a": [1.0, 1000.0, 20000.0], "symmetric": true}}),
        }
    }
}

/// Nonlinear inductor with a piecewise-linear characteristic between its
/// flux linkage and its current, for the saturation of an iron core. The
/// characteristic passes through the origin and is symmetric; hysteresis
/// and remanence are not modelled. Its state is the flux linkage,
/// d(flux)/dt = v; beyond the last point the last segment continues (the
/// air-core inductance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NonlinearInductor {
    pub name: String,
    pub nodes: [String; 2],
    /// Currents of the points in ampere (peak values), > 0 and strictly
    /// increasing.
    pub current_a: Vec<f64>,
    /// Flux linkages of the points in weber (volt second), > 0 and strictly
    /// increasing. The origin is implied; the negative side is the mirror
    /// image.
    pub flux_linkage_wb: Vec<f64>,
    /// Flux linkage at t = 0 in weber; the current follows from the
    /// characteristic. Default 0.
    #[serde(default)]
    pub initial_flux_linkage_wb: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl NonlinearInductor {
    /// The points [flux linkage in weber, current in ampere] over negative
    /// and positive flux.
    pub fn points(&self) -> Vec<[f64; 2]> {
        mirrored(&self.flux_linkage_wb, &self.current_a)
    }
}

impl ComponentSpec for NonlinearInductor {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "nonlinear_inductor"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn signal_names(&self) -> Vec<&'static str> {
        vec!["flux_wb"]
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        report_points(
            check_points(
                ("flux_linkage_wb", &self.flux_linkage_wb),
                ("current_a", &self.current_a),
                true,
                true,
            ),
            p,
            &self.name,
            d,
        );
        if !self.initial_flux_linkage_wb.is_finite() {
            range_error(
                d,
                p,
                &self.name,
                "initial_flux_linkage_wb",
                self.initial_flux_linkage_wb,
                "Use a finite value.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "nonlinear_inductor",
            category: "circuit",
            summary: "Nonlinear inductor (saturation) with a piecewise-linear characteristic between flux linkage and current, through the origin and symmetric, without hysteresis. State: the flux linkage, d(flux)/dt = v. The segment is found within each stage of a step; beyond the last point the last segment continues (air-core inductance). Steady-state initialization and frequency scans use the first segment (warning W260 when the steady state leaves it). The flux linkage is the signal <name>.flux_wb.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![
                p("current_a", "A", true, "Currents of the points (peak values), > 0 and strictly increasing; at least 2."),
                p("flux_linkage_wb", "Wb", true, "Flux linkages of the points (volt seconds), > 0 and strictly increasing. The origin is implied; the negative side is the mirror image."),
                p("initial_flux_linkage_wb", "Wb", false, "Flux linkage at t = 0. Default 0."),
            ]),
            example: json!({"type": "nonlinear_inductor", "name": "L1", "nodes": ["n2", "gnd"],
                "current_a": [1.0, 100.0], "flux_linkage_wb": [30.0, 36.0], "initial_flux_linkage_wb": 0.0}),
        }
    }
}

/// Saturation of the magnetizing branch of a transformer: points of flux
/// linkage against magnetizing current, per unit of the peak values at the
/// ratings of the winding the branch is on (winding 0). The flux base is
/// sqrt(2) V / (2 pi f), the flux linkage at rated voltage; the current
/// base is sqrt(2) S / V. The origin is implied and the characteristic is
/// symmetric. The first segment is the unsaturated branch: its slope
/// flux_pu[0] / current_pu[0] must be the magnetizing reactance in per unit
/// that no_load_current_percent and no_load_loss_kw give (within 1 percent;
/// the characteristic is what the solver uses). The last segment continues
/// beyond the last point: its slope is the air-core inductance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Saturation {
    /// Magnetizing currents of the points in per unit, > 0 and strictly
    /// increasing; at least 2.
    pub current_pu: Vec<f64>,
    /// Flux linkages of the points in per unit, > 0 and strictly
    /// increasing.
    pub flux_pu: Vec<f64>,
}

impl Saturation {
    /// The points [flux linkage in weber, current in ampere] over negative
    /// and positive flux, for a winding of rated voltage `voltage_v` (rms)
    /// and rating `power_va` at the angular frequency `omega`.
    pub fn points(&self, voltage_v: f64, power_va: f64, omega: f64) -> Vec<[f64; 2]> {
        let flux_base = std::f64::consts::SQRT_2 * voltage_v / omega;
        let current_base = std::f64::consts::SQRT_2 * power_va / voltage_v;
        let flux: Vec<f64> = self.flux_pu.iter().map(|f| f * flux_base).collect();
        let current: Vec<f64> = self.current_pu.iter().map(|i| i * current_base).collect();
        mirrored(&flux, &current)
    }

    /// Checks the points and their first slope against the magnetizing
    /// branch of a transformer with no-load current `i0_percent`, core loss
    /// `loss_kw` and rating `power_mva` (all in range).
    pub(crate) fn check(
        &self,
        p: &str,
        name: &str,
        [i0_percent, loss_kw, power_mva]: [f64; 3],
        d: &mut Vec<Diagnostic>,
    ) {
        let result = check_points(
            ("flux_pu", &self.flux_pu),
            ("current_pu", &self.current_pu),
            true,
            true,
        );
        let ok = result.is_ok();
        report_points(result, p, name, d);
        let y = i0_percent / 100.0;
        let g = loss_kw / (1000.0 * power_mva);
        if !(y.is_finite() && g.is_finite() && y >= 0.0 && g >= 0.0 && g <= y) {
            // Reported with the ratings.
            return;
        }
        let b = (y * y - g * g).sqrt();
        if b == 0.0 {
            d.push(Diagnostic::error(
                "E264",
                p.to_string(),
                format!(
                    "\"{name}\" has a saturation characteristic but no magnetizing inductance \
                     (no_load_current_percent is {i0_percent}, of which the core loss takes all)"
                ),
                "The first segment of the characteristic is the unsaturated magnetizing branch: \
                 give no_load_current_percent > 0 (above 100 * no_load_loss_kw / (1000 * \
                 rated_power_mva)), or remove \"saturation\".",
            ));
            return;
        }
        if !ok {
            return;
        }
        let slope = self.flux_pu[0] / self.current_pu[0];
        if (slope * b - 1.0).abs() > FIRST_SLOPE_TOLERANCE {
            d.push(Diagnostic::error(
                "E264",
                format!("{p}.current_pu[0]"),
                format!(
                    "the first segment of the saturation characteristic of \"{name}\" has the \
                     slope {slope:.6} pu, but no_load_current_percent and no_load_loss_kw give a \
                     magnetizing reactance of {:.6} pu",
                    1.0 / b
                ),
                format!(
                    "The first point lies on the unsaturated branch: set current_pu[0] to \
                     flux_pu[0] times the magnetizing susceptance, {:.6e}, or change \
                     no_load_current_percent so that the susceptance is {:.6e} pu.",
                    self.flux_pu[0] * b,
                    1.0 / slope
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power_law(tolerance: f64) -> Characteristic {
        // Two segments that meet at v / V_ref = 1.2:
        // 2 * 1.2^10 = p * 1.2^30 gives p = 2 * 1.2^-20.
        Characteristic::PowerLaw {
            reference_voltage_v: 100e3,
            segments: vec![
                PowerLawSegment {
                    coefficient_a: 2.0,
                    exponent: 10.0,
                    from_voltage_pu: 0.8,
                },
                PowerLawSegment {
                    coefficient_a: 2.0 * 1.2_f64.powi(-20),
                    exponent: 30.0,
                    from_voltage_pu: 1.2,
                },
            ],
            max_current_a: 20e3,
            relative_voltage_tolerance: tolerance,
        }
    }

    /// Voltage of the power law of `power_law` at the current `i`.
    fn exact_voltage(i: f64) -> f64 {
        let meet = 2.0 * 1.2_f64.powi(10);
        let u = if i < meet {
            (i / 2.0).powf(0.1)
        } else {
            (i / (2.0 * 1.2_f64.powi(-20))).powf(1.0 / 30.0)
        };
        100e3 * u
    }

    #[test]
    fn the_table_of_a_power_law_stays_within_its_tolerance_in_voltage() {
        // The chords of a convex curve lie above it in current, so below it
        // in voltage at equal current. The spacing uses most of the
        // tolerance: the error is between a third of it and all of it.
        for tolerance in [1e-2, 1e-3, 1e-4] {
            let c = power_law(tolerance);
            let mut d = Vec::new();
            c.check("c", "MOV1", &mut d);
            assert!(d.is_empty(), "{d:?}");
            let points = c.points();
            let side = &points[points.len() / 2..];
            assert_eq!(side[0], [80e3, 2.0 * 0.8_f64.powi(10)]);
            assert_eq!(side[side.len() - 1][1], 20e3);
            let mut worst = 0.0_f64;
            for w in side.windows(2) {
                for k in 0..=20 {
                    let f = k as f64 / 20.0;
                    let v = w[0][0] + f * (w[1][0] - w[0][0]);
                    let i = w[0][1] + f * (w[1][1] - w[0][1]);
                    let e = 1.0 - v / exact_voltage(i);
                    assert!(e > -1e-12, "the chord lies above the curve: {e}");
                    worst = worst.max(e);
                }
            }
            assert!(
                worst <= tolerance && worst > tolerance / 3.0,
                "tolerance {tolerance}: error {worst}, {} points",
                side.len()
            );
            // The negative side is the mirror image.
            for (a, b) in points.iter().zip(points.iter().rev()) {
                assert_eq!([a[0], a[1]], [-b[0], -b[1]]);
            }
        }
        // The step in ln v is sqrt(8 tolerance / (q - 1)): the number of
        // points grows as 1 / sqrt(tolerance), by 10 from 1e-2 to 1e-4.
        let n = |t: f64| power_law(t).points().len() as f64;
        let growth = n(1e-4) / n(1e-2);
        assert!((7.0..13.0).contains(&growth), "{growth}");
    }

    #[test]
    fn segments_that_do_not_meet_give_a_warning_with_the_voltage_where_they_would() {
        let Characteristic::PowerLaw {
            reference_voltage_v,
            mut segments,
            max_current_a,
            relative_voltage_tolerance,
        } = power_law(1e-3)
        else {
            unreachable!()
        };
        segments[1].from_voltage_pu = 1.25;
        let c = Characteristic::PowerLaw {
            reference_voltage_v,
            segments,
            max_current_a,
            relative_voltage_tolerance,
        };
        let mut d = Vec::new();
        c.check("c", "MOV1", &mut d);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "W261");
        assert_eq!(d[0].path, "c.segments[1].from_voltage_pu");
        assert!(d[0].hint.contains("1.200000"), "{}", d[0].hint);
        // The table is still monotonic.
        let points = c.points();
        assert!(points
            .windows(2)
            .all(|w| w[1][0] > w[0][0] && w[1][1] >= w[0][1]));
    }

    #[test]
    fn a_tolerance_that_needs_too_many_points_is_an_error() {
        let c = Characteristic::PowerLaw {
            reference_voltage_v: 1.0,
            segments: vec![PowerLawSegment {
                coefficient_a: 1.0,
                exponent: 50.0,
                from_voltage_pu: 0.01,
            }],
            max_current_a: 1e3,
            relative_voltage_tolerance: 1e-6,
        };
        let mut d = Vec::new();
        c.check("c", "MOV1", &mut d);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "E263");
        assert!(d[0].hint.contains("relative_voltage_tolerance"));
    }
}
