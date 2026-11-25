use serde::Serialize;
use serde_json::Value;

/// Machine-readable description of one component type or control block type.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    #[serde(rename = "type")]
    pub type_name: &'static str,
    /// "circuit" (an entry of `components`) or "control" (an entry of `controls`).
    pub category: &'static str,
    pub summary: &'static str,
    pub sign_convention: &'static str,
    pub parameters: Vec<ParameterInfo>,
    /// A complete instance with every field written out.
    pub example: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterInfo {
    pub name: &'static str,
    pub unit: &'static str,
    pub required: bool,
    pub description: &'static str,
}

pub(crate) const COMMON_SIGN: &str =
    "Branch current is positive from nodes[0] to nodes[1] through the component; branch voltage is V(nodes[0]) - V(nodes[1]).";

pub(crate) fn p(
    name: &'static str,
    unit: &'static str,
    required: bool,
    description: &'static str,
) -> ParameterInfo {
    ParameterInfo {
        name,
        unit,
        required,
        description,
    }
}

pub(crate) fn name() -> ParameterInfo {
    p(
        "name",
        "",
        true,
        "Unique name: letters, digits, '_', '-', '.'.",
    )
}

fn common() -> Vec<ParameterInfo> {
    vec![
        name(),
        p(
            "nodes",
            "",
            true,
            "[from, to] node names. The reference node is \"gnd\".",
        ),
    ]
}

pub(crate) const SIGNAL: &str = "The block's name is the name of its output signal. Blocks are evaluated at every grid point after the circuit has been solved up to it; their outputs act on the circuit over the next time step.";

pub(crate) const TWO_STATE: &str = "Branch current is positive from nodes[0] to nodes[1] through the element; branch voltage is V(nodes[0]) - V(nodes[1]). The element is a resistor of on_resistance_ohm or off_resistance_ohm.";

pub(crate) const BUS: &str = "A bus name, which stands for the nodes <bus>.a, <bus>.b and <bus>.c, or three node names [a, b, c].";

pub(crate) const IMPEDANCE: &str = "{\"type\":\"sequence\",\"r1_ohm\",\"l1_h\",\"r0_ohm\",\"l0_h\"} (self (Z0 + 2 Z1)/3, mutual (Z0 - Z1)/3) | {\"type\":\"matrix\",\"resistance_ohm\":[[3x3]],\"inductance_h\":[[3x3]]} (symmetric; L positive definite, R positive semidefinite).";

pub(crate) const THREE_PHASE_SOURCE_SIGN: &str = "Phase currents I(<name>.a) etc. flow from the bus into the source (as a voltage_source with nodes [bus, neutral]), so a source delivering power has currents opposite to its voltages. Phasors are rms with a cosine reference; phase b lags a by 120 degrees.";

pub(crate) const IMPEDANCE_PERCENT: &str = "Short-circuit impedance |Z| (u_k) in percent, > 0.";

pub(crate) const RESISTANCE_PERCENT: &str =
    "Resistive part u_r in percent, >= 0 and < impedance_percent. Default 0.";

pub(crate) const NO_LOAD_CURRENT: &str = "Magnitude of the magnetizing admittance (no-load current i_0) in percent, >= 0. Default 0 (no magnetizing branch).";

fn control_name() -> ParameterInfo {
    p(
        "name",
        "",
        true,
        "Unique name (shared namespace with components); also the name of the output signal.",
    )
}

pub(crate) fn resistances() -> Vec<ParameterInfo> {
    vec![
        p(
            "on_resistance_ohm",
            "ohm",
            false,
            "Resistance while conducting, > 0. Default 1e-3, the order of one press-pack device at its rated current. From a data sheet: n (V_T0 + r_T I)/I for n devices in series with threshold voltage V_T0 and slope resistance r_T, at the current I the element carries. A resistor with two values has no threshold voltage, no switching loss and no snubber.",
        ),
        p(
            "off_resistance_ohm",
            "ohm",
            false,
            "Resistance while blocking, > on_resistance_ohm. Default 1e6. From a data sheet: rated voltage / leakage current, or the grading resistance across a series connection. The leak loss is V^2 / off_resistance_ohm. An element left at the defaults that blocks more than 10 kV gives warning W271.",
        ),
    ]
}

/// The parameters of a component with `nodes` [from, to]: name, nodes and
/// `extra`.
pub(crate) fn with(extra: Vec<ParameterInfo>) -> Vec<ParameterInfo> {
    let mut v = common();
    v.extend(extra);
    v
}

/// The parameters of a control block: name and `extra`.
pub(crate) fn control(extra: Vec<ParameterInfo>) -> Vec<ParameterInfo> {
    let mut v = vec![control_name()];
    v.extend(extra);
    v
}

pub(crate) const WAVEFORM: &str = "Waveform object: {\"type\":\"dc\",\"value\":..} | {\"type\":\"step\",\"time_s\":..,\"before\":..,\"after\":..} | {\"type\":\"sine\",\"amplitude\":..,\"frequency_hz\":..,\"phase_deg\":..,\"offset\":..} (sine: offset + amplitude*sin(2*pi*f*t + phase), amplitude is the peak value; step: the value changes like a switch, at the first grid point at or after time_s, right after the output of that grid point).";

/// Every component type and control block type the solver supports, with
/// parameters, units, sign conventions and an example. A test keeps this
/// list in step with the model structures.
pub fn component_catalog() -> Vec<ComponentInfo> {
    let mut v = crate::components::catalog();
    v.extend(crate::controls::catalog());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Component, Control};

    #[test]
    fn catalog_matches_model_structures() {
        for info in component_catalog() {
            let (type_name, back) = match info.category {
                "circuit" => {
                    let c: Component =
                        serde_json::from_value(info.example.clone()).unwrap_or_else(|e| {
                            panic!("example of {} does not parse: {e}", info.type_name)
                        });
                    (c.type_name(), serde_json::to_value(&c).unwrap())
                }
                "control" => {
                    let c: Control =
                        serde_json::from_value(info.example.clone()).unwrap_or_else(|e| {
                            panic!("example of {} does not parse: {e}", info.type_name)
                        });
                    (c.type_name(), serde_json::to_value(&c).unwrap())
                }
                other => panic!("unknown category {other}"),
            };
            assert_eq!(type_name, info.type_name);
            let mut keys: Vec<&str> = info
                .example
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .filter(|k| *k != "type")
                .collect();
            keys.sort_unstable();
            let is_omitted_default = |k: &str| {
                (info.category == "circuit" && k == "initialization")
                    || (info.type_name == "pi" && k == "anti_windup")
                    || (info.type_name == "expression" && k == "variables")
            };
            if info.category == "circuit" {
                assert!(
                    info.parameters.iter().any(|p| p.name == "initialization"),
                    "{} is missing initialization parameter",
                    info.type_name
                );
            }
            if info.type_name == "pi" {
                assert!(
                    info.parameters.iter().any(|p| p.name == "anti_windup"),
                    "pi is missing anti_windup parameter"
                );
            }
            let mut params: Vec<&str> = info
                .parameters
                .iter()
                .map(|p| p.name)
                .filter(|k| !is_omitted_default(k))
                .collect();
            params.sort_unstable();
            assert_eq!(keys, params, "parameters of {}", info.type_name);
            // Every field of the structure appears in the example.
            let mut back_keys: Vec<&str> = back
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .filter(|k| *k != "type")
                .collect();
            back_keys.sort_unstable();
            assert_eq!(back_keys, params, "fields of {}", info.type_name);
        }
    }
}
