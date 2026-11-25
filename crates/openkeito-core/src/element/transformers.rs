//! Ideal transformers: the single-phase one and the three-phase one with a
//! ratio matrix.

use super::{add, Element, Node, Stage, SwitchAction, PHASE_NAMES};
use crate::linalg::{Complex64, Scalar, Stamp};

/// Ideal transformer with turns ratio n: V(p1) - V(q1) = n (V(p2) - V(q2)),
/// and the current i into p1 leaves winding 2 as -n i into p2 (no power is
/// stored or lost). Unknown: i.
pub(crate) struct IdealTransformer {
    pub name: String,
    pub p1: Node,
    pub q1: Node,
    pub p2: Node,
    pub q2: Node,
    pub ratio: f64,
    pub k: usize,
    pub changes: Vec<(f64, f64)>,
    pub next_change: usize,
    pub event_name: Option<String>,
}

impl Element for IdealTransformer {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.next_change < self.changes.len() && self.changes[self.next_change].0 <= t + tol {
            self.ratio = self.changes[self.next_change].1;
            self.next_change += 1;
            changed = true;
        }
        if changed {
            if let Some(name) = &self.event_name {
                actions.push((name.clone(), SwitchAction::Step));
            }
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.changes.iter().map(|c| c.0).collect()
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        let k = Some(self.k);
        let n = self.ratio;
        add(m, self.p1, k, 1.0);
        add(m, self.q1, k, -1.0);
        add(m, self.p2, k, -n);
        add(m, self.q2, k, n);
        add(m, k, self.p1, 1.0);
        add(m, k, self.q1, -1.0);
        add(m, k, self.p2, -n);
        add(m, k, self.q2, n);
    }
    /// 0: winding 1 (into p1); 1: winding 2 (into p2).
    fn current(&self, k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        if k == 0 {
            x[self.k]
        } else {
            -self.ratio * x[self.k]
        }
    }
    fn current_phasor(&self, k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        if k == 0 {
            x[self.k]
        } else {
            -self.ratio * x[self.k]
        }
    }
}

/// Ideal three-phase transformer V(side1) = N V(side0) (phase voltages to
/// gnd). Unknowns: the currents j into the transformer at side 1; the
/// currents into it at side 0 are -N^T j, so that V0^T i0 + V1^T j = 0.
pub(crate) struct MatrixTransformer {
    pub name: String,
    pub side0: [Node; 3],
    pub side1: [Node; 3],
    pub n: [[f64; 3]; 3],
    pub k: usize,
    pub changes: Vec<(f64, [[f64; 3]; 3])>,
    pub next_change: usize,
}

impl MatrixTransformer {
    /// 0..3: currents into side 0; 3..6: into side 1.
    fn line_current<T: Scalar>(&self, k: usize, j: [T; 3]) -> T {
        if k < 3 {
            let mut s = T::ZERO;
            for (r, jr) in j.iter().enumerate() {
                s = s - jr.scale(self.n[r][k]);
            }
            s
        } else {
            j[k - 3]
        }
    }
}

impl Element for MatrixTransformer {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        3
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn aux_label(&self, k: usize) -> String {
        format!("I({}.1.{})", self.name, PHASE_NAMES[k])
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.next_change < self.changes.len() && self.changes[self.next_change].0 <= t + tol {
            self.n = self.changes[self.next_change].1;
            self.next_change += 1;
            changed = true;
        }
        if changed {
            actions.push((self.name.clone(), SwitchAction::Step));
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.changes.iter().map(|c| c.0).collect()
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        for r in 0..3 {
            let k = Some(self.k + r);
            add(m, self.side1[r], k, 1.0);
            add(m, k, self.side1[r], 1.0);
            for c in 0..3 {
                add(m, self.side0[c], k, -self.n[r][c]);
                add(m, k, self.side0[c], -self.n[r][c]);
            }
        }
    }
    fn current(&self, k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        self.line_current(k, [x[self.k], x[self.k + 1], x[self.k + 2]])
    }
    fn current_phasor(&self, k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        self.line_current(k, [x[self.k], x[self.k + 1], x[self.k + 2]])
    }
}
