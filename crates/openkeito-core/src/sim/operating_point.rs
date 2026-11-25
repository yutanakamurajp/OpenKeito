//! The iteration of a stage for elements whose operating point follows from
//! its solution (the segment of a piecewise-linear characteristic, the
//! speed terms of a machine).
//!
//! Every solution of a stage is followed by this iteration: the stages of a
//! step, the solution at t = 0+ and the trial solution that settles the
//! diode states (so that the diodes are judged on a solution the nonlinear
//! elements agree with). The solutions at t = 0+ and of the trial are not
//! kept, but the operating points they leave are: an operating point is the
//! guess for the next solution, which is iterated in its turn, so it needs
//! no `save` and `restore`. The states of diodes are not decided here; they
//! change at grid points only (`switching`).
//!
//! The iteration stays within a part of the circuit (`parts`): a part
//! whose elements agree with its solution is not solved again when another
//! part iterates (`PartRun::stage`).

/// Largest number of additional solutions of one stage. A piecewise-linear
/// element that moves to the segment of the solution settles within a few;
/// 50 is far beyond that and still cheap for a stage that fails.
pub const MAX_OPERATING_POINT_ITERATIONS: u64 = 50;
