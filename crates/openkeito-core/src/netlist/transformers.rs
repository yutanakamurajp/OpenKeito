//! Transformers: the single-phase unit (T equivalent), the three-phase bank
//! with its vector groups and the ideal three-phase transformer.

use openkeito_model::{self as model, WindingConnection, GROUND, PHASES};

use super::{one, Builder, Expansion, Quantity, Term};
use crate::element::{self, CoupledRl, Node, NonlinearInductor};

/// Scheduled tap state of one single-phase unit.
struct UnitTap {
    time_s: f64,
    w_h: f64,
    w_l: f64,
    z: f64,
    r: f64,
}

/// Ratings of one single-phase unit of a transformer.
struct Unit<'a> {
    /// Rated voltages of the windings in volt.
    w_h: f64,
    w_l: f64,
    /// Rating in VA.
    s: f64,
    omega: f64,
    /// Short-circuit impedance and its resistive part, per unit.
    z: f64,
    r: f64,
    /// Magnetizing admittance magnitude, per unit.
    y_m: f64,
    /// Core loss in watt.
    loss: f64,
    /// Saturation of the magnetizing branch, with the name of the element
    /// that takes the place of the linear inductor.
    saturation: Option<(&'a model::Saturation, String)>,
    /// Scheduled tap changes.
    tap_changes: Vec<UnitTap>,
    /// Component name to emit on tap change (once per transformer).
    event_name: Option<String>,
}

/// Winding currents of a unit: the leakage branches of the two windings,
/// each with its current flowing into the dotted terminal.
struct UnitBranches {
    hv: usize,
    lv: usize,
}

/// One single-phase unit (T equivalent): leakage R/2 + jX/2 from the dotted
/// terminal hp of winding 0 to the inner node m, the magnetizing branch from
/// m to hn, the ideal transformer (m, hn) : (x, ln), and leakage R/2 + jX/2
/// from the dotted terminal lp of winding 1 to x.
fn unit(
    b: &mut Builder,
    name: &str,
    label: &str,
    [hp, hn, lp, ln]: [Node; 4],
    u: &Unit,
) -> UnitBranches {
    let x = (u.z * u.z - u.r * u.r).sqrt();
    let base_h = u.w_h * u.w_h / u.s;
    let base_l = u.w_l * u.w_l / u.s;
    let m = b.inner(format!("V({label}m)"));
    let xn = b.inner(format!("V({label}x)"));
    let mut hv_changes = Vec::with_capacity(u.tap_changes.len());
    let mut lv_changes = Vec::with_capacity(u.tap_changes.len());
    let mut core_changes = Vec::with_capacity(u.tap_changes.len());
    let mut mag_lin_changes = Vec::with_capacity(u.tap_changes.len());
    let mut mag_sat_changes = Vec::with_capacity(u.tap_changes.len());
    let mut ideal_changes = Vec::with_capacity(u.tap_changes.len());
    let g = u.loss / u.s;
    let susceptance = (u.y_m * u.y_m - g * g).max(0.0).sqrt();
    let mut prev_w_h = u.w_h;
    for tap in &u.tap_changes {
        let tx = (tap.z * tap.z - tap.r * tap.r).sqrt();
        let tbh = tap.w_h * tap.w_h / u.s;
        let tbl = tap.w_l * tap.w_l / u.s;
        hv_changes.push((
            tap.time_s,
            one(tap.r / 2.0 * tbh),
            one(tx / 2.0 * tbh / u.omega),
        ));
        lv_changes.push((
            tap.time_s,
            one(tap.r / 2.0 * tbl),
            one(tx / 2.0 * tbl / u.omega),
        ));
        if g > 0.0 {
            core_changes.push((tap.time_s, g / tbh));
        }
        let scale = tap.w_h / prev_w_h;
        prev_w_h = tap.w_h;
        if let Some((saturation, _)) = &u.saturation {
            mag_sat_changes.push((tap.time_s, saturation.points(tap.w_h, u.s, u.omega), scale));
        } else if susceptance > 0.0 {
            // Preserve core flux per turn: lambda = L_m * i_m scales by w_h,
            // while L_m scales by w_h^2, so i_m scales by 1 / scale.
            mag_lin_changes.push((tap.time_s, tbh / (susceptance * u.omega), 1.0 / scale));
        }
        ideal_changes.push((tap.time_s, tap.w_h / tap.w_l));
    }
    let hv = b.push(Box::new(CoupledRl::with_changes(
        format!("{name}.0"),
        1,
        [hp, None, None],
        [m, None, None],
        one(u.r / 2.0 * base_h),
        one(x / 2.0 * base_h / u.omega),
        hv_changes,
        None,
    )));
    // Magnetizing admittance y_m with the conductance of the core loss.
    if g > 0.0 {
        b.push(Box::new(element::Resistor {
            name: format!("{name}.core"),
            a: m,
            b: hn,
            g: g / base_h,
            changes: core_changes,
            next_change: 0,
        }));
    }
    if let Some((saturation, mag)) = &u.saturation {
        // The magnetizing branch is on winding 0: its ratings are the bases.
        b.push(Box::new(NonlinearInductor::with_changes(
            mag.clone(),
            m,
            hn,
            &saturation.points(u.w_h, u.s, u.omega),
            0.0,
            mag_sat_changes,
        )));
    } else if susceptance > 0.0 {
        b.push(Box::new(element::Inductor {
            name: format!("{name}.mag"),
            a: m,
            b: hn,
            l: base_h / (susceptance * u.omega),
            i_start: 0.0,
            f_prev: 0.0,
            saved: (0.0, 0.0),
            changes: mag_lin_changes,
            next_change: 0,
        }));
    }
    b.push(Box::new(element::IdealTransformer {
        name: name.to_string(),
        p1: m,
        q1: hn,
        p2: xn,
        q2: ln,
        ratio: u.w_h / u.w_l,
        k: 0,
        changes: ideal_changes,
        next_change: 0,
        event_name: u.event_name.clone(),
    }));
    let lv = b.push(Box::new(CoupledRl::with_changes(
        format!("{name}.1"),
        1,
        [lp, None, None],
        [xn, None, None],
        one(u.r / 2.0 * base_l),
        one(x / 2.0 * base_l / u.omega),
        lv_changes,
        None,
    )));
    UnitBranches { hv, lv }
}

/// End of a winding of the unit of phase p: the phase p + offset (offset 1
/// is the next phase, 2 the previous one) or the star point.
#[derive(Clone, Copy, PartialEq)]
enum End {
    Phase(usize),
    Star,
}

/// The windings of the unit of phase p as (dotted end, other end), for the
/// high- and low-voltage side. With positive-sequence voltages, a delta
/// winding from p to the next phase carries sqrt(3) times the phase voltage
/// advanced by 30 degrees, and one from p to the previous phase the same
/// delayed by 30 degrees; reversing a winding adds 180 degrees. The table
/// picks the ends that make the low-voltage side lag by clock * 30 degrees.
fn winding_ends(g: &model::VectorGroup) -> ((End, End), (End, End)) {
    use End::{Phase, Star};
    let star = |w| matches!(w, WindingConnection::Star { .. });
    match (star(g.hv), star(g.lv), g.clock) {
        (true, true, 0) => ((Phase(0), Star), (Phase(0), Star)),
        (true, true, 6) => ((Phase(0), Star), (Star, Phase(0))),
        (true, false, 1) => ((Phase(0), Star), (Phase(0), Phase(1))),
        (true, false, 11) => ((Phase(0), Star), (Phase(0), Phase(2))),
        (true, false, 5) => ((Phase(0), Star), (Phase(2), Phase(0))),
        (true, false, 7) => ((Phase(0), Star), (Phase(1), Phase(0))),
        (false, true, 11) => ((Phase(0), Phase(1)), (Phase(0), Star)),
        (false, true, 5) => ((Phase(0), Phase(1)), (Star, Phase(0))),
        (false, true, 1) => ((Phase(0), Phase(2)), (Phase(0), Star)),
        (false, true, 7) => ((Phase(0), Phase(2)), (Star, Phase(0))),
        (false, false, 0) => ((Phase(0), Phase(1)), (Phase(0), Phase(1))),
        (false, false, 6) => ((Phase(0), Phase(1)), (Phase(1), Phase(0))),
        _ => unreachable!("validated vector group"),
    }
}

pub(super) fn transformer(b: &mut Builder, t: &model::Transformer) -> Expansion {
    let node = |n: &String| b.node(n);
    let ends = [
        node(&t.windings[0][0]),
        node(&t.windings[0][1]),
        node(&t.windings[1][0]),
        node(&t.windings[1][1]),
    ];
    let u = Unit {
        w_h: t.rated_voltage_kv[0] * 1e3,
        w_l: t.rated_voltage_kv[1] * 1e3,
        s: t.rated_power_mva * 1e6,
        omega: 2.0 * std::f64::consts::PI * t.rated_frequency_hz,
        z: t.impedance_percent / 100.0,
        r: t.resistance_percent / 100.0,
        y_m: t.no_load_current_percent / 100.0,
        loss: t.no_load_loss_kw * 1e3,
        saturation: t
            .saturation
            .as_ref()
            .map(|s| (s, format!("{}.mag", t.name))),
        tap_changes: t
            .tap_changes
            .iter()
            .map(|ch| UnitTap {
                time_s: ch.time_s,
                w_h: ch.rated_voltage_kv[0] * 1e3,
                w_l: ch.rated_voltage_kv[1] * 1e3,
                z: ch.impedance_percent.unwrap_or(t.impedance_percent) / 100.0,
                r: ch.resistance_percent.unwrap_or(t.resistance_percent) / 100.0,
            })
            .collect(),
        event_name: Some(t.name.clone()),
    };
    let w = unit(b, &t.name, &format!("{}:", t.name), ends, &u);
    Expansion::currents(vec![
        Quantity {
            suffix: ".0".into(),
            terms: vec![Term::Current(w.hv, 0, 1.0)],
        },
        Quantity {
            suffix: ".1".into(),
            terms: vec![Term::Current(w.lv, 0, 1.0)],
        },
    ])
}

pub(super) fn three_phase_transformer(
    b: &mut Builder,
    t: &model::ThreePhaseTransformer,
) -> Expansion {
    let g = model::VectorGroup::parse(&t.vector_group).expect("validated vector group");
    let sides = [b.bus(&t.buses[0]), b.bus(&t.buses[1])];
    let given = t.neutral_nodes();
    let mut stars = [None; 2];
    let mut winding_voltage = [0.0; 2];
    for (k, w) in [g.hv, g.lv].into_iter().enumerate() {
        let line = t.rated_voltage_kv[k] * 1e3;
        (stars[k], winding_voltage[k]) = match w {
            WindingConnection::Star { neutral: true } => (
                b.node(given[k].as_deref().unwrap_or(GROUND)),
                line / 3.0_f64.sqrt(),
            ),
            WindingConnection::Star { neutral: false } => (
                b.inner(format!("V({}:{}n)", t.name, ["hv.", "lv."][k])),
                line / 3.0_f64.sqrt(),
            ),
            WindingConnection::Delta => (None, line),
        };
    }
    let tap_scale = [
        if matches!(g.hv, WindingConnection::Star { .. }) {
            1e3 / 3.0_f64.sqrt()
        } else {
            1e3
        },
        if matches!(g.lv, WindingConnection::Star { .. }) {
            1e3 / 3.0_f64.sqrt()
        } else {
            1e3
        },
    ];
    let tap_changes: Vec<UnitTap> = t
        .tap_changes
        .iter()
        .map(|ch| UnitTap {
            time_s: ch.time_s,
            w_h: ch.rated_voltage_kv[0] * tap_scale[0],
            w_l: ch.rated_voltage_kv[1] * tap_scale[1],
            z: ch.impedance_percent.unwrap_or(t.impedance_percent) / 100.0,
            r: ch.resistance_percent.unwrap_or(t.resistance_percent) / 100.0,
        })
        .collect();
    let mut u = Unit {
        w_h: winding_voltage[0],
        w_l: winding_voltage[1],
        s: t.rated_power_mva * 1e6 / 3.0,
        omega: 2.0 * std::f64::consts::PI * t.rated_frequency_hz,
        z: t.impedance_percent / 100.0,
        r: t.resistance_percent / 100.0,
        y_m: t.no_load_current_percent / 100.0,
        loss: t.no_load_loss_kw * 1e3 / 3.0,
        saturation: None,
        tap_changes,
        event_name: None,
    };
    let ends = winding_ends(&g);
    let ends = [ends.0, ends.1];
    // Phase index of an end of the winding of phase p, None for the star.
    let phase_of = |e: End, p: usize| match e {
        End::Phase(o) => Some((p + o) % 3),
        End::Star => None,
    };
    let node_of = |side: usize, e: End, p: usize| match phase_of(e, p) {
        Some(q) => sides[side][q],
        None => stars[side],
    };
    let mut branches = Vec::with_capacity(3);
    for (p, ph) in PHASES.iter().enumerate() {
        let nodes = [
            node_of(0, ends[0].0, p),
            node_of(0, ends[0].1, p),
            node_of(1, ends[1].0, p),
            node_of(1, ends[1].1, p),
        ];
        let name = format!("{}:{ph}", t.name);
        let label = format!("{}:{ph}.", t.name);
        // Named without ':' so that its flux can be read as a signal.
        u.saturation = t
            .saturation
            .as_ref()
            .map(|s| (s, format!("{}.mag.{ph}", t.name)));
        u.event_name = (p == 0).then(|| t.name.clone());
        branches.push(unit(b, &name, &label, nodes, &u));
    }
    // Line current into phase q of a side: the winding currents entering at
    // their dotted end q, minus those leaving at their other end q.
    let mut currents = Vec::with_capacity(6);
    for (side, (dot, other)) in ends.iter().enumerate() {
        for (q, ph) in PHASES.iter().enumerate() {
            let mut terms = Vec::new();
            for (p, w) in branches.iter().enumerate() {
                let e = if side == 0 { w.hv } else { w.lv };
                if phase_of(*dot, p) == Some(q) {
                    terms.push(Term::Current(e, 0, 1.0));
                }
                if phase_of(*other, p) == Some(q) {
                    terms.push(Term::Current(e, 0, -1.0));
                }
            }
            currents.push(Quantity {
                suffix: format!(".{side}.{ph}"),
                terms,
            });
        }
    }
    Expansion::currents(currents)
}

/// One element: V(side 1) = N V(side 0), three current unknowns.
pub(super) fn three_phase_ideal_transformer(
    b: &mut Builder,
    t: &model::ThreePhaseIdealTransformer,
) -> Expansion {
    let side0 = b.bus(&t.buses[0]);
    let side1 = b.bus(&t.buses[1]);
    let changes = t
        .tap_changes
        .iter()
        .map(|ch| {
            (
                ch.time_s,
                model::ThreePhaseIdealTransformer::ratio_matrix_for(
                    ch.rated_voltage_kv,
                    ch.phase_shift_deg,
                    t.zero_sequence_inverted,
                ),
            )
        })
        .collect();
    let e = b.push(Box::new(element::MatrixTransformer {
        name: t.name.clone(),
        side0,
        side1,
        n: t.ratio_matrix(),
        k: 0,
        changes,
        next_change: 0,
    }));
    let mut currents = Vec::with_capacity(6);
    for side in 0..2 {
        for (k, ph) in PHASES.iter().enumerate() {
            currents.push(Quantity {
                suffix: format!(".{side}.{ph}"),
                terms: vec![Term::Current(e, 3 * side + k, 1.0)],
            });
        }
    }
    Expansion::currents(currents)
}
