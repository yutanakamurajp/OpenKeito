//! Shared by the tests that compare runs with hand calculations in phasors
//! (`three_phase.rs`, `steady_state.rs`): complex numbers, the phasor of a
//! simulated waveform, the model around a list of components and the
//! comparison itself.
//!
//! Phasors are rms with a cosine reference: x(t) = sqrt(2) |X| cos(wt + arg X).

use std::f64::consts::PI;

pub use openkeito_core::Complex64;
use serde_json::{json, Value};

use super::Run;

pub const F: f64 = 50.0;
pub const W: f64 = 2.0 * PI * F;
pub const METHODS: [&str; 2] = ["2s-dirk", "trapezoidal"];

pub fn c(re: f64, im: f64) -> Complex64 {
    Complex64::new(re, im)
}

pub fn polar(r: f64, deg: f64) -> Complex64 {
    Complex64::from_polar(r, deg.to_radians())
}

pub fn parallel(a: Complex64, b: Complex64) -> Complex64 {
    a * b / (a + b)
}

/// e^{j 120 deg}.
pub fn a_op() -> Complex64 {
    polar(1.0, 120.0)
}

/// Magnitude and angle in degrees, under the names the hand calculations
/// use.
pub trait Phasor {
    fn abs(self) -> f64;
    fn deg(self) -> f64;
}

impl Phasor for Complex64 {
    fn abs(self) -> f64 {
        self.norm()
    }
    fn deg(self) -> f64 {
        self.arg().to_degrees()
    }
}

impl Run {
    /// Phasor at the frequency `F` of a column over the last `cycles`
    /// periods.
    pub fn phasor(&self, name: &str, cycles: usize) -> Complex64 {
        self.phasor_at(name, F, cycles)
    }

    /// Phasor at the frequency `f` of a column over its last `cycles`
    /// periods (a whole number of steps each): a discrete Fourier transform.
    pub fn phasor_at(&self, name: &str, f: f64, cycles: usize) -> Complex64 {
        let k = self.col(name);
        let per_period = (1.0 / (f * self.h)).round() as usize;
        let n = cycles * per_period;
        let start = self.rows.len() - n;
        let w = 2.0 * PI * f;
        let mut s = Complex64::new(0.0, 0.0);
        for i in start..self.rows.len() {
            s += self.rows[i][k] * Complex64::from_polar(1.0, -w * self.t[i]);
        }
        s * (2.0_f64.sqrt() / n as f64)
    }
}

/// A model of format 0.3 around `components`; `initialization` is the
/// default of the format when `None`.
pub fn model_of(
    method: &str,
    h: f64,
    end: f64,
    initialization: Option<&str>,
    components: Value,
    outputs: Value,
) -> Value {
    let mut model = json!({
        "format": "openkeito-model",
        "version": "0.3",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
        "components": components,
        "outputs": outputs,
    });
    if let Some(initialization) = initialization {
        model["simulation"]["initialization"] = initialization.into();
    }
    model
}

/// |simulated - expected| relative to |expected|, with a message.
pub fn check(name: &str, got: Complex64, expected: Complex64, tol: f64) {
    let e = (got - expected).norm() / expected.norm();
    // Printed for the record (cargo test -- --nocapture).
    eprintln!("{name}: relative error {e:.2e} (tolerance {tol:.1e})");
    assert!(
        e < tol,
        "{name}: got {:.6} at {:.4} deg, expected {:.6} at {:.4} deg (relative error {e:.2e}, tolerance {tol:.1e})",
        got.norm(),
        got.arg().to_degrees(),
        expected.norm(),
        expected.arg().to_degrees()
    );
}
