//! Sources: single-phase voltage and current sources, the three-phase
//! voltage source behind an impedance, and the sources whose values are
//! control signals.

use openkeito_model::{self as model, PHASES};

use super::{phase_differences, Builder, Expansion, Quantity, Term, PHASE_SHIFT_DEG};
use crate::element::{self, CoupledRl, Followed};
use crate::linalg::Complex64;

pub(super) fn voltage_source(b: &mut Builder, s: &model::VoltageSource) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let e = b.push(Box::new(element::VoltageSource {
        name: s.name.clone(),
        a,
        b: bb,
        waveform: s.waveform.clone(),
        stepped: false,
        k: 0,
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn current_source(b: &mut Builder, s: &model::CurrentSource) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let e = b.push(Box::new(element::CurrentSource {
        name: s.name.clone(),
        a,
        b: bb,
        waveform: s.waveform.clone(),
        stepped: false,
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn three_phase_source(b: &mut Builder, s: &model::ThreePhaseSource) -> Expansion {
    let bus = b.bus(&s.bus);
    let neutral = b.node(&s.neutral);
    // The ideal sources sit between inner nodes and the star point when there
    // is an impedance, so that the impedance connects them to the bus.
    let terminals = if s.impedance.is_some() {
        let mut t = [None; 3];
        for (p, ph) in PHASES.iter().enumerate() {
            t[p] = b.inner(format!("V({}:{ph})", s.name));
        }
        t
    } else {
        bus
    };
    let amplitude = (2.0_f64 / 3.0).sqrt() * s.line_voltage_rms_v;
    let mut sources = [0; 3];
    for (p, ph) in PHASES.iter().enumerate() {
        // cos(x) = sin(x + 90 degrees).
        let shift = PHASE_SHIFT_DEG[p];
        sources[p] = b.push(Box::new(element::VoltageSource {
            name: format!("{}.{ph}", s.name),
            a: terminals[p],
            b: neutral,
            waveform: model::Waveform::Sine {
                amplitude,
                frequency_hz: s.frequency_hz,
                phase_deg: s.angle_deg + 90.0 + shift,
                offset: 0.0,
            },
            stepped: false,
            k: 0,
        }));
    }
    if let Some(z) = &s.impedance {
        let (r, l) = z.matrices();
        b.push(Box::new(CoupledRl::new(
            format!("{}:z", s.name),
            3,
            bus,
            terminals,
            r,
            l,
        )));
    }
    let currents = PHASES
        .iter()
        .enumerate()
        .map(|(p, ph)| Quantity {
            suffix: format!(".{ph}"),
            terms: vec![Term::Current(sources[p], 0, 1.0)],
        })
        .collect();
    Expansion {
        currents,
        voltages: phase_differences(bus, [neutral; 3]),
    }
}

/// Rms phasor with a cosine reference.
fn phasor(rms: f64, angle_deg: f64) -> Complex64 {
    Complex64::from_polar(rms, angle_deg.to_radians())
}

pub(super) fn controlled_voltage_source(
    b: &mut Builder,
    s: &model::ControlledVoltageSource,
) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let e = b.push(Box::new(element::ControlledVoltageSource {
        name: s.name.clone(),
        a,
        b: bb,
        value: Followed::new(b.signal(&s.signal)),
        phasor: s
            .steady_state
            .map_or(Complex64::ZERO, |p| phasor(p.voltage_rms_v, p.angle_deg)),
        k: 0,
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn controlled_current_source(
    b: &mut Builder,
    s: &model::ControlledCurrentSource,
) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let e = b.push(Box::new(element::ControlledCurrentSource {
        name: s.name.clone(),
        a,
        b: bb,
        value: Followed::new(b.signal(&s.signal)),
        phasor: s
            .steady_state
            .map_or(Complex64::ZERO, |p| phasor(p.current_rms_a, p.angle_deg)),
    }));
    Expansion::two_terminal(e, a, bb)
}

/// Three controlled voltage sources from the star point to the bus, or to
/// inner nodes that the impedance connects to the bus.
pub(super) fn three_phase_controlled_source(
    b: &mut Builder,
    s: &model::ThreePhaseControlledSource,
) -> Expansion {
    let bus = b.bus(&s.bus);
    let neutral = b.node(&s.neutral);
    let terminals = if s.impedance.is_some() {
        let mut t = [None; 3];
        for (p, ph) in PHASES.iter().enumerate() {
            t[p] = b.inner(format!("V({}:{ph})", s.name));
        }
        t
    } else {
        bus
    };
    let mut sources = [0; 3];
    for (p, ph) in PHASES.iter().enumerate() {
        sources[p] = b.push(Box::new(element::ControlledVoltageSource {
            name: format!("{}.{ph}", s.name),
            a: terminals[p],
            b: neutral,
            value: Followed::new(b.signal(&s.signals[p])),
            phasor: s.steady_state.map_or(Complex64::ZERO, |v| {
                phasor(
                    v.line_voltage_rms_v / 3.0_f64.sqrt(),
                    v.angle_deg + PHASE_SHIFT_DEG[p],
                )
            }),
            k: 0,
        }));
    }
    if let Some(z) = &s.impedance {
        let (r, l) = z.matrices();
        b.push(Box::new(CoupledRl::new(
            format!("{}:z", s.name),
            3,
            bus,
            terminals,
            r,
            l,
        )));
    }
    let currents = PHASES
        .iter()
        .enumerate()
        .map(|(p, ph)| Quantity {
            suffix: format!(".{ph}"),
            terms: vec![Term::Current(sources[p], 0, 1.0)],
        })
        .collect();
    Expansion {
        currents,
        voltages: phase_differences(bus, [neutral; 3]),
    }
}

/// Three controlled current sources from the neutral into the bus phases.
pub(super) fn three_phase_controlled_current_source(
    b: &mut Builder,
    s: &model::ThreePhaseControlledCurrentSource,
) -> Expansion {
    let bus = b.bus(&s.bus);
    let neutral = b.node(&s.neutral);
    let mut currents = Vec::with_capacity(3);
    for (p, ph) in PHASES.iter().enumerate() {
        let e = b.push(Box::new(element::ControlledCurrentSource {
            name: format!("{}.{ph}", s.name),
            a: neutral,
            b: bus[p],
            value: Followed::new(b.signal(&s.signals[p])),
            phasor: s.steady_state.map_or(Complex64::ZERO, |i| {
                phasor(i.current_rms_a, i.angle_deg + PHASE_SHIFT_DEG[p])
            }),
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
