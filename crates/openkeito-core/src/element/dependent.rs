//! Dependent sources of modified nodal analysis: a constant gain relates the
//! output to a voltage or a current of the same solution (Ho, Ruehli and
//! Brennan 1975; Vlach and Singhal 1993).
//!
//! None has a state: the stamps do not use the stage coefficient, the
//! right-hand side is 0, and the phasor stamp is the real one (the default
//! `stamp_phasor`). The stamps are not symmetric: the column of an output
//! current lies in the node equations of the output, its row (or the
//! entries of a source of current) at the nodes that control it.

use super::{add, phasor, stamp_branch_current, volt, Element, Node, Stage};
use crate::linalg::{Complex64, Stamp};

/// V(p) - V(q) = gain (V(cp) - V(cq)). Unknown: the output current from p
/// through the source to q.
pub(crate) struct Vcvs {
    pub name: String,
    pub p: Node,
    pub q: Node,
    pub cp: Node,
    pub cq: Node,
    pub gain: f64,
    pub k: usize,
}

impl Element for Vcvs {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn is_dependent_source(&self) -> bool {
        true
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.p, self.q, self.k, true);
        let k = Some(self.k);
        add(m, k, self.cp, -self.gain);
        add(m, k, self.cq, self.gain);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        x[self.k]
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        x[self.k]
    }
}

/// The current gain (V(cp) - V(cq)) flows from p through the source to q.
/// No unknown of its own.
pub(crate) struct Vccs {
    pub name: String,
    pub p: Node,
    pub q: Node,
    pub cp: Node,
    pub cq: Node,
    pub gain: f64,
}

impl Element for Vccs {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_dependent_source(&self) -> bool {
        true
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        let g = self.gain;
        add(m, self.p, self.cp, g);
        add(m, self.p, self.cq, -g);
        add(m, self.q, self.cp, -g);
        add(m, self.q, self.cq, g);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        self.gain * (volt(x, self.cp) - volt(x, self.cq))
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        self.gain * (phasor(x, self.cp) - phasor(x, self.cq))
    }
}

/// The current gain I(sense) flows from p through the source to q; the
/// sense port is a branch of 0 V from sp to sq. Unknown: the sense current.
pub(crate) struct Cccs {
    pub name: String,
    pub p: Node,
    pub q: Node,
    pub sp: Node,
    pub sq: Node,
    pub gain: f64,
    pub k: usize,
}

impl Element for Cccs {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn aux_label(&self, _k: usize) -> String {
        format!("I({}.sense)", self.name)
    }
    fn is_dependent_source(&self) -> bool {
        true
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.sp, self.sq, self.k, true);
        let k = Some(self.k);
        add(m, self.p, k, self.gain);
        add(m, self.q, k, -self.gain);
    }
    /// 0: the output current; 1: the sense current.
    fn current(&self, k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        if k == 0 {
            self.gain * x[self.k]
        } else {
            x[self.k]
        }
    }
    fn current_phasor(&self, k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        if k == 0 {
            self.gain * x[self.k]
        } else {
            x[self.k]
        }
    }
}

/// V(p) - V(q) = gain I(sense), with the sense port of a `Cccs`. Unknowns:
/// the sense current (k) and the output current (k + 1).
pub(crate) struct Ccvs {
    pub name: String,
    pub p: Node,
    pub q: Node,
    pub sp: Node,
    pub sq: Node,
    pub gain: f64,
    pub k: usize,
}

impl Element for Ccvs {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        2
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn aux_label(&self, k: usize) -> String {
        if k == 0 {
            format!("I({}.sense)", self.name)
        } else {
            format!("I({})", self.name)
        }
    }
    fn is_dependent_source(&self) -> bool {
        true
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.sp, self.sq, self.k, true);
        let out = self.k + 1;
        stamp_branch_current(m, self.p, self.q, out, true);
        m.add(out, self.k, -self.gain);
    }
    /// 0: the output current; 1: the sense current.
    fn current(&self, k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        if k == 0 {
            x[self.k + 1]
        } else {
            x[self.k]
        }
    }
    fn current_phasor(&self, k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        if k == 0 {
            x[self.k + 1]
        } else {
            x[self.k]
        }
    }
}
