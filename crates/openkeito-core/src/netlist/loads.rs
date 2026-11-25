//! Loads and injections: the constant-impedance load and the three-phase
//! current source.

use openkeito_model::{self as model, PHASES};

use super::{phase_differences, Builder, Expansion, Quantity, Term, PHASE_SHIFT_DEG};
use crate::element;

/// Each phase: R = V^2/P in parallel with L = V^2/(w Q) (Q > 0) or
/// C = -Q/(w V^2) (Q < 0) from the bus to the neutral, V the rated
/// line-to-line voltage and P, Q three-phase: per phase (V/sqrt 3)^2 / R
/// is P/3.
pub(super) fn three_phase_load(b: &mut Builder, l: &model::ThreePhaseLoad) -> Expansion {
    let bus = b.bus(&l.bus);
    let neutral = b.node(&l.neutral);
    let v2 = (l.rated_voltage_kv * 1e3).powi(2);
    let (p, q) = (l.active_power_mw * 1e6, l.reactive_power_mvar * 1e6);
    let w = 2.0 * std::f64::consts::PI * l.rated_frequency_hz;
    let mut currents = Vec::with_capacity(3);
    for (k, ph) in PHASES.iter().enumerate() {
        let mut terms = Vec::new();
        if p > 0.0 {
            let e = b.push(Box::new(element::Resistor {
                name: format!("{}:r.{ph}", l.name),
                a: bus[k],
                b: neutral,
                g: p / v2,
                changes: Vec::new(),
                next_change: 0,
            }));
            terms.push(Term::Current(e, 0, 1.0));
        }
        if q > 0.0 {
            let e = b.push(Box::new(element::Inductor {
                name: format!("{}:l.{ph}", l.name),
                a: bus[k],
                b: neutral,
                l: v2 / (w * q),
                i_start: 0.0,
                f_prev: 0.0,
                saved: (0.0, 0.0),
                changes: Vec::new(),
                next_change: 0,
            }));
            terms.push(Term::Current(e, 0, 1.0));
        } else if q < 0.0 {
            let e = b.push(Box::new(element::Capacitor {
                name: format!("{}:c.{ph}", l.name),
                a: bus[k],
                b: neutral,
                c: -q / (w * v2),
                v_start: 0.0,
                f_prev: 0.0,
                saved: (0.0, 0.0),
            }));
            terms.push(Term::Current(e, 0, 1.0));
        }
        currents.push(Quantity {
            suffix: format!(".{ph}"),
            terms,
        });
    }
    Expansion {
        currents,
        voltages: phase_differences(bus, [neutral; 3]),
    }
}

/// Three current sources from the neutral into the bus phases:
/// sqrt(2) I cos(wt + angle - 120 k) = sqrt(2) I sin(wt + angle + 90 - 120 k).
pub(super) fn three_phase_current_source(
    b: &mut Builder,
    s: &model::ThreePhaseCurrentSource,
) -> Expansion {
    let bus = b.bus(&s.bus);
    let neutral = b.node(&s.neutral);
    let mut currents = Vec::with_capacity(3);
    for (k, ph) in PHASES.iter().enumerate() {
        let shift = PHASE_SHIFT_DEG[k];
        let e = b.push(Box::new(element::CurrentSource {
            name: format!("{}.{ph}", s.name),
            a: neutral,
            b: bus[k],
            waveform: model::Waveform::Sine {
                amplitude: std::f64::consts::SQRT_2 * s.current_rms_a,
                frequency_hz: s.frequency_hz,
                phase_deg: s.angle_deg + 90.0 + shift,
                offset: 0.0,
            },
            stepped: false,
        }));
        currents.push(Quantity {
            suffix: format!(".{ph}"),
            terms: vec![Term::Current(e, 0, 1.0)],
        });
    }
    Expansion {
        currents,
        voltages: phase_differences(bus, [neutral; 3]),
    }
}
