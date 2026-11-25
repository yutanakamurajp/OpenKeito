//! Rotating machines: the synchronous machine.

use openkeito_model::{self as model, PHASES};

use super::{phase_differences, Builder, Expansion, Quantity, Term};
use crate::element;

/// One element between the phases of the bus and gnd. Its currents flow
/// from the bus into the machine.
pub(super) fn synchronous_machine(b: &mut Builder, m: &model::SynchronousMachine) -> Expansion {
    let bus = b.bus(&m.bus);
    let signals = [&m.field_voltage_signal, &m.mechanical_torque_signal]
        .map(|s| s.as_ref().map(|name| b.signal(name)));
    let e = b.push(Box::new(element::SynchronousMachine::new(m, bus, signals)));
    let currents = PHASES
        .iter()
        .enumerate()
        .map(|(p, ph)| Quantity {
            suffix: format!(".{ph}"),
            terms: vec![Term::Current(e, p, 1.0)],
        })
        .collect();
    Expansion {
        currents,
        voltages: phase_differences(bus, [None; 3]),
    }
}
