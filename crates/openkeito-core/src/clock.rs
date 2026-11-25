//! The clock behind the timing of a run (`timing` and `elapsed_s` of the
//! summary).
//!
//! `std::time::Instant::now()` panics on wasm32-unknown-unknown, which has
//! no clock of its own. There the host gives one with [`set_clock`] (in a
//! browser or in Node: `performance.now()`); without it every time is 0 and
//! so is the timing of the summary. Everywhere else the clock is `Instant`,
//! called directly: the choice is made when the crate is compiled, not when
//! it runs.
//!
//! The clock only measures: no result depends on it.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod imp {
    use std::time::Instant;

    pub type Moment = Instant;

    #[inline(always)]
    pub fn now() -> Moment {
        Instant::now()
    }

    #[inline(always)]
    pub fn seconds(from: Moment, to: Moment) -> f64 {
        to.duration_since(from).as_secs_f64()
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod imp {
    use std::cell::Cell;

    /// Seconds since a moment the host chooses.
    pub type Moment = f64;

    thread_local! {
        pub static CLOCK: Cell<Option<fn() -> f64>> = const { Cell::new(None) };
    }

    #[inline]
    pub fn now() -> Moment {
        CLOCK.with(|c| c.get()).map_or(0.0, |clock| clock())
    }

    #[inline]
    pub fn seconds(from: Moment, to: Moment) -> f64 {
        // A host clock may step back; a time spent is not negative.
        (to - from).max(0.0)
    }
}

/// Sets the clock of wasm32-unknown-unknown: `seconds` returns the seconds
/// since a moment of its choice and never goes back (`performance.now()`
/// divided by 1000). `None` takes the clock away: every time is 0 again.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn set_clock(seconds: Option<fn() -> f64>) {
    imp::CLOCK.with(|c| c.set(seconds));
}

/// A moment to measure from.
#[derive(Debug, Clone, Copy)]
pub struct Stopwatch(imp::Moment);

impl Stopwatch {
    /// Now.
    #[inline(always)]
    pub fn start() -> Stopwatch {
        Stopwatch(imp::now())
    }

    /// Seconds from this moment to now.
    #[inline(always)]
    pub fn elapsed_s(&self) -> f64 {
        imp::seconds(self.0, imp::now())
    }

    /// Seconds from `earlier` to this moment (0 when `earlier` is later).
    #[inline(always)]
    pub fn since(&self, earlier: &Stopwatch) -> f64 {
        imp::seconds(earlier.0, self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Stopwatch;

    #[test]
    fn times_are_not_negative_and_add_up() {
        let a = Stopwatch::start();
        let b = Stopwatch::start();
        let whole = a.elapsed_s();
        assert!(b.since(&a) >= 0.0);
        assert!(whole >= b.since(&a));
        // The other way round is 0, not negative.
        assert_eq!(a.since(&b), 0.0);
    }
}
