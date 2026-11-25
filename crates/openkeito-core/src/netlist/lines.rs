//! Lines: the pi line as cascaded sections and the travelling-wave line.

use openkeito_model::{self as model, PHASES};

use super::{phase_differences, phase_quantities, scaled, Builder, Expansion, Quantity, Term};
use crate::element::{BergeronLine, CapacitanceMatrix, CoupledRl};

pub(super) fn pi_line(b: &mut Builder, l: &model::PiLine) -> Expansion {
    let n = l.sections as usize;
    let (r, ind, c) = l.per_km.matrices();
    let seg = l.length_km / n as f64;
    let from = b.bus(&l.buses[0]);
    let to = b.bus(&l.buses[1]);
    let mut sets = vec![from];
    for k in 1..n {
        let mut s = [None; 3];
        for (p, ph) in PHASES.iter().enumerate() {
            s[p] = b.inner(format!("V({}:{k}.{ph})", l.name));
        }
        sets.push(s);
    }
    sets.push(to);
    let mut series = Vec::with_capacity(n);
    for k in 0..n {
        series.push(b.push(Box::new(CoupledRl::new(
            format!("{}:{}", l.name, k + 1),
            3,
            sets[k],
            sets[k + 1],
            scaled(&r, seg),
            scaled(&ind, seg),
        ))));
    }
    let mut shunt = Vec::with_capacity(n + 1);
    for (k, set) in sets.iter().enumerate() {
        let weight = if k == 0 || k == n { 0.5 } else { 1.0 };
        shunt.push(b.push(Box::new(CapacitanceMatrix::new(
            format!("{}:c{k}", l.name),
            3,
            *set,
            [None; 3],
            scaled(&c, seg * weight),
        ))));
    }
    let mut currents = Vec::with_capacity(6);
    for (p, ph) in PHASES.iter().enumerate() {
        currents.push(Quantity {
            suffix: format!(".0.{ph}"),
            terms: vec![
                Term::Current(series[0], p, 1.0),
                Term::Current(shunt[0], p, 1.0),
            ],
        });
    }
    for (p, ph) in PHASES.iter().enumerate() {
        currents.push(Quantity {
            suffix: format!(".1.{ph}"),
            terms: vec![
                Term::Current(series[n - 1], p, -1.0),
                Term::Current(shunt[n], p, 1.0),
            ],
        });
    }
    Expansion {
        currents,
        voltages: phase_differences(from, to),
    }
}

/// One element: the currents 0 to 2 flow into the line at buses[0], 3 to 5
/// at buses[1].
pub(super) fn bergeron_line(b: &mut Builder, l: &model::BergeronLine) -> Expansion {
    let from = b.bus(&l.buses[0]);
    let to = b.bus(&l.buses[1]);
    let modes = l.modes().expect("validated line parameters");
    let e = b.push(Box::new(BergeronLine::new(
        l.name.clone(),
        from,
        to,
        &modes,
    )));
    let mut currents: Vec<Quantity> = phase_quantities(e, ".0", 1.0).collect();
    for (p, ph) in PHASES.iter().enumerate() {
        currents.push(Quantity {
            suffix: format!(".1.{ph}"),
            terms: vec![Term::Current(e, 3 + p, 1.0)],
        });
    }
    Expansion {
        currents,
        voltages: phase_differences(from, to),
    }
}
