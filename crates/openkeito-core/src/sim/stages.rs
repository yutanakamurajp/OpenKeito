//! The stages of every kind of step of a run.

use openkeito_model::Method;

use crate::element::Stage;

/// The conditions of a run that its elements receive in `prepare`: the time
/// step, the method and the stages of every kind of step. A stage at
/// `time_fraction` f of a step from t0 to t1 is solved at t0 + f (t1 - t0);
/// t1 - t0 is `time_step_s` except at t = 0+.
#[derive(Debug, Clone)]
pub struct Stages {
    /// The fixed time step h in seconds.
    pub time_step_s: f64,
    pub method: Method,
    /// A regular step.
    pub regular: Vec<Stage>,
    /// The step that follows a switching action.
    pub after_event: Vec<Stage>,
    /// Two backward-Euler half steps. They follow every action under the
    /// trapezoidal rule, and a diode that stopped conducting at a current
    /// zero under 2S-DIRK.
    pub half_steps: Vec<Stage>,
    /// The solution at t = 0+: backward Euler over a negligible interval
    /// eps = 1e-6 h, with the sources at its end, which gives the voltages
    /// and currents for the first output row. States are not changed. A
    /// capacitor whose voltage a source dictates carries C dv/dt there,
    /// which needs the source at eps, not at 0.
    ///
    /// The trapezoidal rule needs the derivatives at t = 0+. A first
    /// negligible step, with the sources at t = 0, takes up any jump caused
    /// by initial conditions that contradict the sources and keeps the
    /// states (they then belong to t = 0, where the first step starts); the
    /// second one, with the sources at eps and not kept, yields consistent
    /// derivatives.
    pub initial: Vec<Stage>,
    /// Length eps of the interval of `initial`.
    pub initial_interval: f64,
    /// Solution that decides the diode states after a switching action: one
    /// backward-Euler step of length 1/alpha with the matrix of the regular
    /// stages, not kept.
    pub probe: Stage,
}

impl Stages {
    pub fn new(method: Method, h: f64) -> Stages {
        let gamma = 1.0 - std::f64::consts::FRAC_1_SQRT_2;
        let (regular, after_event): (Vec<Stage>, Vec<Stage>) = match method {
            Method::TwoStageDirk => {
                let alpha = 1.0 / (gamma * h);
                let stages = vec![
                    Stage {
                        alpha,
                        c: 0.0,
                        time_fraction: gamma,
                        commit: false,
                    },
                    Stage {
                        alpha,
                        c: (1.0 - gamma) / gamma,
                        time_fraction: 1.0,
                        commit: true,
                    },
                ];
                (stages.clone(), stages)
            }
            Method::Trapezoidal => {
                let alpha = 2.0 / h;
                let half = |time_fraction| Stage {
                    alpha,
                    c: 0.0,
                    time_fraction,
                    commit: true,
                };
                (
                    vec![Stage {
                        alpha,
                        c: 1.0,
                        time_fraction: 1.0,
                        commit: true,
                    }],
                    vec![half(0.5), half(1.0)],
                )
            }
        };
        let alpha = regular[0].alpha;
        let half_alpha = 2.0 / h;
        let initial_alpha = 1.0 / (1e-6 * h);
        let initial = Stage {
            alpha: initial_alpha,
            c: 0.0,
            time_fraction: 1.0,
            commit: false,
        };
        let initial_stages = match method {
            Method::TwoStageDirk => vec![initial],
            Method::Trapezoidal => vec![
                Stage {
                    commit: true,
                    time_fraction: 0.0,
                    ..initial
                },
                initial,
            ],
        };
        let half_steps: Vec<Stage> = [0.5, 1.0]
            .iter()
            .map(|&time_fraction| Stage {
                alpha: half_alpha,
                c: 0.0,
                time_fraction,
                commit: true,
            })
            .collect();
        Stages {
            time_step_s: h,
            method,
            regular,
            after_event,
            half_steps,
            initial: initial_stages,
            initial_interval: 1.0 / initial_alpha,
            probe: Stage {
                alpha,
                c: 0.0,
                time_fraction: 1.0 / (alpha * h),
                commit: false,
            },
        }
    }

    /// The stage coefficient of the regular stages.
    pub fn alpha(&self) -> f64 {
        self.regular[0].alpha
    }

    /// Every stage coefficient of the run: the regular stages, the
    /// backward-Euler half steps and the negligible step at t = 0+.
    pub fn alphas(&self) -> [f64; 3] {
        [
            self.alpha(),
            self.half_steps[0].alpha,
            self.initial[0].alpha,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_validation_knows_the_stage_coefficients_of_a_run() {
        // `Simulation::stage_coefficients` of openkeito-model, with which
        // the validation checks the series impedances (E139), gives the
        // values that the elements are prepared with, to the last bit.
        for method in [Method::TwoStageDirk, Method::Trapezoidal] {
            for h in [1e-6, 2e-5, 1e-4, 3.3e-3] {
                let text = format!(
                    r#"{{"time_step_s": {h:e}, "end_time_s": 1.0, "method": "{}", "switching": "interpolated"}}"#,
                    method.as_str()
                );
                let simulation: openkeito_model::Simulation = serde_json::from_str(&text).unwrap();
                let known = simulation.stage_coefficients();
                let stages = Stages::new(method, h);
                assert_eq!(known[..3], stages.alphas());
                // The shortest part of a split step, of every kind of step.
                let shortest = openkeito_model::SHORTEST_STEP_PART;
                let largest = [StepKind::Regular, StepKind::AfterEvent, StepKind::HalfSteps]
                    .map(|kind| stages.part(kind, shortest).alpha())
                    .into_iter()
                    .chain([stages.probe_of_part(shortest).alpha])
                    .fold(0.0, f64::max);
                assert_eq!(known[3], largest);
                assert_eq!(known.len(), 4);
            }
        }
    }
}

/// The kinds of step of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepKind {
    Regular,
    AfterEvent,
    HalfSteps,
}

/// The stages of a part of a time step (at most two).
#[derive(Debug, Clone, Copy)]
pub(super) struct Part {
    stages: [Stage; 2],
    len: usize,
}

impl Part {
    pub fn stages(&self) -> &[Stage] {
        &self.stages[..self.len]
    }

    /// The stage coefficient, the same in every stage.
    pub fn alpha(&self) -> f64 {
        self.stages[0].alpha
    }
}

impl Stages {
    pub(super) fn of(&self, kind: StepKind) -> &[Stage] {
        match kind {
            StepKind::Regular => &self.regular,
            StepKind::AfterEvent => &self.after_event,
            StepKind::HalfSteps => &self.half_steps,
        }
    }

    /// The stages of a part that covers `fraction` of the time step: the
    /// method applied to the shorter interval, so `alpha` is divided by
    /// `fraction`; `c`, `time_fraction` (now a fraction of the part) and
    /// `commit` stay.
    pub(super) fn part(&self, kind: StepKind, fraction: f64) -> Part {
        let whole = self.of(kind);
        let mut stages = [whole[0]; 2];
        for (s, w) in stages.iter_mut().zip(whole) {
            *s = Stage {
                alpha: w.alpha / fraction,
                ..*w
            };
        }
        Part {
            stages,
            len: whole.len(),
        }
    }

    /// The solution that decides the diode states at the start of a part
    /// that covers `fraction` of the time step (see `probe`).
    pub(super) fn probe_of_part(&self, fraction: f64) -> Stage {
        Stage {
            alpha: self.probe.alpha / fraction,
            ..self.probe
        }
    }
}
