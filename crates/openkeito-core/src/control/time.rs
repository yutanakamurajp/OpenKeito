//! The blocks that are functions of time: step, ramp, sine wave and time
//! series.

use openkeito_model as model;

use super::{Block, Context};

/// Rounding allowed when a time is compared with a grid point, as a fraction
/// of the time step (as for the switching actions of the circuit).
const GRID_TOLERANCE: f64 = 1e-9;

pub(super) struct Step {
    time_s: f64,
    before: f64,
    after: f64,
}

impl Step {
    pub fn new(s: &model::Step) -> Self {
        Step {
            time_s: s.time_s,
            before: s.before,
            after: s.after,
        }
    }
}

impl Block for Step {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = if ctx.t >= self.time_s - GRID_TOLERANCE * ctx.h {
            self.after
        } else {
            self.before
        };
    }
    fn preview(&self, t: f64, _signals: &[f64], out: &mut [f64]) {
        out[0] = if t >= self.time_s {
            self.after
        } else {
            self.before
        };
    }
}

pub(super) struct Ramp {
    start_time_s: f64,
    slope_per_s: f64,
    initial: f64,
}

impl Ramp {
    pub fn new(r: &model::Ramp) -> Self {
        Ramp {
            start_time_s: r.start_time_s,
            slope_per_s: r.slope_per_s,
            initial: r.initial,
        }
    }
}

impl Ramp {
    #[inline]
    fn at(&self, t: f64) -> f64 {
        self.initial + self.slope_per_s * (t - self.start_time_s).max(0.0)
    }
}

impl Block for Ramp {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.at(ctx.t);
    }
    fn preview(&self, t: f64, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.at(t);
    }
}

pub(super) struct SineWave {
    amplitude: f64,
    /// 2 pi f.
    omega: f64,
    phase_rad: f64,
    offset: f64,
}

impl SineWave {
    pub fn new(s: &model::SineWave) -> Self {
        SineWave {
            amplitude: s.amplitude,
            omega: 2.0 * std::f64::consts::PI * s.frequency_hz,
            phase_rad: s.phase_deg.to_radians(),
            offset: s.offset,
        }
    }
}

impl SineWave {
    #[inline]
    fn at(&self, t: f64) -> f64 {
        self.offset + self.amplitude * (self.omega * t + self.phase_rad).sin()
    }
}

impl Block for SineWave {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.at(ctx.t);
    }
    fn preview(&self, t: f64, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.at(t);
    }
}

pub(super) struct TimeSeries {
    times: Vec<f64>,
    values: Vec<f64>,
    linear: bool,
    /// Index of the last time at or before the last evaluation: the search
    /// starts there, as time only increases.
    at: usize,
}

impl TimeSeries {
    pub fn new(s: &model::TimeSeries) -> Self {
        TimeSeries {
            times: s.times_s.clone(),
            values: s.values.clone(),
            linear: s.interpolation == model::SeriesInterpolation::Linear,
            at: 0,
        }
    }
}

impl TimeSeries {
    /// The value at time `t`, where `k` is the index of the last time at or
    /// before `edge` (`t`, or `t` and the rounding of a grid point).
    fn value(&self, t: f64, edge: f64, k: usize) -> f64 {
        let last = self.times.len() - 1;
        if edge < self.times[0] {
            self.values[0]
        } else if !self.linear || k == last {
            self.values[k]
        } else {
            let x = ((t - self.times[k]) / (self.times[k + 1] - self.times[k])).clamp(0.0, 1.0);
            self.values[k] + x * (self.values[k + 1] - self.values[k])
        }
    }
}

impl Block for TimeSeries {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        // A time within rounding of a grid point belongs to it.
        let edge = ctx.t + GRID_TOLERANCE * ctx.h;
        let last = self.times.len() - 1;
        while self.at < last && self.times[self.at + 1] <= edge {
            self.at += 1;
        }
        out[0] = self.value(ctx.t, edge, self.at);
    }
    fn preview(&self, t: f64, _signals: &[f64], out: &mut [f64]) {
        let k = self.times.partition_point(|&x| x <= t).saturating_sub(1);
        out[0] = self.value(t, t, k);
    }
}
