//! Converters: the source of the averaged grid-following converter and its
//! reactor. Its controls are blocks (`model::expanded`).

use openkeito_model::{self as model, PHASES};

use super::{phase_differences, Builder, Expansion, Quantity, Term, PHASE_SHIFT_DEG};
use crate::element::{ControlledVoltageSource, ConverterSource, CoupledRl, Followed};
use crate::linalg::Complex64;

/// Three sources from the star point to inner nodes, which the reactor
/// connects to the bus. The currents I(<name>.a) etc. flow into the bus.
pub(super) fn grid_following_converter(
    b: &mut Builder,
    c: &model::GridFollowingConverter,
) -> Expansion {
    let bus = b.bus(&c.bus);
    let neutral = b.node(&c.neutral);
    let mut inner = [None; 3];
    for (p, ph) in PHASES.iter().enumerate() {
        inner[p] = b.inner(format!("V({}:{ph})", c.name));
    }
    let (rms, angle_deg) = c.steady_current().unwrap_or((0.0, 0.0));
    let signals = c.source_signals();
    let mut currents = Vec::with_capacity(3);
    for (p, ph) in PHASES.iter().enumerate() {
        let e = b.push(Box::new(ConverterSource {
            source: ControlledVoltageSource {
                name: format!("{}.{ph}", c.name),
                a: inner[p],
                b: neutral,
                value: Followed::new(b.signal(&signals[p])),
                phasor: Complex64::ZERO,
                k: 0,
            },
            injection: Complex64::from_polar(rms, (angle_deg + PHASE_SHIFT_DEG[p]).to_radians()),
        }));
        // The branch current of the source flows from the inner node to the
        // star point: out of the bus.
        currents.push(Quantity {
            suffix: format!(".{ph}"),
            terms: vec![Term::Current(e, 0, -1.0)],
        });
    }
    let (r, l) = c.reactor_matrices();
    b.push(Box::new(CoupledRl::new(
        format!("{}:z", c.name),
        3,
        bus,
        inner,
        r,
        l,
    )));
    Expansion {
        currents,
        voltages: phase_differences(bus, [neutral; 3]),
    }
}
