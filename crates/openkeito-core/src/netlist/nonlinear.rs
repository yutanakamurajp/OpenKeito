//! Nonlinear components: the piecewise-linear resistor and inductor.

use openkeito_model as model;

use super::{Builder, Expansion};
use crate::element;

pub(super) fn nonlinear_resistor(b: &mut Builder, r: &model::NonlinearResistor) -> Expansion {
    let (a, bb) = b.two_nodes(&r.nodes);
    let e = b.push(Box::new(element::NonlinearResistor::new(
        r.name.clone(),
        a,
        bb,
        &r.characteristic.points(),
    )));
    Expansion::two_terminal(e, a, bb)
}

pub(super) fn nonlinear_inductor(b: &mut Builder, l: &model::NonlinearInductor) -> Expansion {
    let (a, bb) = b.two_nodes(&l.nodes);
    let e = b.push(Box::new(element::NonlinearInductor::new(
        l.name.clone(),
        a,
        bb,
        &l.points(),
        l.initial_flux_linkage_wb,
    )));
    Expansion::two_terminal(e, a, bb)
}
