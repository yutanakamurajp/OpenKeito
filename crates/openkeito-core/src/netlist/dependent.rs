//! Dependent sources: one element each, between the nodes of its two ports.

use openkeito_model as model;

use super::{Builder, Expansion, Quantity, Term};
use crate::element;

/// I(<name>) from current 0 of element `e`, and I(<name>.sense) from its
/// current 1 for a source with a sense port.
fn currents(e: usize, sense: bool) -> Expansion {
    let mut currents = vec![Quantity {
        suffix: String::new(),
        terms: vec![Term::Current(e, 0, 1.0)],
    }];
    if sense {
        currents.push(Quantity {
            suffix: ".sense".into(),
            terms: vec![Term::Current(e, 1, 1.0)],
        });
    }
    Expansion::currents(currents)
}

pub(super) fn vcvs(b: &mut Builder, s: &model::Vcvs) -> Expansion {
    let (cp, cq) = b.two_nodes(&s.control);
    let (p, q) = b.two_nodes(&s.output);
    let e = b.push(Box::new(element::Vcvs {
        name: s.name.clone(),
        p,
        q,
        cp,
        cq,
        gain: s.gain_v_per_v,
        k: 0,
    }));
    currents(e, false)
}

pub(super) fn vccs(b: &mut Builder, s: &model::Vccs) -> Expansion {
    let (cp, cq) = b.two_nodes(&s.control);
    let (p, q) = b.two_nodes(&s.output);
    let e = b.push(Box::new(element::Vccs {
        name: s.name.clone(),
        p,
        q,
        cp,
        cq,
        gain: s.transconductance_s,
    }));
    currents(e, false)
}

pub(super) fn cccs(b: &mut Builder, s: &model::Cccs) -> Expansion {
    let (sp, sq) = b.two_nodes(&s.sense);
    let (p, q) = b.two_nodes(&s.output);
    let e = b.push(Box::new(element::Cccs {
        name: s.name.clone(),
        p,
        q,
        sp,
        sq,
        gain: s.gain_a_per_a,
        k: 0,
    }));
    currents(e, true)
}

pub(super) fn ccvs(b: &mut Builder, s: &model::Ccvs) -> Expansion {
    let (sp, sq) = b.two_nodes(&s.sense);
    let (p, q) = b.two_nodes(&s.output);
    let e = b.push(Box::new(element::Ccvs {
        name: s.name.clone(),
        p,
        q,
        sp,
        sq,
        gain: s.transresistance_ohm,
        k: 0,
    }));
    currents(e, true)
}
