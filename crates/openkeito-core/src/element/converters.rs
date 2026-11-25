//! The source of an averaged converter.

use super::{phasor, Element, SignalResponse, Stage};
use crate::element::sources::ControlledVoltageSource;
use crate::linalg::{Complex64, Stamp};
use crate::sim::Stages;

/// One phase of the source behind the reactor of an averaged converter.
///
/// In the run it is a voltage source that follows a control signal, like
/// `ControlledVoltageSource`. In a phasor solution it is a current source:
/// the controls of a converter hold its current, not its voltage, so the
/// steady state is the one in which the converter injects the current of its
/// operating point, whatever voltage the network then has at its terminals.
/// (A voltage phasor computed beforehand from an assumed terminal voltage
/// would turn every error of that assumption into a current through the
/// small reactance.) The voltage the source has in that steady state follows
/// from the solution and is where the run starts. With every source at zero
/// (a frequency scan) the converter is an open circuit.
pub(crate) struct ConverterSource {
    pub source: ControlledVoltageSource,
    /// Phasor of the current injected into `a` in the steady-state
    /// initialization (rms, cosine reference).
    pub injection: Complex64,
}

impl Element for ConverterSource {
    fn name(&self) -> &str {
        self.source.name()
    }
    fn aux_count(&self) -> usize {
        self.source.aux_count()
    }
    fn set_aux(&mut self, index: usize) {
        self.source.set_aux(index);
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        self.source.stamp(alpha, m);
    }
    fn structure(&self, m: &mut dyn Stamp) {
        self.source.stamp(1.0, m);
        // The phasor equation of the branch current.
        m.add(self.source.k, self.source.k, 1.0);
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        self.source.prepare(stages)
    }
    fn rhs(&self, t: f64, stage: &Stage, b: &mut [f64]) {
        self.source.rhs(t, stage, b);
    }
    fn current(&self, k: usize, t: f64, stage: &Stage, x: &[f64]) -> f64 {
        self.source.current(k, t, stage, x)
    }
    fn reads_signals(&self) -> bool {
        self.source.reads_signals()
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        self.source.read_signals(signals)
    }
    fn stamp_phasor(&self, _omega: f64, m: &mut dyn Stamp<Complex64>) {
        // The branch current flows from `a` to `b` through the source: it
        // leaves `a`, so it is minus the injection.
        let k = self.source.k;
        super::add(m, self.source.a, Some(k), Complex64::ONE);
        super::add(m, self.source.b, Some(k), -Complex64::ONE);
        m.add(k, k, Complex64::ONE);
    }
    fn rhs_phasor(&self, _omega: f64, b: &mut [Complex64]) {
        b[self.source.k] -= self.injection;
    }
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        self.source.current_phasor(k, omega, x)
    }
    fn init_from_phasor(&mut self, _omega: f64, x: &[Complex64]) {
        let v = phasor(x, self.source.a) - phasor(x, self.source.b);
        self.source.value.set(super::instant(v));
    }
}
