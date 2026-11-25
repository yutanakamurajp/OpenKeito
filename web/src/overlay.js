// The results of a run on the diagram: which column of result.csv belongs to
// which component, node, bus or signal, the state of every switch, diode and
// fault from the events, and what the diagram shows at one row. Nothing here
// depends on a browser: the same run and the same row give the same frame.
//
// The promises (web/README.md, "Results on the diagram"):
//
// - A value is a cell of result.csv, at the row of the cursor: only what the
//   model records under `outputs` is shown, with the unit of the column.
// - Of a three-phase quantity one phase is shown, the same for the whole
//   diagram (`phase`: "a", "b" or "c").
// - The state of a switch, a diode or a fault at a row is the state that row
//   was computed with: its state at the start and every event before the
//   time of the row. An event at the time of the row itself, or between it
//   and the next row, has not acted yet; the frame says that it is about to.

import { siScale } from "./format.js";

export const PHASES = ["a", "b", "c"];

const LEVEL_RANK = { low: 0, mid: 1, high: 2, over: 3 };
const scaleCache = new WeakMap();

/// Classifies a voltage magnitude against its nominal peak reference.
export function classifyVoltageLevel(value, nominal) {
  if (!Number.isFinite(value) || !Number.isFinite(nominal) || nominal <= 0) return "low";
  const ratio = Math.abs(value) / nominal;
  if (ratio >= 1.1) return "over";
  if (ratio >= 0.9) return "high";
  if (ratio >= 0.45) return "mid";
  return "low";
}

/// Classifies a current magnitude against its running maximum absolute scale.
export function classifyCurrentLevel(value, scale) {
  if (!Number.isFinite(value) || !Number.isFinite(scale) || scale <= 0) return "low";
  const ratio = Math.abs(value) / scale;
  if (ratio >= 0.85) return "over";
  if (ratio >= 0.5) return "high";
  if (ratio >= 0.15) return "mid";
  return "low";
}

/// The component types that have a state, and the words for it.
const STATES = {
  switch: ["open", "closed"],
  controlled_switch: ["open", "closed"],
  diode: ["off", "on"],
  fault: ["no fault", "fault"],
};

/// A value with four significant digits, an SI prefix and its unit:
/// "−12.35 kA", "1.5 mV", "0 V"; without a unit (a signal) five digits.
export function formatQuantity(value, unit) {
  if (typeof value !== "number" || Number.isNaN(value)) return "–";
  if (!Number.isFinite(value)) return value > 0 ? "∞" : "−∞";
  const minus = (text) => text.replace(/^-/, "−");
  if (!unit) return minus(String(Number(value.toPrecision(5))));
  if (value === 0) return `0 ${unit}`;
  const { exponent, prefix } = siScale(Math.abs(value));
  let scaled = Number((value / 10 ** exponent).toPrecision(4));
  let scale = { exponent, prefix };
  // 999.96 rounds to 1000: one prefix up.
  if (Math.abs(scaled) >= 1000 && exponent < 12) {
    scale = siScale(Math.abs(scaled) * 10 ** exponent);
    scaled = Number((value / 10 ** scale.exponent).toPrecision(4));
  }
  return `${minus(String(scaled))} ${scale.prefix}${unit}`;
}

/// What follows the name of the component in the name of a column:
/// "I(T1.0.a)" of T1 is terminal 0, phase "a".
function suffixOf(name, target) {
  const inner = name.slice(name.indexOf("(") + 1, name.endsWith(")") ? -1 : undefined);
  if (!inner.startsWith(target)) return null;
  const rest = inner.slice(target.length);
  if (rest !== "" && !rest.startsWith(".")) return null;
  let terminal = null;
  let phase = null;
  for (const part of rest.split(".").filter((p) => p !== "")) {
    if (/^\d+$/.test(part) && terminal === null && phase === null) terminal = Number(part);
    else if (phase === null) phase = part;
    else return null;
  }
  return { terminal, phase };
}

/// Binds the columns of a result to the diagram.
///
/// `columns`: `result.columns` of the summary ({ name, kind, unit, target }).
/// `header`: the header of result.csv (the first is the time).
///
/// Returns `entries`, one per column that has a place: `owner` is the id of
/// a component ("component:R1"), a rail ("node:n1", "bus:b2") or a signal
/// ("signal:pwm"); `slot` where it is written there ("V", "S", "I", "U", with
/// a terminal "I0", "I1"; null for a column that is only listed, such as the
/// ground current of a fault); `phase` "a", "b", "c" or null. `unbound` are
/// the columns the diagram has no place for.
export function bindColumns(topology, columns, header) {
  const nodes = new Map(topology.nodes.map((node) => [node.name, node]));
  const components = new Map(topology.components.map((c) => [c.name, c]));
  const signals = new Set(topology.signals.map((s) => s.name));
  const entries = [];
  const unbound = [];
  for (const column of columns ?? []) {
    const index = header.indexOf(column.name);
    if (index < 1) continue;
    const entry = { index, name: column.name, kind: column.kind, unit: column.unit ?? "", owner: null, slot: null, phase: null };
    if (column.kind === "node_voltage") {
      const node = nodes.get(column.target);
      if (node && !node.ground) {
        entry.owner = node.bus !== undefined ? `bus:${node.bus}` : `node:${node.name}`;
        entry.nodeOwner = node.bus !== undefined ? `node:${node.name}` : null;
        entry.phase = node.bus !== undefined ? node.phase : null;
        entry.slot = "V";
      }
    } else if (column.kind === "signal") {
      if (signals.has(column.target)) {
        entry.owner = `signal:${column.target}`;
        entry.slot = "S";
      }
    } else if (column.kind === "branch_current" || column.kind === "branch_voltage") {
      const suffix = components.has(column.target) ? suffixOf(column.name, column.target) : null;
      if (suffix) {
        const letter = column.kind === "branch_current" ? "I" : "U";
        entry.owner = `component:${column.target}`;
        entry.phase = suffix.phase;
        const shown = suffix.phase === null || PHASES.includes(suffix.phase);
        const side = suffix.terminal === null ? "" : String(suffix.terminal);
        entry.slot = shown && (side === "" || side === "0" || side === "1") ? `${letter}${side}` : null;
      }
    }
    if (entry.owner === null) unbound.push(column.name);
    else entries.push(entry);
  }
  const byOwner = new Map();
  const byNodeOwner = new Map();
  for (const entry of entries) {
    if (!byOwner.has(entry.owner)) byOwner.set(entry.owner, []);
    byOwner.get(entry.owner).push(entry);
    if (entry.nodeOwner) {
      if (!byNodeOwner.has(entry.nodeOwner)) byNodeOwner.set(entry.nodeOwner, []);
      byNodeOwner.get(entry.nodeOwner).push(entry);
    }
  }
  return { entries, byOwner, byNodeOwner, unbound };
}

/// The names of the columns of a component, a node, a bus or a signal, in
/// the order of the result: what a press on it shows in the waveforms.
export function columnsOf(binding, owner) {
  return (binding?.byOwner.get(owner) ?? binding?.byNodeOwner?.get(owner) ?? []).map((entry) => entry.name);
}

/// The slots that the diagram keeps room for: for every owner, the slots
/// that some column fills ("I", "U", "I0", ...), whatever the phase.
export function slotsOf(binding, timeline) {
  const slots = new Map();
  const add = (owner, slot) => {
    if (!slots.has(owner)) slots.set(owner, new Set());
    slots.get(owner).add(slot);
  };
  for (const entry of binding?.entries ?? []) {
    if (entry.slot !== null) {
      add(entry.owner, entry.slot);
      if (entry.nodeOwner) add(entry.nodeOwner, entry.slot);
    }
  }
  for (const owner of timeline?.units.keys() ?? []) add(owner, "state");
  return slots;
}

/// The states of the switches, diodes and faults over a run.
///
/// `model`: the model as an object (for `initially_closed` of a switch).
/// `events`: every event of the run, in the order of the summary.
///
/// A diode and a controlled switch start blocking, a fault starts off, a
/// switch as `initially_closed` says; every change after that is an event.
/// The phases of a fault have their own events ("F1.a").
export function stateTimeline(topology, model, events) {
  const units = new Map();
  const byName = new Map();
  for (const c of topology.components) {
    if (!Object.hasOwn(STATES, c.type)) continue;
    const index = Number(/^components\[(\d+)\]$/.exec(c.path)?.[1] ?? -1);
    const written = model?.components?.[index];
    const initial = c.type === "switch" && written?.initially_closed === true;
    const phases = c.type === "fault" ? (c.terminals.find((t) => t.phases !== undefined)?.phases ?? []) : [null];
    const list = phases.map((phase) => ({ phase, initial, times: [], on: [] }));
    units.set(`component:${c.name}`, { type: c.type, name: c.name, list });
    list.forEach((unit) => byName.set(unit.phase === null ? c.name : `${c.name}.${unit.phase}`, unit));
  }
  const times = [];
  for (const e of events ?? []) {
    if (!Number.isFinite(e.time_s)) continue;
    if (times.length === 0 || times[times.length - 1] !== e.time_s) times.push(e.time_s);
    const unit = byName.get(e.component);
    if (!unit || (e.action !== "close" && e.action !== "open")) continue;
    unit.times.push(e.time_s);
    unit.on.push(e.action === "close");
  }
  for (const { list } of units.values()) for (const unit of list) unit.times = Float64Array.from(unit.times);
  return { units, times: Float64Array.from(times).sort() };
}

/// The number of values of the sorted `times` that are below `t`.
function countBelow(times, t) {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/// The last row at or before the time `t` (0 when `t` is before the first):
/// the row at which an event at `t` is about to act.
export function rowAtOrBefore(time, t) {
  const n = time.length;
  if (n === 0) return -1;
  // A time written with fewer digits than the row has is still that row.
  const tolerance = n > 1 ? ((time[n - 1] - time[0]) / (n - 1)) * 1e-6 : 0;
  return Math.max(0, countBelow(time, t + tolerance) - 1);
}

/// The time of the first event after the row `row` (`direction` 1), or of
/// the last event before it (-1); null when there is none.
export function neighbouringEvent(timeline, time, row, direction) {
  const times = timeline?.times ?? [];
  if (times.length === 0 || row < 0) return null;
  if (direction > 0) {
    // Not the events that are about to act at this row: those after them.
    const next = row + 1 < time.length ? time[row + 1] : Infinity;
    const k = countBelow(times, next);
    return k < times.length ? times[k] : null;
  }
  const k = countBelow(times, time[row]);
  return k > 0 ? times[k - 1] : null;
}

function stateWord(type, list, on) {
  const words = STATES[type];
  if (type !== "fault") return words[Number(on[0])];
  const phases = list.filter((_, k) => on[k]).map((unit) => unit.phase).join("");
  return phases === "" ? words[0] : `${words[1]} ${phases}`;
}

function windowMedianPeak(col, rows, timeSpanS) {
  const windowCount =
    timeSpanS >= 0.04
      ? Math.min(15, Math.max(3, Math.floor(timeSpanS / 0.02)))
      : Math.min(7, rows);
  const peaks = [];
  for (let w = 0; w < windowCount; w += 1) {
    const start = Math.floor((w * rows) / windowCount);
    const end = Math.floor(((w + 1) * rows) / windowCount);
    let peak = 0;
    for (let r = start; r < end; r += 1) {
      const a = Math.abs(col[r]);
      if (Number.isFinite(a) && a > peak) peak = a;
    }
    if (peak > 0) peaks.push(peak);
  }
  if (peaks.length === 0) return 0;
  peaks.sort((a, b) => a - b);
  return peaks[Math.floor(peaks.length / 2)];
}

/// Detects a positive fundamental AC frequency (Hz) from model or summary metadata, if any.
export function detectFundamentalHz(model, summary = null) {
  const initHz = summary?.initialization?.frequency_hz;
  if (typeof initHz === "number" && Number.isFinite(initHz) && initHz > 0) return initHz;
  for (const comp of Array.isArray(model?.components) ? model.components : []) {
    if (!comp || typeof comp !== "object") continue;
    for (const candidate of [comp.frequency_hz, comp.rated_frequency_hz, comp.waveform?.frequency_hz]) {
      if (typeof candidate === "number" && Number.isFinite(candidate) && candidate > 0) return candidate;
    }
  }
  return null;
}

function trailingCycleMaxAbs(col, startRow, row) {
  if (!col) return 0;
  let peak = 0;
  for (let r = startRow; r <= row; r += 1) {
    const a = Math.abs(col[r]);
    if (Number.isFinite(a) && a > peak) peak = a;
  }
  return peak;
}

/// Estimates deterministic nominal scales (`V_ref` for voltages, `I_max` for
/// currents) once per run so cursor updates avoid re-scanning full columns.
export function computeNominalScales(binding, table, { model = null, summary = null, frequencyHz = null } = {}) {
  const byIndex = [];
  const byName = new Map();
  const byOwner = new Map();
  const threePhaseGroups = [];
  const threePhaseGroupKeys = new Set();
  const resolvedFreqHz =
    typeof frequencyHz === "number" && Number.isFinite(frequencyHz) && frequencyHz > 0
      ? frequencyHz
      : detectFundamentalHz(model, summary);
  const rows = table?.rows ?? 0;
  const columns = table?.columns ?? [];
  const entries = binding?.entries ?? [];
  if (rows <= 0 || columns.length === 0 || entries.length === 0) {
    return { byIndex, byName, byOwner, threePhaseGroups, threePhaseGroupKeys, frequencyHz: resolvedFreqHz };
  }

  const time = columns[0];
  const timeSpanS =
    time && rows > 1 && Number.isFinite(time[rows - 1]) && Number.isFinite(time[0])
      ? time[rows - 1] - time[0]
      : 0;

  let systemMaxV = 0;
  let systemMaxI = 0;
  const voltageStats = [];
  const currentStats = [];

  for (const entry of entries) {
    if (entry.unit !== "V" && entry.unit !== "A") continue;
    const col = columns[entry.index];
    if (!col) continue;
    let colMax = 0;
    for (let r = 0; r < rows; r += 1) {
      const a = Math.abs(col[r]);
      if (Number.isFinite(a) && a > colMax) colMax = a;
    }
    if (entry.unit === "V") {
      if (colMax > systemMaxV) systemMaxV = colMax;
      const med = colMax > 0 ? windowMedianPeak(col, rows, timeSpanS) : 0;
      voltageStats.push({ entry, colMax, med });
    } else {
      if (colMax > systemMaxI) systemMaxI = colMax;
      currentStats.push({ entry, colMax });
    }
  }

  const floorV = Math.max(1e-15, systemMaxV * 1e-9);
  const floorI = Math.max(1e-15, systemMaxI * 1e-9);

  // Group three-phase voltage columns so steady-state initialized buses use
  // the exact t=0 space-vector peak and non-initialized buses share a phase-consistent scale.
  const groups = new Map();
  const assigned = new Set();
  for (const stat of voltageStats) {
    const { entry } = stat;
    if (!PHASES.includes(entry.phase)) continue;
    const key = `${entry.owner}|${entry.slot ?? ""}`;
    if (!groups.has(key)) groups.set(key, new Map());
    groups.get(key).set(entry.phase, stat);
  }
  for (const [key, group] of groups) {
    if (!PHASES.every((p) => group.has(p))) continue;
    const stats = PHASES.map((p) => group.get(p));
    const groupMax = Math.max(...stats.map((s) => s.colMax));
    let scale = 0;
    if (groupMax > floorV) {
      const [va0, vb0, vc0] = stats.map((s) => columns[s.entry.index][0]);
      const v0 = Math.sqrt((2 / 3) * (va0 * va0 + vb0 * vb0 + vc0 * vc0));
      if (Number.isFinite(v0) && v0 > 0.05 * groupMax) {
        scale = v0;
      } else {
        const meds = stats.map((s) => s.med).filter((m) => m > floorV).sort((a, b) => a - b);
        scale = meds.length > 0 ? meds[Math.floor(meds.length / 2)] : groupMax;
      }
    }
    for (const stat of stats) {
      assigned.add(stat.entry.index);
      byIndex[stat.entry.index] = scale;
    }
    threePhaseGroupKeys.add(key);
    threePhaseGroups.push({
      owner: stats[0].entry.owner,
      slot: stats[0].entry.slot ?? null,
      unit: "V",
      idxA: stats[0].entry.index,
      idxB: stats[1].entry.index,
      idxC: stats[2].entry.index,
      scale,
    });
  }

  for (const stat of voltageStats) {
    if (assigned.has(stat.entry.index)) continue;
    const scale = stat.colMax <= floorV ? 0 : stat.med > floorV ? stat.med : stat.colMax;
    byIndex[stat.entry.index] = scale;
  }

  const currentGroups = new Map();
  for (const stat of currentStats) {
    const { entry } = stat;
    byIndex[entry.index] = stat.colMax <= floorI ? 0 : stat.colMax;
    if (!PHASES.includes(entry.phase)) continue;
    const key = `${entry.owner}|${entry.slot ?? ""}`;
    if (!currentGroups.has(key)) currentGroups.set(key, new Map());
    currentGroups.get(key).set(entry.phase, stat);
  }
  for (const [key, group] of currentGroups) {
    if (!PHASES.every((p) => group.has(p))) continue;
    const stats = PHASES.map((p) => group.get(p));
    const groupMax = Math.max(...stats.map((s) => s.colMax));
    const scale = groupMax <= floorI ? 0 : groupMax;
    threePhaseGroupKeys.add(key);
    threePhaseGroups.push({
      owner: stats[0].entry.owner,
      slot: stats[0].entry.slot ?? null,
      unit: "A",
      idxA: stats[0].entry.index,
      idxB: stats[1].entry.index,
      idxC: stats[2].entry.index,
      scale,
    });
  }

  for (const entry of entries) {
    if (entry.unit !== "V" && entry.unit !== "A") continue;
    const scale = byIndex[entry.index] ?? 0;
    byName.set(entry.name, scale);
    const prev = byOwner.get(entry.owner);
    if (prev === undefined || scale > prev) byOwner.set(entry.owner, scale);
    if (entry.nodeOwner) {
      const prevNode = byOwner.get(entry.nodeOwner);
      if (prevNode === undefined || scale > prevNode) byOwner.set(entry.nodeOwner, scale);
    }
  }

  return { byIndex, byName, byOwner, threePhaseGroups, threePhaseGroupKeys, frequencyHz: resolvedFreqHz };
}

function resolveScales(binding, table, scales, { model = null, summary = null, frequencyHz = null } = {}) {
  const explicitHz =
    typeof frequencyHz === "number" && Number.isFinite(frequencyHz) && frequencyHz > 0
      ? frequencyHz
      : detectFundamentalHz(model, summary);
  if (scales) {
    if (explicitHz !== null && !scales.frequencyHz) {
      return { ...scales, frequencyHz: explicitHz };
    }
    return scales;
  }
  if (!binding || !table || !(table.rows > 0)) return null;
  let base;
  if (table.rows * (binding.entries?.length ?? 0) <= 20000) {
    base = computeNominalScales(binding, table, { frequencyHz: explicitHz });
  } else {
    let byBinding = scaleCache.get(table);
    if (!byBinding) {
      byBinding = new WeakMap();
      scaleCache.set(table, byBinding);
    }
    let cached = byBinding.get(binding);
    if (!cached) {
      cached = computeNominalScales(binding, table);
      byBinding.set(binding, cached);
    }
    base = cached;
  }
  if (explicitHz !== null && explicitHz !== base.frequencyHz) {
    return { ...base, frequencyHz: explicitHz };
  }
  return base;
}

function recordLevel(levels, id, candidate) {
  const prev = levels.get(id);
  if (
    !prev ||
    LEVEL_RANK[candidate.level] > LEVEL_RANK[prev.level] ||
    (LEVEL_RANK[candidate.level] === LEVEL_RANK[prev.level] && candidate.ratio > prev.ratio)
  ) {
    levels.set(id, candidate);
  }
}

/// What the diagram shows at the row `row`.
///
/// Returns `texts` (slot id "<owner>|<slot>" -> text; a slot without a value
/// at this phase is not in it), `states` (component id -> { on, word,
/// acting }: `on` whether it conducts, `word` what is written, `acting` true
/// when an event changes it before the next row), `levels` (owner id ->
/// { level, ratio, value, unit }) and the time.
export function frameAt({
  binding,
  timeline,
  table,
  row,
  phase = "a",
  scales = null,
  model = null,
  summary = null,
  frequencyHz = null,
}) {
  const texts = new Map();
  const states = new Map();
  const levels = new Map();
  const time = table?.columns?.[0];
  if (!time || !(row >= 0 && row < time.length)) return { row: -1, time: NaN, phase, texts, states, levels };
  const resolvedScales = resolveScales(binding, table, scales, { model, summary, frequencyHz });
  const t = time[row];
  const n = time.length;
  const tolerance = n > 1 ? ((time[n - 1] - time[0]) / (n - 1)) * 1e-6 : 0;
  const next = row + 1 < n ? time[row + 1] : Infinity;

  const cycleHz = resolvedScales?.frequencyHz ?? null;
  let startRow = row;
  if (cycleHz > 0 && n > 1 && Number.isFinite(t)) {
    startRow = Math.min(row, countBelow(time, t - 1 / cycleHz + tolerance));
  }

  for (const group of resolvedScales?.threePhaseGroups ?? []) {
    const va = table.columns[group.idxA]?.[row];
    const vb = table.columns[group.idxB]?.[row];
    const vc = table.columns[group.idxC]?.[row];
    const mag =
      Number.isFinite(va) && Number.isFinite(vb) && Number.isFinite(vc)
        ? Math.sqrt((2 / 3) * (va * va + vb * vb + vc * vc))
        : NaN;
    const ratio = Number.isFinite(mag) && group.scale > 0 ? mag / group.scale : 0;
    const level = group.unit === "V" ? classifyVoltageLevel(mag, group.scale) : classifyCurrentLevel(mag, group.scale);
    recordLevel(levels, group.owner, { level, ratio, value: mag, unit: group.unit });
  }

  for (const entry of binding?.entries ?? []) {
    if (entry.unit === "V" || entry.unit === "A") {
      const col = table.columns[entry.index];
      const scale = resolvedScales?.byIndex?.[entry.index] ?? 0;
      const inThreePhaseGroup =
        PHASES.includes(entry.phase) &&
        resolvedScales?.threePhaseGroupKeys?.has(`${entry.owner}|${entry.slot ?? ""}`);
      if (entry.nodeOwner || (!inThreePhaseGroup && (entry.phase === null || entry.phase === phase))) {
        const mag = trailingCycleMaxAbs(col, startRow, row);
        const ratio = Number.isFinite(mag) && scale > 0 ? mag / scale : 0;
        const level = entry.unit === "V" ? classifyVoltageLevel(mag, scale) : classifyCurrentLevel(mag, scale);
        const info = { level, ratio, value: mag, unit: entry.unit };
        if (entry.nodeOwner) recordLevel(levels, entry.nodeOwner, info);
        if (!inThreePhaseGroup && (entry.phase === null || entry.phase === phase)) {
          recordLevel(levels, entry.owner, info);
        }
      }
    }
    if (entry.slot !== null) {
      const formatted = formatQuantity(table.columns[entry.index][row], entry.unit);
      if (entry.phase === null || entry.phase === phase) {
        texts.set(`${entry.owner}|${entry.slot}`, formatted);
      }
      if (entry.nodeOwner) {
        texts.set(`${entry.nodeOwner}|${entry.slot}`, formatted);
      }
    }
  }
  for (const [owner, { type, list }] of timeline?.units ?? []) {
    const at = (limit) =>
      list.map((unit) => {
        const k = countBelow(unit.times, limit - tolerance);
        return k === 0 ? unit.initial : unit.on[k - 1];
      });
    const now = at(t);
    const then = at(next);
    const acting = now.some((on, k) => on !== then[k]);
    const word = stateWord(type, list, now);
    states.set(owner, {
      on: now.some(Boolean),
      acting,
      word: acting ? `${word} ▸ ${stateWord(type, list, then)}` : word,
    });
    texts.set(`${owner}|state`, states.get(owner).word);
  }
  return { row, time: t, phase, texts, states, levels };
}

/// The columns of an owner with their values at a row, for the line below
/// the diagram: "I(F1.a) = 1.234 kA, I(F1.g) = 1.234 kA".
export function describeValues(binding, table, row, owner) {
  if (!table || !(row >= 0)) return "";
  return (binding?.byOwner.get(owner) ?? binding?.byNodeOwner?.get(owner) ?? [])
    .map((entry) => `${entry.name} = ${formatQuantity(table.columns[entry.index][row], entry.unit)}`)
    .join(", ");
}

/// The owners of result columns represented by a placed diagram item.
export function itemOwners(placed, item) {
  if (item.category === "circuit") return [item.id];
  if (item.category === "reduced") return (item.members ?? []).map((name) => `component:${name}`);
  const signals = item.ports
    .filter((p) => p.side === "R")
    .flatMap((p) => p.rails.map((r) => placed.rails[r.rail].id));
  return item.category === "control" ? signals : [`component:${item.name}`, ...signals];
}
