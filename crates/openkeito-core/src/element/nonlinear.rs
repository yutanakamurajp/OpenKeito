//! Elements with a piecewise-linear characteristic: the nonlinear resistor
//! and the nonlinear inductor. In a segment such an element is a
//! conductance and a current source; its operating point is the segment,
//! which it finds within every stage (`update_operating_point`).

use openkeito_model::Diagnostic;

use super::{
    inject, instant, phasor, stamp_conductance, volt, Element, Node, OperatingPoint, Stage,
};
use crate::linalg::{Complex64, Stamp};

/// A piecewise-linear function y(x) through points with strictly increasing
/// x, continued beyond both ends with the outermost segments. Segment k
/// lies between the points k and k + 1; in it y = slope[k] x + intercept[k].
#[derive(Clone)]
pub(crate) struct Piecewise {
    /// Upper end of every segment but the last.
    bounds: Vec<f64>,
    slope: Vec<f64>,
    intercept: Vec<f64>,
    /// By how much x may lie outside a segment and still count as inside:
    /// rounding of a solution that lies on a breakpoint must not move the
    /// segment. 1e-12 of the largest |x| of the points.
    tight: f64,
    /// The same when the search contradicts itself, which rounding of a
    /// badly conditioned solution on a breakpoint does: 1e-6 of the largest
    /// |x|, the relative threshold of the diode states.
    loose: f64,
}

impl Piecewise {
    /// `points`: at least two [x, y] with strictly increasing x (validated).
    pub fn new(points: &[[f64; 2]]) -> Piecewise {
        let n = points.len() - 1;
        let mut slope = Vec::with_capacity(n);
        let mut intercept = Vec::with_capacity(n);
        for w in points.windows(2) {
            let ([x0, y0], [x1, y1]) = (w[0], w[1]);
            slope.push((y1 - y0) / (x1 - x0));
            // Exactly 0 for a segment between a point and its mirror image.
            intercept.push((y0 * x1 - y1 * x0) / (x1 - x0));
        }
        let scale = points.iter().fold(0.0_f64, |s, p| s.max(p[0].abs()));
        Piecewise {
            bounds: points[1..n].iter().map(|p| p[0]).collect(),
            slope,
            intercept,
            tight: 1e-12 * scale,
            loose: 1e-6 * scale,
        }
    }

    #[inline]
    fn last(&self) -> usize {
        self.slope.len() - 1
    }

    /// The segment of x; a breakpoint belongs to the segment above it.
    #[inline]
    pub fn segment_of(&self, x: f64) -> usize {
        self.bounds.partition_point(|b| *b <= x)
    }

    #[inline]
    fn lower(&self, k: usize) -> f64 {
        if k == 0 {
            f64::NEG_INFINITY
        } else {
            self.bounds[k - 1]
        }
    }

    #[inline]
    fn upper(&self, k: usize) -> f64 {
        self.bounds.get(k).copied().unwrap_or(f64::INFINITY)
    }

    /// `None` when x lies in segment k (within `slack`), otherwise whether
    /// it lies above it.
    #[inline]
    fn outside(&self, k: usize, x: f64, slack: f64) -> Option<bool> {
        if x > self.upper(k) + slack {
            Some(true)
        } else if x < self.lower(k) - slack {
            Some(false)
        } else {
            None
        }
    }

    /// The range of x over which the function is the straight line through
    /// the origin, and the segment of the origin.
    fn origin(&self) -> (usize, f64, f64) {
        let k = self.segment_of(0.0);
        // The origin is a point of a table that is not symmetric: the line
        // continues below it only when the next segment is the same line.
        let lower = if k > 0 && self.lower(k) == 0.0 && self.slope[k - 1] == self.slope[k] {
            self.lower(k - 1)
        } else {
            self.lower(k)
        };
        (k, lower, self.upper(k))
    }
}

/// The search for the segment of an element within one stage.
///
/// The circuit that the element sees is linear, and the characteristic does
/// not decrease. When the solution computed with the line of segment k lies
/// above segment k, the solution of the circuit lies above it too (the two
/// agree at the upper end of the segment, and the residual grows with x), so
/// every trial segment narrows the range [lo, hi] that still holds the
/// solution. The search first goes to the segment of the trial solution
/// (Newton's method, exact in one move when the solution is in a
/// neighbouring segment), and halves the range when two moves have not
/// settled it, which a steep characteristic such as an arrester's needs
/// (Newton's method comes down such a curve a few segments at a time): at
/// most 2 + log2(segments) moves.
///
/// With several such elements in one circuit the others move at the same
/// time and the range of one can be wrong. It then contradicts itself and is
/// opened again; the solver stops with E205 if that never ends.
pub(crate) struct Search {
    /// The present segment.
    pub segment: usize,
    lo: usize,
    hi: usize,
    /// Moves since the element last agreed with a solution.
    moves: u32,
}

impl Search {
    pub fn new(curve: &Piecewise, segment: usize) -> Search {
        Search {
            segment,
            lo: 0,
            hi: curve.last(),
            moves: 0,
        }
    }

    fn reset(&mut self, curve: &Piecewise) {
        self.lo = 0;
        self.hi = curve.last();
        self.moves = 0;
    }

    /// Compares the segment with the value `x` of a stage solution and
    /// moves it when they disagree.
    pub fn update(&mut self, curve: &Piecewise, x: f64) -> OperatingPoint {
        let k = self.segment;
        let Some(above) = curve.outside(k, x, curve.tight) else {
            self.reset(curve);
            return OperatingPoint::Consistent;
        };
        // `above` implies k < last, and below implies k > 0.
        if above {
            self.lo = self.lo.max(k + 1);
        } else {
            self.hi = self.hi.min(k - 1);
        }
        if self.lo > self.hi {
            // The solution was above a segment and is now below one that is
            // not lower (or the reverse). On a breakpoint that is rounding,
            // and both segments give the same current there.
            if curve.outside(k, x, curve.loose).is_none() {
                self.reset(curve);
                return OperatingPoint::Consistent;
            }
            // Otherwise the circuit around the element changed with the
            // segments of other elements: start again from here.
            self.reset(curve);
            if above {
                self.lo = k + 1;
            } else {
                self.hi = k - 1;
            }
        }
        let next = if self.moves >= 2 {
            self.lo + (self.hi - self.lo) / 2
        } else {
            curve.segment_of(x).clamp(self.lo, self.hi)
        };
        self.moves += 1;
        self.segment = next;
        if curve.slope[next] != curve.slope[k] {
            OperatingPoint::MatrixChanged
        } else {
            OperatingPoint::RhsChanged
        }
    }
}

/// i = g v + j in the present segment of the characteristic i(v).
pub(crate) struct NonlinearResistor {
    name: String,
    a: Node,
    b: Node,
    curve: Piecewise,
    search: Search,
    /// Segment of the origin: the element of a phasor solution.
    origin: usize,
    /// Peak of the steady-state voltage, when the run started from one.
    steady_peak: Option<f64>,
}

impl NonlinearResistor {
    /// `points`: [voltage, current] with strictly increasing voltage.
    pub fn new(name: String, a: Node, b: Node, points: &[[f64; 2]]) -> NonlinearResistor {
        let curve = Piecewise::new(points);
        let origin = curve.origin().0;
        NonlinearResistor {
            name,
            a,
            b,
            search: Search::new(&curve, origin),
            curve,
            origin,
            steady_peak: None,
        }
    }

    #[inline]
    fn voltage(&self, x: &[f64]) -> f64 {
        volt(x, self.a) - volt(x, self.b)
    }
}

impl Element for NonlinearResistor {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, self.curve.slope[self.search.segment]);
    }
    fn structure(&self, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, 1.0);
    }
    fn rhs(&self, _t: f64, _stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, self.b, self.curve.intercept[self.search.segment]);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        let k = self.search.segment;
        self.curve.slope[k] * self.voltage(x) + self.curve.intercept[k]
    }
    fn has_operating_point(&self) -> bool {
        true
    }
    fn update_operating_point(&mut self, _t: f64, _stage: &Stage, x: &[f64]) -> OperatingPoint {
        let v = self.voltage(x);
        self.search.update(&self.curve, v)
    }
    fn stamp_phasor(&self, _omega: f64, m: &mut dyn Stamp<Complex64>) {
        let g = Complex64::new(self.curve.slope[self.origin], 0.0);
        stamp_conductance(m, self.a, self.b, g);
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        self.curve.slope[self.origin] * (phasor(x, self.a) - phasor(x, self.b))
    }
    fn init_from_phasor(&mut self, _omega: f64, x: &[Complex64]) {
        let v = phasor(x, self.a) - phasor(x, self.b);
        self.steady_peak = Some(std::f64::consts::SQRT_2 * v.norm());
        self.search = Search::new(&self.curve, self.curve.segment_of(instant(v)));
    }
    fn steady_state_warning(&self) -> Option<Diagnostic> {
        let peak = self.steady_peak?;
        let (k, lower, upper) = self.curve.origin();
        let inside = peak <= upper + self.curve.tight && -peak >= lower - self.curve.tight;
        if inside {
            return None;
        }
        Some(Diagnostic::warning(
            "W260",
            self.name.clone(),
            format!(
                "the steady-state voltage across \"{}\" peaks at {peak:e} V, outside the segment \
                 through the origin of its characteristic ({lower:e} V to {upper:e} V)",
                self.name
            ),
            format!(
                "The steady state is computed with that segment alone (a conductance of {:e} S), \
                 so the run does not start in its steady state. Set \"initialization\": \"none\" \
                 and let the run reach its periodic state, or check the characteristic and the \
                 source voltages.",
                self.curve.slope[k]
            ),
        ))
    }
}

/// d(flux)/dt = v and i = Γ flux + j in the present segment of the
/// characteristic i(flux). State: the flux linkage. With the stage formula
/// v = alpha (flux - flux_start) - c f_prev, the flux of a stage is
/// flux_start + (v + c f_prev) / alpha, so
/// i = (Γ / alpha) v + Γ (flux_start + (c / alpha) f_prev) + j.
pub(crate) struct NonlinearInductor {
    name: String,
    a: Node,
    b: Node,
    curve: Piecewise,
    search: Search,
    /// Segment of the origin: the unsaturated inductance.
    origin: usize,
    /// Flux linkage at the start of the step.
    flux_start: f64,
    /// d(flux)/dt = v of the previous stage.
    f_prev: f64,
    /// (flux_start, f_prev) stored by `save`.
    saved: (f64, f64),
    /// Peak of the steady-state flux linkage, when the run started from
    /// one.
    steady_peak: Option<f64>,
    /// Scheduled (time_s, curve, flux_scale).
    changes: Vec<(f64, Piecewise, f64)>,
    next_change: usize,
}

impl NonlinearInductor {
    /// `points`: [flux linkage, current] with strictly increasing values of
    /// both; `flux` is the flux linkage at t = 0.
    pub fn new(name: String, a: Node, b: Node, points: &[[f64; 2]], flux: f64) -> Self {
        Self::with_changes(name, a, b, points, flux, Vec::new())
    }

    pub fn with_changes(
        name: String,
        a: Node,
        b: Node,
        points: &[[f64; 2]],
        flux: f64,
        changes: Vec<(f64, Vec<[f64; 2]>, f64)>,
    ) -> Self {
        let curve = Piecewise::new(points);
        let changes = changes
            .into_iter()
            .map(|(t, pts, scale)| (t, Piecewise::new(&pts), scale))
            .collect();
        NonlinearInductor {
            name,
            a,
            b,
            search: Search::new(&curve, curve.segment_of(flux)),
            origin: curve.origin().0,
            curve,
            flux_start: flux,
            f_prev: 0.0,
            saved: (0.0, 0.0),
            steady_peak: None,
            changes,
            next_change: 0,
        }
    }

    #[inline]
    fn voltage(&self, x: &[f64]) -> f64 {
        volt(x, self.a) - volt(x, self.b)
    }

    /// The part of the flux of a stage that does not depend on its voltage.
    #[inline]
    fn history(&self, stage: &Stage) -> f64 {
        self.flux_start + stage.c / stage.alpha * self.f_prev
    }

    #[inline]
    fn flux(&self, stage: &Stage, x: &[f64]) -> f64 {
        self.voltage(x) / stage.alpha + self.history(stage)
    }
}

impl Element for NonlinearInductor {
    fn name(&self) -> &str {
        &self.name
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        _actions: &mut Vec<(String, super::SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.next_change < self.changes.len() && self.changes[self.next_change].0 <= t + tol {
            let scale = self.changes[self.next_change].2;
            self.flux_start *= scale;
            self.f_prev *= scale;
            self.curve = self.changes[self.next_change].1.clone();
            self.origin = self.curve.origin().0;
            self.search = Search::new(&self.curve, self.curve.segment_of(self.flux_start));
            self.next_change += 1;
            changed = true;
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.changes.iter().map(|c| c.0).collect()
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        let g = self.curve.slope[self.search.segment] / alpha;
        stamp_conductance(m, self.a, self.b, g);
    }
    fn structure(&self, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, 1.0);
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        let k = self.search.segment;
        let i = self.curve.slope[k] * self.history(stage) + self.curve.intercept[k];
        inject(b, self.a, self.b, i);
    }
    fn current(&self, _k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        let k = self.search.segment;
        self.curve.slope[k] * self.flux(stage, x) + self.curve.intercept[k]
    }
    fn has_operating_point(&self) -> bool {
        true
    }
    fn update_operating_point(&mut self, _t: f64, stage: &Stage, x: &[f64]) -> OperatingPoint {
        let flux = self.flux(stage, x);
        self.search.update(&self.curve, flux)
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let flux = self.flux(stage, x);
        self.f_prev = self.voltage(x);
        if stage.commit {
            self.flux_start = flux;
        }
    }
    fn signal_names(&self) -> Vec<String> {
        vec!["flux_wb".to_string()]
    }
    fn signal(&self, _k: usize) -> f64 {
        self.flux_start
    }
    fn save(&mut self) {
        self.saved = (self.flux_start, self.f_prev);
    }
    fn restore(&mut self) {
        (self.flux_start, self.f_prev) = self.saved;
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        let y = self.curve.slope[self.origin] / Complex64::new(0.0, omega);
        stamp_conductance(m, self.a, self.b, y);
    }
    fn current_phasor(&self, _k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        self.curve.slope[self.origin] * (phasor(x, self.a) - phasor(x, self.b))
            / Complex64::new(0.0, omega)
    }
    fn init_from_phasor(&mut self, omega: f64, x: &[Complex64]) {
        let flux = (phasor(x, self.a) - phasor(x, self.b)) / Complex64::new(0.0, omega);
        self.flux_start = instant(flux);
        self.steady_peak = Some(std::f64::consts::SQRT_2 * flux.norm());
        self.search = Search::new(&self.curve, self.curve.segment_of(self.flux_start));
    }
    fn steady_state_warning(&self) -> Option<Diagnostic> {
        let peak = self.steady_peak?;
        let (k, _, upper) = self.curve.origin();
        if peak <= upper + self.curve.tight {
            return None;
        }
        Some(Diagnostic::warning(
            "W260",
            self.name.clone(),
            format!(
                "the steady-state flux linkage of \"{}\" peaks at {peak:e} Wb, beyond the first \
                 point of its saturation characteristic ({upper:e} Wb)",
                self.name
            ),
            format!(
                "The steady state is computed with the unsaturated inductance ({:e} H), so the \
                 run does not start in its steady state. Set \"initialization\": \"none\" and \
                 let the run reach its periodic state, or check the characteristic (the flux \
                 linkage at a sinusoidal voltage is sqrt(2) V_rms / (2 pi f)) and the source \
                 voltages.",
                1.0 / self.curve.slope[k]
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of moves until the search agrees, for a source `e` behind
    /// a resistance `r` across the characteristic i(v): v + r i(v) = e.
    /// Checks the solution against the residual.
    fn moves(curve: &Piecewise, start: usize, e: f64, r: f64) -> u32 {
        let mut search = Search::new(curve, start);
        for n in 0..=curve.slope.len() as u32 {
            let k = search.segment;
            let v = (e - r * curve.intercept[k]) / (1.0 + r * curve.slope[k]);
            if search.update(curve, v) == OperatingPoint::Consistent {
                let k = curve.segment_of(v);
                let i = curve.slope[k] * v + curve.intercept[k];
                let residual = v + r * i - e;
                assert!(
                    residual.abs() <= 1e-6 * e.abs().max(1.0),
                    "residual {residual} at v = {v}"
                );
                return n;
            }
        }
        panic!("the search did not end");
    }

    #[test]
    fn a_segment_between_mirror_images_passes_through_the_origin_exactly() {
        let c = Piecewise::new(&[[-0.5, -7.0], [-0.125, -0.875], [0.125, 0.875], [0.5, 7.0]]);
        assert_eq!(c.intercept[1], 0.0);
        assert_eq!(c.slope[1], 7.0);
        assert_eq!(c.origin(), (1, -0.125, 0.125));
        assert_eq!(c.segment_of(-0.2), 0);
        assert_eq!(c.segment_of(0.125), 2);
        assert_eq!(c.segment_of(1e9), 2);
        // A table with the origin as a point: the line through it ends there
        // unless both sides have the same slope.
        let c = Piecewise::new(&[[-1.0, -1.0], [0.0, 0.0], [1.0, 2.0]]);
        assert_eq!(c.origin(), (1, 0.0, f64::INFINITY));
        let c = Piecewise::new(&[[-1.0, -2.0], [0.0, 0.0], [1.0, 2.0], [2.0, 9.0]]);
        assert_eq!(c.origin(), (1, f64::NEG_INFINITY, 1.0));
    }

    #[test]
    fn the_search_does_not_go_back_and_forth_where_moving_to_the_solution_does() {
        // i = v below 1 V, 4 v - 3 up to 2 V, 0.1 v + 4.8 above. With 5 V
        // behind 1 ohm, segment 0 gives 2.5 V (segment 2) and segment 2
        // gives 0.18 V (segment 0): moving to the segment of the solution
        // never ends. The search knows after these two that the solution is
        // above segment 0 and below segment 2.
        let c = Piecewise::new(&[[0.0, 0.0], [1.0, 1.0], [2.0, 5.0], [3.0, 5.1]]);
        assert_eq!(moves(&c, 0, 5.0, 1.0), 2);
        assert_eq!(moves(&c, 2, 5.0, 1.0), 2);
        assert_eq!(moves(&c, 1, 5.0, 1.0), 0);
    }

    #[test]
    fn a_solution_on_a_breakpoint_is_accepted_in_either_segment() {
        let c = Piecewise::new(&[[0.0, 0.0], [1.0, 1.0], [2.0, 5.0]]);
        // 2 V behind 1 ohm: v = 1 V exactly, the breakpoint.
        assert_eq!(moves(&c, 0, 2.0, 1.0), 0);
        assert_eq!(moves(&c, 1, 2.0, 1.0), 0);
        // Rounding beyond the tight slack: the search goes up, comes back
        // below the same breakpoint and accepts instead of going down again.
        let mut s = Search::new(&c, 0);
        assert_eq!(s.update(&c, 1.0 + 1e-8), OperatingPoint::MatrixChanged);
        assert_eq!(s.segment, 1);
        assert_eq!(s.update(&c, 1.0 - 1e-8), OperatingPoint::Consistent);
        assert_eq!(s.segment, 1);
        // Far below it, the circuit has changed: the search starts again.
        let mut s = Search::new(&c, 0);
        assert_eq!(s.update(&c, 1.5), OperatingPoint::MatrixChanged);
        assert_eq!(s.update(&c, 0.5), OperatingPoint::MatrixChanged);
        assert_eq!(s.segment, 0);
        assert_eq!(s.update(&c, 0.5), OperatingPoint::Consistent);
    }

    #[test]
    fn the_number_of_moves_grows_with_the_logarithm_of_the_number_of_segments() {
        // A steep symmetric characteristic i = (v / 1 V)^30 in n segments on
        // each side, with equal ratios of the voltages from 0.5 V to 2 V,
        // behind sources and resistances of many sizes and from every kind
        // of start. Newton's method alone needs about n / 4 moves down from
        // an open-circuit voltage far above the knee.
        let mut state = 12345_u64;
        let mut random = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        for n in [4_usize, 30, 300, 3000] {
            let side: Vec<[f64; 2]> = (0..=n)
                .map(|k| {
                    let v = 0.5 * 4.0_f64.powf(k as f64 / n as f64);
                    [v, v.powi(30)]
                })
                .collect();
            let points: Vec<[f64; 2]> = side
                .iter()
                .rev()
                .map(|p| [-p[0], -p[1]])
                .chain(side.iter().copied())
                .collect();
            let c = Piecewise::new(&points);
            let limit = 3 + (c.slope.len() as f64).log2().ceil() as u32;
            let mut worst = 0;
            for _ in 0..2000 {
                let e = (random() - 0.5) * 10.0_f64.powf(1.0 + 3.0 * random());
                let r = 10.0_f64.powf(-6.0 + 9.0 * random());
                let start = (random() * c.slope.len() as f64) as usize;
                worst = worst.max(moves(&c, start, e, r));
            }
            assert!(worst <= limit, "{n} segments: {worst} moves, limit {limit}");
            if n == 3000 {
                assert!(worst >= 8, "{worst}");
            }
        }
    }

    #[test]
    fn the_nonlinear_inductor_offers_its_flux_as_a_signal() {
        let mut l = NonlinearInductor::new(
            "L1".to_string(),
            Some(0),
            None,
            &[[-2.0, -11.0], [-1.0, -1.0], [1.0, 1.0], [2.0, 11.0]],
            1.5,
        );
        assert_eq!(l.signal_names(), ["flux_wb"]);
        assert_eq!(l.signal(0), 1.5);
        assert_eq!(l.search.segment, 2);
        // One backward-Euler stage of 0.1 s with 2 V: the flux rises by 0.2.
        let stage = Stage {
            alpha: 10.0,
            c: 0.0,
            time_fraction: 1.0,
            commit: true,
        };
        let x = [2.0];
        assert_eq!(
            l.update_operating_point(0.1, &stage, &x),
            OperatingPoint::Consistent
        );
        // i = 10 flux - 9 in the upper segment.
        assert!((l.current(0, 0.1, &stage, &x) - 8.0).abs() < 1e-12);
        l.advance(&stage, &x);
        assert!((l.signal(0) - 1.7).abs() < 1e-15);
    }
}
