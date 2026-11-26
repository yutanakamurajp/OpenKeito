//! Switching components: the time-controlled switch, the diode, the switch
//! driven by a control signal, the thyristor and the fault.

use openkeito_model as model;

use super::{Builder, Expansion, Quantity, Term};
use crate::element::{self, Breaker, Node};

pub(super) fn switch(b: &mut Builder, s: &model::Switch) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let e = b.push(Box::new(element::Switch {
        name: s.name.clone(),
        a,
        b: bb,
        k: 0,
        initially_closed: s.initially_closed,
        close_time: s.close_time_s,
        open_time: s.open_time_s,
        open_at_current_zero: s.open_at_current_zero,
        breaker: Breaker::new(s.initially_closed),
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn diode(b: &mut Builder, d: &model::Diode) -> Expansion {
    let (a, bb) = b.two_nodes(&d.nodes);
    let e = b.push_diode(Box::new(element::TwoState {
        name: d.name.clone(),
        a,
        b: bb,
        g_on: 1.0 / d.on_resistance_ohm.ohm(),
        g_off: 1.0 / d.off_resistance_ohm.ohm(),
        on: false,
        gate: None,
        latching: false,
        fired: false,
        i_last: 0.0,
        saved: 0.0,
        stress: element::DefaultStress::of(d.on_resistance_ohm, d.off_resistance_ohm),
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn controlled_switch(b: &mut Builder, s: &model::ControlledSwitch) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let gate = b.signal(&s.gate);
    let e = b.push_two_state(Box::new(element::TwoState {
        name: s.name.clone(),
        a,
        b: bb,
        g_on: 1.0 / s.on_resistance_ohm.ohm(),
        g_off: 1.0 / s.off_resistance_ohm.ohm(),
        on: false,
        gate: Some(gate),
        latching: false,
        fired: false,
        i_last: 0.0,
        saved: 0.0,
        stress: element::DefaultStress::of(s.on_resistance_ohm, s.off_resistance_ohm),
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn thyristor(b: &mut Builder, s: &model::Thyristor) -> Expansion {
    let (a, bb) = b.two_nodes(&s.nodes);
    let gate = b.signal(&s.gate);
    // A diode that reads its gate: its state follows from the solution.
    let e = b.push_diode(Box::new(element::TwoState {
        name: s.name.clone(),
        a,
        b: bb,
        g_on: 1.0 / s.on_resistance_ohm.ohm(),
        g_off: 1.0 / s.off_resistance_ohm.ohm(),
        on: false,
        gate: Some(gate),
        latching: true,
        fired: false,
        i_last: 0.0,
        saved: 0.0,
        stress: element::DefaultStress::of(s.on_resistance_ohm, s.off_resistance_ohm),
    }));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn fault(b: &mut Builder, f: &model::Fault) -> Expansion {
    let nodes = f.bus.nodes();
    let point = if f.ground && f.ground_resistance_ohm == 0.0 {
        None
    } else {
        b.inner(format!("V({}:f)", f.name))
    };
    let phases: Vec<(String, Node)> = f
        .phases
        .iter()
        .map(|ph| {
            (
                format!("{}.{}", f.name, ph.as_str()),
                b.node(&nodes[ph.index()]),
            )
        })
        .collect();
    let count = phases.len();
    let e = b.push(Box::new(element::Fault {
        name: f.name.clone(),
        phases,
        point,
        r_f: f.fault_resistance_ohm,
        g_ground: (f.ground && f.ground_resistance_ohm > 0.0)
            .then(|| 1.0 / f.ground_resistance_ohm),
        grounded: f.ground,
        k: 0,
        start: f.start_time_s,
        clear: f.clear_time_s,
        clear_at_current_zero: f.clear_at_current_zero,
        breakers: vec![Breaker::new(false); count],
    }));
    let mut currents: Vec<Quantity> = f
        .phases
        .iter()
        .enumerate()
        .map(|(i, ph)| Quantity {
            suffix: format!(".{}", ph.as_str()),
            terms: vec![Term::Current(e, i, 1.0)],
        })
        .collect();
    if f.ground {
        currents.push(Quantity {
            suffix: ".g".into(),
            terms: (0..count).map(|i| Term::Current(e, i, 1.0)).collect(),
        });
    }
    Expansion::currents(currents)
}
