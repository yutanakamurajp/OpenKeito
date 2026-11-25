//! Solver core of OpenKeito.
//!
//! The circuit is written as modified nodal equations. Inductors and
//! capacitors become a conductance and a history current source whose values
//! follow from the integration method (see `docs/numerical-method.md`), so one
//! time step is one or two solutions of a linear system with a constant
//! matrix. The matrix is factorized again only when a switch changes state.

pub mod clock;
mod control;
mod element;
mod linalg;
mod netlist;
mod phasor;
mod sim;
mod sparse;

pub use element::{Element, Node, OperatingPoint, SignalResponse, Stage, SwitchAction, Tolerance};
pub use linalg::{Complex64, DenseMatrix, Factors, Lu, Scalar, Solver, Stamp};
pub use netlist::{component_nodes, ComponentNodes};
pub use phasor::{FrequencyScanner, ScanColumn, ScanStats};
#[doc(hidden)]
pub use sim::last_detail_control_s;
pub use sim::{
    Column, PhasorValue, RunStats, SimError, Simulator, Stages, SteadyStateReport, SwitchEvent,
    Timing, MAX_OPERATING_POINT_ITERATIONS,
};
pub use sparse::{SparseLu, SparseSymbolic, Triplets};
