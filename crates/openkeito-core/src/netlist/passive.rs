//! Passive components: resistor, inductor, capacitor and coupled RL
//! branches.

use openkeito_model as model;

use super::{phase_differences, phase_quantities, Builder, Expansion};
use crate::element;

pub(super) fn resistor(b: &mut Builder, r: &model::Resistor) -> Expansion {
    let (a, bb) = b.two_nodes(&r.nodes);
    let e = b.push(Box::new(element::Resistor {
        name: r.name.clone(),
        a,
        b: bb,
        g: 1.0 / r.resistance_ohm,
        changes: Vec::new(),
        next_change: 0,
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn inductor(b: &mut Builder, l: &model::Inductor) -> Expansion {
    let (a, bb) = b.two_nodes(&l.nodes);
    let e = b.push(Box::new(element::Inductor {
        name: l.name.clone(),
        a,
        b: bb,
        l: l.inductance_h,
        i_start: l.initial_current_a,
        f_prev: 0.0,
        saved: (0.0, 0.0),
        changes: Vec::new(),
        next_change: 0,
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn capacitor(b: &mut Builder, k: &model::Capacitor) -> Expansion {
    let (a, bb) = b.two_nodes(&k.nodes);
    let e = b.push(Box::new(element::Capacitor {
        name: k.name.clone(),
        a,
        b: bb,
        c: k.capacitance_f,
        v_start: k.initial_voltage_v,
        f_prev: 0.0,
        saved: (0.0, 0.0),
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn coupled_rl(b: &mut Builder, c: &model::CoupledRl) -> Expansion {
    let (from, to) = (b.bus(&c.buses[0]), b.bus(&c.buses[1]));
    let (r, l) = c.impedance.matrices();
    let changes = c
        .impedance_changes
        .iter()
        .map(|ch| {
            let (cr, cl) = ch.impedance.matrices();
            (ch.time_s, cr, cl)
        })
        .collect();
    let e = b.push(Box::new(element::CoupledRl::with_changes(
        c.name.clone(),
        3,
        from,
        to,
        r,
        l,
        changes,
        Some(c.name.clone()),
    )));
    Expansion {
        currents: phase_quantities(e, "", 1.0).collect(),
        voltages: phase_differences(from, to),
    }
}
