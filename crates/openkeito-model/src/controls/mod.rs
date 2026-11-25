//! Control blocks.
//!
//! A block type is a structure with an implementation of [`ControlSpec`]:
//! its inputs and outputs, the checks of its parameters, the format version
//! that added it and its catalog entry. To add a type, write both in a module
//! here and add one line to `control_types!` below.
//!
//! A block with one output gives the signal `<name>`; a block with several
//! gives the signals `<name>.<output>` (`ControlSpec::outputs`).

mod blocks;
mod dynamic;
mod measurement;
mod nonlinear;
mod time;
mod transforms;
mod vector;

use std::collections::{BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use blocks::*;
pub use dynamic::*;
pub use measurement::*;
pub use nonlinear::*;
pub use time::*;
pub use transforms::*;
pub use vector::*;

use crate::catalog::ComponentInfo;
use crate::check::{finite, order_error};
use crate::components::Component;
use crate::diagnostic::Diagnostic;

/// What a block may refer to in the circuit, and the time step of the run.
pub(crate) struct Scope<'a> {
    /// Every node used by a component.
    pub nodes: &'a BTreeSet<&'a str>,
    pub component_names: &'a BTreeSet<&'a str>,
    pub components: &'a [Component],
    /// `simulation.time_step_s`; not finite or not positive when that field
    /// is wrong (reported on its own).
    pub time_step_s: f64,
}

/// What validation and the catalog need to know about a block type.
pub(crate) trait ControlSpec {
    /// Name of the block and of its output signal.
    fn name(&self) -> &str;

    /// Value of the `type` field.
    fn type_name(&self) -> &'static str;

    /// Format version this block needs ("0.2" added the controls): the
    /// version that added the type, or a later one when it uses a field that
    /// was added later.
    fn since(&self) -> &'static str;

    /// Input signals, each with the field it is written in.
    fn inputs(&self) -> Vec<(String, &str)> {
        Vec::new()
    }

    /// Names of the outputs of a block with several outputs: its signals are
    /// `<name>.<output>`. Empty for a block with one output, whose signal is
    /// `<name>`.
    fn outputs(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// False when the output at a grid point does not depend on the value
    /// that the input written in `field` has at that same grid point, in the
    /// continuous system: the input of an integrator, of a lag, of a strictly
    /// proper transfer function, of a delay of at least one time step. A loop
    /// of blocks must pass through such an input (E171); the solver then
    /// iterates the loop at every grid point.
    fn is_direct(&self, _field: &str, _time_step_s: f64) -> bool {
        true
    }

    /// Ranges and consistency of the parameters, and what the block refers
    /// to in the circuit. `p` is the path of the block, such as
    /// "controls[2]". The names of the input signals are checked elsewhere.
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>);

    /// The catalog entry of the type.
    fn info() -> ComponentInfo
    where
        Self: Sized;
}

/// The signals of a field `inputs`.
pub(crate) fn input_list(v: &[String]) -> Vec<(String, &str)> {
    v.iter()
        .enumerate()
        .map(|(i, s)| (format!("inputs[{i}]"), s.as_str()))
        .collect()
}

/// The signal of a field `input`.
pub(crate) fn one_input(s: &str) -> Vec<(String, &str)> {
    vec![("input".to_string(), s)]
}

/// A value at t = 0: a number, or the name of a signal whose value at t = 0
/// is taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, expecting = "a number or the name of a control signal")]
pub enum StartValue {
    /// The value.
    Value(f64),
    /// Name of a signal; its value at t = 0 is the initial value.
    Signal(String),
}

impl Default for StartValue {
    fn default() -> Self {
        StartValue::Value(0.0)
    }
}

impl StartValue {
    /// The signal, when the value is taken from one.
    pub fn signal(&self) -> Option<&str> {
        match self {
            StartValue::Signal(s) => Some(s),
            StartValue::Value(_) => None,
        }
    }

    /// E130 for a number that is not finite.
    pub(crate) fn check(&self, d: &mut Vec<Diagnostic>, p: &str, name: &str, field: &str) {
        if let StartValue::Value(v) = self {
            finite(d, p, name, field, *v);
        }
    }
}

/// A limit of a control block: a constant number, or (from version 0.7)
/// the name of a control signal giving the limit at each grid point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, expecting = "a number or the name of a control signal")]
pub enum LimitValue {
    /// Constant limit.
    Value(f64),
    /// Name of a control signal giving the limit.
    Signal(String),
}

impl LimitValue {
    /// Constant limit, when a number was given.
    pub fn value(&self) -> Option<f64> {
        match self {
            LimitValue::Value(v) => Some(*v),
            LimitValue::Signal(_) => None,
        }
    }

    /// The signal, when the limit is taken from one.
    pub fn signal(&self) -> Option<&str> {
        match self {
            LimitValue::Signal(s) => Some(s),
            LimitValue::Value(_) => None,
        }
    }

    /// E130 for a number that is not finite.
    pub(crate) fn check(&self, d: &mut Vec<Diagnostic>, p: &str, name: &str, field: &str) {
        if let LimitValue::Value(v) = self {
            finite(d, p, name, field, *v);
        }
    }
}

/// Optional `LimitValue` bounds: finite when constant, and the upper one
/// above the lower one when both are constants (E130, E132).
pub(crate) fn check_limit_values(
    d: &mut Vec<Diagnostic>,
    p: &str,
    name: &str,
    (low_field, low): (&str, Option<&LimitValue>),
    (high_field, high): (&str, Option<&LimitValue>),
) {
    for (field, v) in [(low_field, low), (high_field, high)] {
        if let Some(v) = v {
            v.check(d, p, name, field);
        }
    }
    if let (Some(LimitValue::Value(lo)), Some(LimitValue::Value(hi))) = (low, high) {
        if lo.is_finite() && hi.is_finite() && hi <= lo {
            order_error(
                d,
                format!("{p}.{high_field}"),
                format!("{high_field} ({hi}) of \"{name}\" is not greater than {low_field} ({lo})"),
                &format!("Use {high_field} > {low_field}, or omit a limit."),
            );
        }
    }
}

/// Declares the block types: the variants of `Control` (the structure of
/// each has the name of its variant), in the order of the catalog.
macro_rules! control_types {
    ($($variant:ident),* $(,)?) => {
/// A control block. Its `name` is the name of its output signal; a block
/// with several outputs gives the signals `<name>.<output>`.
///
/// Blocks are evaluated at every grid point, after the circuit has been
/// solved up to that point; their outputs act on the circuit over the next
/// time step (one step of delay from the controls to the circuit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
            $($variant($variant)),*
        }

        impl Control {
            /// The block as an implementation of `ControlSpec`.
            pub(crate) fn spec(&self) -> &dyn ControlSpec {
                match self {
                    $(Control::$variant(c) => c),*
                }
            }
        }

        /// The catalog entries of every block type.
        pub(crate) fn catalog() -> Vec<ComponentInfo> {
            vec![$(<$variant as ControlSpec>::info()),*]
        }
    };
}

control_types! {
    Constant,
    Gain,
    Sum,
    TriangleWave,
    Comparator,
    Pi,
    Measurement,
    TransferFunction,
    Integrator,
    Lag,
    LeadLag,
    Delay,
    Limiter,
    RateLimiter,
    DeadBand,
    PiecewiseLinear,
    Product,
    Quotient,
    Function,
    Selector,
    InitialValue,
    Step,
    Ramp,
    SineWave,
    TimeSeries,
    AbcToDq0,
    Dq0ToAbc,
    Pll,
    ThreePhaseMeasurement,
    DqLimiter,
    PiDq,
}

impl Control {
    pub fn name(&self) -> &str {
        self.spec().name()
    }

    pub fn type_name(&self) -> &'static str {
        self.spec().type_name()
    }

    /// Input signals, each with the field it is written in.
    pub fn inputs(&self) -> Vec<(String, &str)> {
        self.spec().inputs()
    }

    /// Names of the outputs of a block with several; empty for a block with
    /// one output.
    pub fn outputs(&self) -> Vec<&'static str> {
        self.spec().outputs()
    }

    /// The signals of the block: `<name>`, or `<name>.<output>` for each
    /// output of a block with several.
    pub fn signal_names(&self) -> Vec<String> {
        let outputs = self.outputs();
        if outputs.is_empty() {
            vec![self.name().to_string()]
        } else {
            outputs
                .iter()
                .map(|o| format!("{}.{o}", self.name()))
                .collect()
        }
    }

    /// False when the output at a grid point does not depend on the value
    /// that the input written in `field` has at that grid point.
    pub fn is_direct(&self, field: &str, time_step_s: f64) -> bool {
        self.spec().is_direct(field, time_step_s)
    }
}

/// Every signal of the controls, in the order of the blocks and, within a
/// block, of its outputs. The index of a signal in this list is its index in
/// the solver.
pub fn signal_names(controls: &[Control]) -> Vec<String> {
    controls.iter().flat_map(Control::signal_names).collect()
}

/// Blocks that are evaluated together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationGroup {
    /// Indices of the blocks, in the order of evaluation.
    pub blocks: Vec<usize>,
    /// True for the blocks of a loop: they are evaluated again and again at
    /// a grid point until their outputs no longer change.
    pub iterate: bool,
}

/// The order in which the blocks are evaluated at a grid point: groups whose
/// inputs come from earlier groups only. A group is one block, or the blocks
/// of a loop. A loop is allowed when it passes through an input that is not
/// direct (`Control::is_direct`); otherwise the blocks of one such loop are
/// returned as the error. Inputs that name no signal are ignored (they are
/// reported on their own).
pub fn evaluation_plan(
    controls: &[Control],
    time_step_s: f64,
) -> Result<Vec<EvaluationGroup>, Vec<usize>> {
    let mut producer: HashMap<String, usize> = HashMap::new();
    for (i, c) in controls.iter().enumerate() {
        for s in c.signal_names() {
            producer.entry(s).or_insert(i);
        }
    }
    let mut all: Vec<Vec<usize>> = Vec::with_capacity(controls.len());
    let mut direct: Vec<Vec<usize>> = Vec::with_capacity(controls.len());
    for c in controls {
        let (mut a, mut d) = (Vec::new(), Vec::new());
        for (field, s) in c.inputs() {
            if let Some(&k) = producer.get(s) {
                a.push(k);
                if c.is_direct(&field, time_step_s) {
                    d.push(k);
                }
            }
        }
        all.push(a);
        direct.push(d);
    }
    // Without the inputs that are not direct, the blocks have an order.
    evaluation_order(&direct)?;
    let mut member = vec![false; controls.len()];
    Ok(strongly_connected(&all)
        .into_iter()
        .map(|blocks| {
            let iterate = blocks.len() > 1 || all[blocks[0]].contains(&blocks[0]);
            let blocks = if iterate {
                loop_order(blocks, &all, &direct, &mut member)
            } else {
                blocks
            };
            EvaluationGroup { blocks, iterate }
        })
        .collect())
}

/// The order of the blocks of a loop within a pass: every block after the
/// blocks it reads, as far as a loop allows. Where no block is left whose
/// inputs are all evaluated, the loop is cut at inputs that are not direct:
/// the block that then goes first reads them from the pass before (or, in
/// the first pass, from the last grid point). With one cut, a pass carries a
/// change once around the whole loop; an order that ignored the inputs that
/// are not direct would need a pass for every block with a state.
fn loop_order(
    mut left: Vec<usize>,
    all: &[Vec<usize>],
    direct: &[Vec<usize>],
    member: &mut [bool],
) -> Vec<usize> {
    left.sort_unstable();
    for &i in &left {
        member[i] = true;
    }
    let mut order = Vec::with_capacity(left.len());
    while !left.is_empty() {
        // `member` is true for the blocks of the loop not yet in the order.
        let waiting = |inputs: &[usize]| inputs.iter().filter(|&&j| member[j]).count();
        let next = left
            .iter()
            .position(|&i| waiting(&all[i]) == 0)
            .or_else(|| {
                (0..left.len())
                    .filter(|&k| waiting(&direct[left[k]]) == 0)
                    .min_by_key(|&k| waiting(&all[left[k]]))
            })
            .expect("the direct inputs have an order");
        let block = left.remove(next);
        member[block] = false;
        order.push(block);
    }
    order
}

/// Order in which blocks can be evaluated so that every input is computed
/// first (`inputs[i]` lists the blocks that block i reads). On a loop,
/// returns the blocks of one loop.
pub fn evaluation_order(inputs: &[Vec<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    // Depth-first search with colours: 0 new, 1 on the stack, 2 done.
    let n = inputs.len();
    let mut colour = vec![0u8; n];
    let mut order = Vec::with_capacity(n);
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if colour[start] != 0 {
            continue;
        }
        stack.push((start, 0));
        colour[start] = 1;
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if let Some(&m) = inputs[node].get(*next) {
                *next += 1;
                match colour[m] {
                    0 => {
                        colour[m] = 1;
                        stack.push((m, 0));
                    }
                    1 => {
                        let from = stack.iter().position(|(k, _)| *k == m).unwrap_or(0);
                        return Err(stack[from..].iter().map(|(k, _)| *k).collect());
                    }
                    _ => {}
                }
            } else {
                colour[node] = 2;
                order.push(node);
                stack.pop();
            }
        }
    }
    Ok(order)
}

/// The strongly connected components of the graph in which block i points
/// to the blocks `inputs[i]`, each after the components it reads from
/// (Tarjan's algorithm, without recursion).
fn strongly_connected(inputs: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const NEW: usize = usize::MAX;
    let n = inputs.len();
    let mut index = vec![NEW; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut found = Vec::new();
    let mut next = 0;
    let mut work: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if index[start] != NEW {
            continue;
        }
        let mut enter = vec![start];
        while let Some(v) = enter.pop() {
            index[v] = next;
            low[v] = next;
            next += 1;
            stack.push(v);
            on_stack[v] = true;
            work.push((v, 0));
            while let Some(&(v, k)) = work.last() {
                if let Some(&w) = inputs[v].get(k) {
                    work.last_mut().expect("not empty").1 += 1;
                    if index[w] == NEW {
                        enter.push(w);
                        break;
                    }
                    if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                } else {
                    work.pop();
                    if let Some(&(parent, _)) = work.last() {
                        low[parent] = low[parent].min(low[v]);
                    }
                    if low[v] == index[v] {
                        let mut component = Vec::new();
                        loop {
                            let w = stack.pop().expect("v is on the stack");
                            on_stack[w] = false;
                            component.push(w);
                            if w == v {
                                break;
                            }
                        }
                        found.push(component);
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(json: serde_json::Value) -> Vec<Control> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn components_come_after_what_they_read() {
        // 0 reads 1; 1 and 2 form a loop; 3 reads 0; 4 reads itself.
        let inputs = vec![vec![1], vec![2], vec![1], vec![0], vec![4]];
        let found = strongly_connected(&inputs);
        let at = |i: usize| found.iter().position(|c| c.contains(&i)).unwrap();
        assert_eq!(found.len(), 4);
        assert_eq!(at(1), at(2));
        assert!(at(1) < at(0) && at(0) < at(3));
        assert_eq!(found[at(4)], [4]);
        // A long chain does not overflow the stack.
        let chain: Vec<Vec<usize>> = (0..100_000)
            .map(|i| if i == 0 { vec![] } else { vec![i - 1] })
            .collect();
        assert_eq!(strongly_connected(&chain).len(), 100_000);
    }

    #[test]
    fn a_loop_through_a_state_is_a_group_that_is_iterated() {
        let c = blocks(serde_json::json!([
            {"type": "sum", "name": "e", "inputs": ["r", "x"], "signs": [1.0, -1.0]},
            {"type": "integrator", "name": "x", "input": "e"},
            {"type": "constant", "name": "r", "value": 1.0},
            {"type": "gain", "name": "y", "input": "x", "gain": 2.0},
        ]));
        let plan = evaluation_plan(&c, 1e-3).unwrap();
        assert_eq!(
            plan,
            [
                EvaluationGroup {
                    blocks: vec![2],
                    iterate: false
                },
                // The integrator first: the sum reads it directly.
                EvaluationGroup {
                    blocks: vec![1, 0],
                    iterate: true
                },
                EvaluationGroup {
                    blocks: vec![3],
                    iterate: false
                },
            ]
        );
    }

    #[test]
    fn a_loop_is_cut_once_and_followed_from_there() {
        // e -> lag x -> integrator y -> e, listed against the flow of the
        // signals. The integrator goes first (it reads the lag of the pass
        // before), then the sum and the lag, each with what it reads.
        let c = blocks(serde_json::json!([
            {"type": "integrator", "name": "y", "input": "x"},
            {"type": "lag", "name": "x", "input": "e", "time_constant_s": 0.1},
            {"type": "sum", "name": "e", "inputs": ["r", "y"], "signs": [1.0, -1.0]},
            {"type": "constant", "name": "r", "value": 1.0},
        ]));
        let plan = evaluation_plan(&c, 1e-3).unwrap();
        assert_eq!(
            plan[1],
            EvaluationGroup {
                blocks: vec![0, 2, 1],
                iterate: true
            }
        );
    }

    #[test]
    fn a_loop_of_direct_blocks_has_no_plan() {
        let c = blocks(serde_json::json!([
            {"type": "gain", "name": "a", "input": "b", "gain": 0.5},
            {"type": "gain", "name": "b", "input": "a", "gain": 0.5},
        ]));
        assert_eq!(evaluation_plan(&c, 1e-3), Err(vec![0, 1]));
        // A delay breaks a loop when it is at least one time step long.
        let with_delay = |delay_s: f64| {
            blocks(serde_json::json!([
                {"type": "gain", "name": "a", "input": "b", "gain": 0.5},
                {"type": "delay", "name": "b", "input": "a", "delay_s": delay_s},
            ]))
        };
        assert!(evaluation_plan(&with_delay(1e-3), 1e-3).is_ok());
        assert!(evaluation_plan(&with_delay(0.5e-3), 1e-3).is_err());
    }

    #[test]
    fn the_signals_of_a_block_with_several_outputs_carry_its_name() {
        let c = blocks(serde_json::json!([
            {"type": "constant", "name": "k", "value": 1.0},
            {"type": "abc_to_dq0", "name": "v", "inputs": ["k", "k", "k"], "angle": "k"},
        ]));
        assert_eq!(signal_names(&c), ["k", "v.d", "v.q", "v.zero"]);
    }
}
