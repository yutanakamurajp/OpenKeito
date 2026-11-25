//! The transformations between phase quantities and the rotating dq0 frame,
//! and the phase-locked loop.

use std::f64::consts::PI;

use openkeito_model as model;

use super::dynamic::wrapped;
use super::{Block, Build, Context};

/// 120 degrees.
const SHIFT: f64 = 2.0 * PI / 3.0;

/// (d, q, zero) of the phase quantities `x` for the angle `theta` of the d
/// axis ahead of the axis of phase a; the q axis leads the d axis by 90
/// degrees and amplitudes are kept:
/// d + j q = 2/3 (a + b e^{j 2pi/3} + c e^{-j 2pi/3}) e^{-j theta}.
#[inline]
pub(super) fn to_dq0(x: [f64; 3], theta: f64) -> [f64; 3] {
    let [a, b, c] = x;
    let (s0, c0) = theta.sin_cos();
    let (s1, c1) = (theta - SHIFT).sin_cos();
    let (s2, c2) = (theta + SHIFT).sin_cos();
    [
        2.0 / 3.0 * (a * c0 + b * c1 + c * c2),
        -2.0 / 3.0 * (a * s0 + b * s1 + c * s2),
        (a + b + c) / 3.0,
    ]
}

/// The inverse of `to_dq0`.
#[inline]
pub(super) fn to_abc(dq0: [f64; 3], theta: f64) -> [f64; 3] {
    let [d, q, zero] = dq0;
    let (s0, c0) = theta.sin_cos();
    let (s1, c1) = (theta - SHIFT).sin_cos();
    let (s2, c2) = (theta + SHIFT).sin_cos();
    [
        d * c0 - q * s0 + zero,
        d * c1 - q * s1 + zero,
        d * c2 - q * s2 + zero,
    ]
}

pub(super) struct AbcToDq0 {
    inputs: [usize; 3],
    angle: usize,
}

impl AbcToDq0 {
    pub fn new(t: &model::AbcToDq0, b: &Build) -> Self {
        AbcToDq0 {
            inputs: b.three(&t.inputs),
            angle: b.signal(&t.angle),
        }
    }
}

impl AbcToDq0 {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let x = self.inputs.map(|k| signals[k]);
        out.copy_from_slice(&to_dq0(x, signals[self.angle]));
    }
}

impl Block for AbcToDq0 {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct Dq0ToAbc {
    d: usize,
    q: usize,
    zero: Option<usize>,
    angle: usize,
}

impl Dq0ToAbc {
    pub fn new(t: &model::Dq0ToAbc, b: &Build) -> Self {
        Dq0ToAbc {
            d: b.signal(&t.d),
            q: b.signal(&t.q),
            zero: t.zero.as_deref().map(|z| b.signal(z)),
            angle: b.signal(&t.angle),
        }
    }
}

impl Dq0ToAbc {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let zero = self.zero.map_or(0.0, |k| signals[k]);
        let dq0 = [signals[self.d], signals[self.q], zero];
        out.copy_from_slice(&to_abc(dq0, signals[self.angle]));
    }
}

impl Block for Dq0ToAbc {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

/// Phase-locked loop in the synchronous reference frame [Kaura, Blasko;
/// Chung]: with e = q(theta) / rated amplitude,
///
/// ```text
/// integral' = Ki e,   omega = omega0 + Kp e + integral,   theta' = omega.
/// ```
///
/// Both states are integrated with the trapezoidal rule. The present e
/// depends on the present theta, so the step is one equation for theta,
///
/// ```text
/// theta = theta_prev + h/2 (omega_prev + omega0 + integral_prev
///         + Ki h/2 e_prev + (Kp + Ki h/2) e(theta)),
/// ```
///
/// solved by Newton's method with de/dtheta = -d / rated amplitude. The
/// result is that of the loop built from the blocks abc_to_dq0, pi and
/// integrator.
pub(super) struct Pll {
    inputs: [usize; 3],
    omega0: f64,
    rated: f64,
    kp: f64,
    ki: f64,
    initial_angle: Option<f64>,
    /// Angle (folded into [0, 2 pi)), integral, error and frequency at the
    /// last grid point.
    theta: f64,
    integral: f64,
    e_prev: f64,
    omega_prev: f64,
    /// The same at the grid point being evaluated.
    next: [f64; 4],
}

/// Passes of Newton's method; it converges in two or three while
/// h/2 (Kp + Ki h/2) is small against 1.
const PLL_ITERATIONS: usize = 30;

impl Pll {
    pub fn new(p: &model::Pll, b: &Build) -> Self {
        Pll {
            inputs: b.three(&p.inputs),
            omega0: 2.0 * PI * p.nominal_frequency_hz,
            rated: p.rated_amplitude,
            kp: p.proportional_gain,
            ki: p.integral_gain_per_s,
            initial_angle: p.initial_angle_rad,
            theta: 0.0,
            integral: 0.0,
            e_prev: 0.0,
            omega_prev: 0.0,
            next: [0.0; 4],
        }
    }
}

impl Block for Pll {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let x = self.inputs.map(|k| signals[k]);
        let (theta, integral, dq0) = if ctx.start {
            // Locked: the angle of the space vector of the inputs, so that
            // q = 0 and d > 0.
            let theta = self.initial_angle.unwrap_or_else(|| {
                let [alpha, beta, _] = to_dq0(x, 0.0);
                beta.atan2(alpha)
            });
            (theta, 0.0, to_dq0(x, theta))
        } else {
            let half = ctx.h / 2.0;
            let gain = self.kp + self.ki * half;
            let known = self.theta
                + half
                    * (self.omega_prev
                        + self.omega0
                        + self.integral
                        + self.ki * half * self.e_prev);
            let mut theta = self.theta + ctx.h * self.omega_prev;
            let mut dq0 = to_dq0(x, theta);
            let mut settled = false;
            for _ in 0..PLL_ITERATIONS {
                let residual = known + half * gain * dq0[1] / self.rated - theta;
                let slope = -half * gain * dq0[0] / self.rated - 1.0;
                let step = residual / slope;
                theta -= step;
                dq0 = to_dq0(x, theta);
                // A few units of rounding of theta.
                if step.abs() <= 8.0 * f64::EPSILON * theta.abs().max(1.0) {
                    settled = true;
                    break;
                }
            }
            if !settled {
                // Not a number: the run stops and names the signal.
                theta = f64::NAN;
            }
            let e = dq0[1] / self.rated;
            (
                theta,
                self.integral + self.ki * half * (self.e_prev + e),
                dq0,
            )
        };
        let e = dq0[1] / self.rated;
        let omega = self.omega0 + self.kp * e + integral;
        let theta = wrapped(theta, 2.0 * PI);
        self.next = [theta, integral, e, omega];
        out.copy_from_slice(&[theta, omega / (2.0 * PI), dq0[0], dq0[1]]);
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, _signals: &[f64]) {
        [self.theta, self.integral, self.e_prev, self.omega_prev] = self.next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transformation_and_its_inverse_give_the_phases_back() {
        // Unbalanced values with a zero component, at angles around the
        // circle: the round trip differs by rounding only.
        let x = [311.0, -47.5, 12.25];
        for k in 0..64 {
            let theta = -7.0 + 0.3 * k as f64;
            let back = to_abc(to_dq0(x, theta), theta);
            for p in 0..3 {
                assert!((back[p] - x[p]).abs() < 4e-13, "theta {theta}: {back:?}");
            }
        }
    }

    #[test]
    fn balanced_phases_give_the_phasor_in_the_rotating_frame() {
        // Phasor X = 100 V rms at 30 degrees (cosine reference, sequence a,
        // b, c) and theta = w t + theta0: d + j q = sqrt(2) X e^{-j theta0},
        // constant in time.
        let (rms, angle, theta0) = (100.0_f64, 30.0_f64.to_radians(), 0.2);
        let peak = std::f64::consts::SQRT_2 * rms;
        let w = 2.0 * PI * 50.0;
        for k in 0..40 {
            let t = 1e-3 * k as f64;
            let phase = |shift: f64| peak * (w * t + angle + shift).cos();
            let x = [phase(0.0), phase(-SHIFT), phase(SHIFT)];
            let [d, q, zero] = to_dq0(x, w * t + theta0);
            assert!((d - peak * (angle - theta0).cos()).abs() < 1e-11, "t {t}");
            assert!((q - peak * (angle - theta0).sin()).abs() < 1e-11, "t {t}");
            assert!(zero.abs() < 1e-12);
        }
    }
}
