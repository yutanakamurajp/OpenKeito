// The diagram of a model, placed: from the document of `topology` to
// positions, without a browser and without chance (the same document gives
// the same positions).
//
// The arrangement is one for both faces of the diagram:
//
// - Every net is a vertical line, a *rail*: a bus (drawn as a bar), a node,
//   or on the face of the controls a signal. The rails stand side by side
//   in the order in which a breadth-first walk from the first source finds
//   them (the controls: in the order in which the blocks are evaluated).
// - Every component lies in the space between two rails, to the right of
//   its leftmost rail (a block: to the left of its first output), and
//   reaches its rails with horizontal wires. It has a band of its own over
//   all the spaces its wires pass, so that a wire never runs through
//   another component; the bands are packed from the top (first fit, which
//   for intervals sorted by their start needs the fewest rows).
// - A wire that has to go against its side (a feedback, both ends on one
//   rail) leaves the symbol, steps down below it and runs back.
//
// Layered drawings of this kind go back to Sugiyama, Tagawa and Toda,
// "Methods for visual understanding of hierarchical system structures"
// (IEEE Trans. SMC 11, 1981); the packing is the greedy colouring of
// interval graphs.

import { applyLayoutOverrides, parseLayoutSidecar } from "./layout_sidecar.js";
import { SYMBOL_WIDTH, TRACK, circuitPorts, circuitTracks } from "./symbols.js";

/// The measures of the drawing, in px.
export const MEASURES = {
  /// From rail to rail.
  column: 124,
  /// From a rail to the cell of a component.
  pad: 20,
  /// The line of the name above a symbol.
  label: 14,
  /// From a line of ports to the edge of the body.
  half: 14,
  /// Between the wires that step down below a symbol.
  escape: 8,
  /// The line of the links below a symbol.
  chip: 14,
  /// Between two components.
  spacing: 8,
  /// Around the drawing.
  margin: 14,
  /// The names of the rails.
  header: 18,
  /// A further line of a name that is too long for one.
  line: 11,
  /// A line of values of a run below a symbol or below the name of a rail.
  value: 11,
};

/// The widths of the names, in characters: a longer name goes on over
/// further lines (at most LINES of them), it is not cut.
const ITEM_CHARS = 16;
const RAIL_CHARS = 18;
const CHIP_CHARS = 21;
const LINES = 3;

/// More components than this are not drawn one by one: the diagram shows
/// the buses and what joins them.
export const MAX_COMPONENTS = 1500;

/// More components than this stay in single-line representation even when
/// three-phase expansion is requested.
export const MAX_THREE_PHASE_EXPAND = 400;

const SOURCES = new Set([
  "voltage_source",
  "three_phase_source",
  "synchronous_machine",
  "controlled_voltage_source",
  "three_phase_controlled_source",
  "grid_following_converter",
  "current_source",
  "three_phase_current_source",
  "controlled_current_source",
  "three_phase_controlled_current_source",
]);

function shorten(text, max) {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/// A name on lines of at most `width` characters: broken after "_", ".",
/// "-", ",", "/", ":", ")" or a space, or before "(", where there is such a
/// place in the second half of a line, and anywhere else. Only what does not
/// fit on `maxLines` lines is cut (and then ends with "…").
export function wrapLabel(text, width, maxLines = LINES) {
  const lines = [];
  let rest = text;
  while (rest.length > width && lines.length < maxLines - 1) {
    let at = width;
    for (let k = width; k > width / 2; k -= 1) {
      if ("_.-,/:) ".includes(rest[k - 1]) || rest[k] === "(") {
        at = k;
        break;
      }
    }
    lines.push(rest.slice(0, at));
    rest = rest.slice(at);
  }
  lines.push(shorten(rest, width));
  return lines;
}

/// The rows of values below a symbol: the state, the currents, the branch
/// voltages; each with the slots that are written in it.
const VALUE_ROWS = [["state"], ["I", "I0", "I1"], ["U", "U0", "U1"]];

/// The links of an item or a rail on lines: side by side while they fit
/// into `chars` characters, a long one on lines of its own; after `maxRows`
/// lines, the number of those left out. Positions are relative: `dx` in px,
/// `row` the line.
function flowChips(chips, chars, maxRows) {
  const placed = [];
  let row = 0;
  let used = 0;
  // Where the line before the current one ends.
  let before = 0;
  for (let k = 0; k < chips.length; k += 1) {
    const chip = chips[k];
    const lines = wrapLabel(chip.text, chars, 2);
    const length = Math.max(...lines.map((l) => l.length));
    if (used > 0 && (lines.length > 1 || used + length + 1 > chars)) {
      row += 1;
      before = used;
      used = 0;
    }
    if (row + lines.length > maxRows && k > 0) {
      // No room: the number of those left out, after the last one shown.
      const rest = chips.slice(k);
      const last = Math.min(row, maxRows - 1);
      const dx = (last < row ? before : used) * 5.6;
      placed.push({ dx, row: last, lines: [`+${rest.length}`], title: rest.map((c) => c.text).join(", "), goto: chip.goto });
      return { placed, rows: last + 1 };
    }
    placed.push({ dx: used * 5.6, row, lines, title: chip.title, goto: chip.goto });
    if (lines.length > 1) {
      row += lines.length;
      before = length + 1;
      used = 0;
    } else {
      used += length + 1;
    }
  }
  return { placed, rows: used > 0 ? row + 1 : row };
}

const worse = (a, b) => (a === "error" || b === "error" ? "error" : (a ?? b ?? null));

/// The severity of the diagnostics about every component, block, node and
/// bus.
function severities(topology) {
  const marks = { components: new Map(), controls: new Map(), nodes: new Map(), buses: new Map() };
  for (const d of topology.diagnostics ?? []) {
    for (const kind of Object.keys(marks)) {
      for (const name of d[kind] ?? []) marks[kind].set(name, worse(marks[kind].get(name), d.severity));
    }
  }
  return marks;
}

/// What a measurement block reads, as the column it would be.
function measuredText(m) {
  const suffix = `${m.terminal === undefined ? "" : `.${m.terminal}`}${m.phase === undefined ? "" : `.${m.phase}`}`;
  if (m.type === "node_voltage") return `V(${m.node})`;
  if (m.type === "bus_voltage") return `V(${m.bus})`;
  if (m.type === "branch_current") return `I(${m.component}${suffix})`;
  if (m.type === "branch_voltage") return `U(${m.component}${suffix})`;
  if (m.type === "component_signal") return `${m.component}.${m.signal}`;
  return m.type ?? "";
}

// ---- the face of the circuit ----

/// The rails of the circuit and where every node is on them: buses and the
/// nodes that are no phase of a bus; gnd has no rail (every connection to it
/// is drawn as an earth symbol). When `expandThreePhase` is true, each bus
/// expands into three phase rails (`<bus>.a`, `<bus>.b`, `<bus>.c`).
function circuitRails(topology, marks, expandThreePhase = false) {
  const rails = [];
  const index = new Map();
  const add = (key, rail) => {
    if (!index.has(key)) {
      index.set(key, rails.length);
      rails.push(rail);
    }
  };
  const nodeRail = new Map();
  if (expandThreePhase) {
    const phases = ["a", "b", "c"];
    for (const bus of topology.buses) {
      const busNodes = Array.isArray(bus.nodes) && bus.nodes.length > 0 ? bus.nodes : phases.map((p) => `${bus.name}.${p}`);
      busNodes.forEach((nodeName, idx) => {
        const phase = phases[idx] ?? nodeName.split(".").pop() ?? null;
        const key = `node:${nodeName}`;
        add(key, {
          id: key,
          kind: "node",
          name: nodeName,
          bus: bus.name,
          phase,
          severity: worse(marks.buses.get(bus.name) ?? null, marks.nodes.get(nodeName) ?? null),
        });
        nodeRail.set(nodeName, { rail: index.get(key), phase });
      });
    }
    for (const node of topology.nodes) {
      if (node.ground) continue;
      const key = `node:${node.name}`;
      if (index.has(key)) {
        const k = index.get(key);
        rails[k].severity = worse(rails[k].severity, marks.nodes.get(node.name));
        nodeRail.set(node.name, { rail: k, phase: node.phase ?? rails[k].phase ?? null });
      } else {
        add(key, {
          id: key,
          kind: "node",
          name: node.name,
          severity: marks.nodes.get(node.name) ?? null,
        });
        nodeRail.set(node.name, { rail: index.get(key), phase: node.phase ?? null });
      }
    }
    return { rails, nodeRail };
  }
  for (const bus of topology.buses) {
    add(`bus:${bus.name}`, { id: `bus:${bus.name}`, kind: "bus", name: bus.name, severity: marks.buses.get(bus.name) ?? null });
  }
  for (const node of topology.nodes) {
    if (node.ground) continue;
    if (node.bus !== undefined) {
      const k = index.get(`bus:${node.bus}`);
      nodeRail.set(node.name, { rail: k, phase: node.phase });
      rails[k].severity = worse(rails[k].severity, marks.nodes.get(node.name));
    } else {
      add(`node:${node.name}`, {
        id: `node:${node.name}`,
        kind: "node",
        name: node.name,
        severity: marks.nodes.get(node.name) ?? null,
      });
      nodeRail.set(node.name, { rail: index.get(`node:${node.name}`), phase: null });
    }
  }
  return { rails, nodeRail };
}

/// The order of the rails: a breadth-first walk over the components, from
/// the rails of the first source, then from every rail not yet reached.
function railOrder(count, touched, components) {
  const neighbours = Array.from({ length: count }, () => []);
  for (const rails of touched) {
    for (const a of rails) for (const b of rails) if (a !== b) neighbours[a].push(b);
  }
  const order = [];
  const seen = new Array(count).fill(false);
  const walk = (start) => {
    if (seen[start]) return;
    seen[start] = true;
    const queue = [start];
    for (let head = 0; head < queue.length; head += 1) {
      const rail = queue[head];
      order.push(rail);
      for (const next of neighbours[rail]) {
        if (!seen[next]) {
          seen[next] = true;
          queue.push(next);
        }
      }
    }
  };
  const source = components.findIndex((c, i) => SOURCES.has(c.type) && touched[i].length > 0);
  if (source >= 0) walk(touched[source][0]);
  for (const rails of touched) for (const rail of rails) walk(rail);
  for (let rail = 0; rail < count; rail += 1) walk(rail);
  return order;
}

/// What a terminal is connected to: gnd, and rails with the tag written at
/// the rail (the phase of a bus, the phases at a star point).
function connections(terminal, nodeRail, rails) {
  const out = { ground: false, rails: new Map() };
  const phases = terminal.phases ?? [];
  terminal.nodes.forEach((node, k) => {
    const at = nodeRail.get(node);
    if (!at) {
      out.ground = true;
      return;
    }
    if (!out.rails.has(at.rail)) out.rails.set(at.rail, []);
    out.rails.get(at.rail).push({ of: phases[k] ?? null, phase: at.phase });
  });
  const tagged = [];
  for (const [rail, parts] of out.rails) {
    // All three phases of a bus, each at its own: nothing to say.
    const whole = rails[rail].kind === "bus" && parts.length === 3 && parts.every((p) => p.of === p.phase);
    // Otherwise the phase of the bus ("a"), the phases of the component at
    // a node ("abc"), or which phase is at which ("a→b").
    const one = (p) => {
      if (p.of === null) return p.phase ?? "";
      return p.phase === null || p.phase === p.of ? p.of : `${p.of}→${p.phase}`;
    };
    const texts = whole ? [] : parts.map(one).filter((t) => t !== "");
    tagged.push({ rail, tag: texts.join(texts.some((t) => t.length > 1) ? " " : "") });
  }
  return { ground: out.ground, rails: tagged };
}

/// The diagram of the circuit, not yet placed: its rails and its
/// components. With more than `maxComponents` components, only the buses
/// and one line for all the components between two of them.
export function circuitFace(topology, { maxComponents = MAX_COMPONENTS, expandThreePhase = false } = {}) {
  const marks = severities(topology);
  const reduced = topology.components.length > maxComponents;
  const overExpandLimit = Boolean(expandThreePhase) && !reduced && topology.components.length > MAX_THREE_PHASE_EXPAND;
  const expand = Boolean(expandThreePhase) && !reduced && !overExpandLimit;
  const all = circuitRails(topology, marks, expand);
  const touchedBy = (c) => {
    const rails = [];
    for (const t of c.terminals) {
      for (const node of t.nodes) {
        const at = all.nodeRail.get(node);
        if (at && !rails.includes(at.rail) && (!reduced || all.rails[at.rail].kind === "bus")) rails.push(at.rail);
      }
    }
    return rails;
  };
  const touched = topology.components.map(touchedBy);
  const order = railOrder(all.rails.length, touched, topology.components).filter(
    (rail) => !reduced || all.rails[rail].kind === "bus",
  );
  const position = new Map(order.map((rail, k) => [rail, k]));
  const rails = order.map((rail) => ({ ...all.rails[rail], chips: [] }));
  const nodeRail = new Map();
  for (const [node, at] of all.nodeRail) {
    if (position.has(at.rail)) nodeRail.set(node, { rail: position.get(at.rail), phase: at.phase });
  }

  if (reduced) {
    // One line for the components between two buses.
    const pairs = new Map();
    topology.components.forEach((c, i) => {
      const at = touched[i].map((rail) => position.get(rail)).sort((a, b) => a - b);
      for (let a = 0; a < at.length; a += 1) {
        for (let b = a + 1; b < at.length; b += 1) {
          const key = `${at[a]} ${at[b]}`;
          if (!pairs.has(key)) pairs.set(key, { from: at[a], to: at[b], members: [] });
          pairs.get(key).members.push(c);
        }
      }
    });
    const items = [...pairs.values()].map((pair) => ({
      id: `between:${rails[pair.from].name}:${rails[pair.to].name}`,
      category: "reduced",
      type: "between",
      name: pair.members.length === 1 ? pair.members[0].name : `${pair.members.length} components`,
      title: pair.members.map((c) => `${c.name} (${c.type})`).join(", "),
      path: pair.members[0].path,
      members: pair.members.map((c) => c.name),
      tracks: 1,
      flip: false,
      gap: pair.from + 1,
      series: true,
      names: new Set(),
      ports: [
        { side: "L", track: 0, ground: false, rails: [{ rail: pair.from, tag: "" }] },
        { side: "R", track: 0, ground: false, rails: [{ rail: pair.to, tag: "" }] },
      ],
      chips: [],
      severity: pair.members.reduce((s, c) => worse(s, marks.components.get(c.name)), null),
    }));
    return {
      kind: "circuit",
      reduced: true,
      note:
        `This model has ${topology.components.length.toLocaleString("en-US")} components: the diagram shows its ` +
        `${rails.length.toLocaleString("en-US")} buses and one line for the components between two of them.`,
      rails,
      items,
      nodeRail,
    };
  }

  // What the controls read in the circuit and act on.
  const owners = new Set();
  const measuredBy = { component: new Map(), node: new Map(), bus: new Map() };
  for (const c of topology.controls) {
    if (c.owner !== undefined) {
      owners.add(c.owner);
      continue;
    }
    const m = c.measures;
    if (!m) continue;
    for (const kind of ["component", "node", "bus"]) {
      if (m[kind] === undefined) continue;
      if (!measuredBy[kind].has(m[kind])) measuredBy[kind].set(m[kind], []);
      measuredBy[kind].get(m[kind]).push(c.name);
    }
  }
  const measured = (names) => names.map((name) => ({ text: `→ ${name}`, title: `Read by the block ${name}`, goto: { face: "controls", control: name } }));
  for (const rail of rails) {
    rail.chips = measured(measuredBy[rail.kind].get(rail.name) ?? []);
    if (expand && rail.bus !== undefined && rail.phase === "a") {
      rail.chips.push(...measured(measuredBy.bus.get(rail.bus) ?? []));
    }
  }
  for (const [node, blocks] of measuredBy.node) {
    const at = nodeRail.get(node);
    // A phase of a bus that a block reads: the link is at the bus.
    if (at && rails[at.rail].kind === "bus") rails[at.rail].chips.push(...measured(blocks));
  }

  const items = topology.components.map((c) => {
    const names = c.terminals.map((t) => t.name);
    const ports = circuitPorts(c.type, names);
    const conns = c.terminals.map((t) => connections(t, nodeRail, rails));
    const reached = conns.flatMap((x) => x.rails.map((r) => r.rail));
    const leftmost = reached.length > 0 ? Math.min(...reached) : -1;
    const touches = (side) => ports.some((p, k) => p.side === side && conns[k].rails.some((r) => r.rail === leftmost));
    const flip = leftmost >= 0 && !touches("L") && touches("R");
    const other = { L: "R", R: "L" };
    const chips = [
      ...(c.signals ?? []).map((s) => ({
        text: `← ${s.signal}`,
        title: `${s.field} is the signal ${s.signal}`,
        goto: { face: "controls", signal: s.signal },
      })),
      ...measured(measuredBy.component.get(c.name) ?? []),
      ...(owners.has(c.name)
        ? [{ text: "→ control", title: `The control blocks of ${c.name}`, goto: { face: "controls", owner: c.name } }]
        : []),
    ];
    const tracks = Math.max(circuitTracks(c.type, new Set(names)), ...ports.map((p) => p.track + 1));
    return {
      id: `component:${c.name}`,
      category: "circuit",
      type: c.type,
      name: c.name,
      title: `${c.name} (${c.type})`,
      path: c.path,
      tracks,
      flip,
      gap: leftmost + 1,
      series: new Set(reached).size > 1,
      names: new Set(names),
      ports: ports.map((p, k) => ({
        side: flip ? other[p.side] : p.side,
        track: p.track,
        ground: conns[k].ground,
        rails: conns[k].rails,
        terminal: c.terminals[k].name,
      })),
      chips,
      severity: marks.components.get(c.name) ?? null,
    };
  });
  const note = overExpandLimit
    ? `This model has ${topology.components.length.toLocaleString("en-US")} components (limit for three-phase expansion: ` +
      `${MAX_THREE_PHASE_EXPAND.toLocaleString("en-US")}): showing the single-line diagram.`
    : "";
  return { kind: "circuit", reduced: false, note, rails, items, nodeRail };
}

// ---- the face of the controls ----

/// The block diagram, not yet placed: a rail for every signal, the blocks,
/// one box for all the blocks that a component brings with it, and a box
/// for every component that reads a signal.
export function controlsFace(topology) {
  const marks = severities(topology);
  const byName = new Map(topology.controls.map((c) => [c.name, c]));
  const components = new Map(topology.components.map((c) => [c.name, c]));
  const producer = new Map();
  for (const s of topology.signals) producer.set(s.name, s.from === null ? null : (byName.get(s.from) ?? null));
  const ownerOfSignal = (signal) => producer.get(signal)?.owner;

  // The boxes that give signals: the blocks of the model, and the owners.
  const boxes = [];
  const boxOf = new Map(); // control name or "owner:<name>" -> box
  for (const c of topology.controls) {
    if (c.owner !== undefined) continue;
    const box = {
      id: `control:${c.name}`,
      category: "control",
      type: c.type,
      name: c.name,
      title: `${c.name} (${c.type})`,
      path: c.path,
      inputs: c.inputs.map((i) => ({ signal: i.signal, label: "" })),
      outputs: c.outputs.map((s) => ({ signal: s, label: c.outputs.length > 1 ? s.slice(c.name.length + 1) : "" })),
      chips: c.measures
        ? [
            {
              text: `← ${measuredText(c.measures)}`,
              title: "Show in the circuit",
              goto: { face: "circuit", component: c.measures.component, node: c.measures.node, bus: c.measures.bus },
            },
          ]
        : [],
      severity: marks.controls.get(c.name) ?? null,
    };
    boxes.push(box);
    boxOf.set(c.name, box);
  }
  const owned = new Map();
  for (const c of topology.controls) {
    if (c.owner === undefined) continue;
    if (!owned.has(c.owner)) owned.set(c.owner, []);
    owned.get(c.owner).push(c);
  }
  for (const [owner, blocks] of owned) {
    const component = components.get(owner);
    const inputs = [];
    const read = (signal) => {
      if (ownerOfSignal(signal) !== owner && !inputs.includes(signal)) inputs.push(signal);
    };
    for (const s of component?.signals ?? []) read(s.signal);
    for (const b of blocks) for (const i of b.inputs) read(i.signal);
    // The signals of the owner that something else reads.
    const outputs = [];
    for (const s of topology.signals) {
      if (ownerOfSignal(s.name) !== owner) continue;
      const outside = s.to.some(
        (end) =>
          end.output !== undefined ||
          (end.component !== undefined && end.component !== owner) ||
          (end.control !== undefined && byName.get(end.control)?.owner !== owner),
      );
      if (outside) outputs.push(s.name);
    }
    const box = {
      id: `owner:${owner}`,
      category: "owner",
      type: component?.type ?? "",
      name: owner,
      title: `The ${blocks.length} control blocks of ${owner} (${component?.type ?? ""})`,
      path: component?.path ?? blocks[0].path,
      inputs: inputs.map((signal) => ({ signal, label: "" })),
      outputs: outputs.map((signal) => ({ signal, label: signal.slice(owner.length + 1) })),
      chips: [{ text: "← circuit", title: "Show in the circuit", goto: { face: "circuit", component: owner } }],
      severity: marks.components.get(owner) ?? null,
    };
    boxes.push(box);
    boxOf.set(`owner:${owner}`, box);
  }
  const boxGiving = (signal) => {
    const block = producer.get(signal);
    if (!block) return null;
    return block.owner === undefined ? boxOf.get(block.name) : boxOf.get(`owner:${block.owner}`);
  };

  // The order of the boxes: every box after those it reads, as far as a
  // loop allows (a depth-first walk in the order of the model).
  const ordered = [];
  const state = new Map();
  for (const start of boxes) {
    if (state.has(start)) continue;
    const stack = [[start, 0]];
    state.set(start, "open");
    while (stack.length > 0) {
      const top = stack[stack.length - 1];
      const [box, next] = top;
      if (next < box.inputs.length) {
        top[1] += 1;
        const from = boxGiving(box.inputs[next].signal);
        if (from && !state.has(from)) {
          state.set(from, "open");
          stack.push([from, 0]);
        }
      } else {
        state.set(box, "done");
        ordered.push(box);
        stack.pop();
      }
    }
  }

  // The components that read signals and bring no blocks of their own.
  const sinks = [];
  for (const c of topology.components) {
    if ((c.signals ?? []).length === 0 || owned.has(c.name)) continue;
    sinks.push({
      id: `sink:${c.name}`,
      category: "sink",
      type: c.type,
      name: c.name,
      title: `${c.name} (${c.type}) in the circuit`,
      path: c.path,
      inputs: c.signals.map((s) => ({ signal: s.signal, label: s.field })),
      outputs: [],
      chips: [{ text: "← circuit", title: "Show in the circuit", goto: { face: "circuit", component: c.name } }],
      severity: marks.components.get(c.name) ?? null,
    });
  }

  // The rails: first the names that no block gives, then the outputs of
  // the boxes in their order.
  const rails = [];
  const railOf = new Map();
  const addRail = (signal, missing) => {
    if (railOf.has(signal)) return;
    railOf.set(signal, rails.length);
    rails.push({
      id: `signal:${signal}`,
      kind: "signal",
      name: signal,
      severity: null,
      missing,
      chips: [],
    });
  };
  for (const box of [...ordered, ...sinks]) {
    for (const input of box.inputs) if (!boxGiving(input.signal)) addRail(input.signal, true);
  }
  for (const box of ordered) for (const output of box.outputs) addRail(output.signal, false);

  const items = [...ordered, ...sinks].map((box) => {
    const inputs = box.inputs.map((i) => railOf.get(i.signal));
    const outputs = box.outputs.map((o) => railOf.get(o.signal));
    const gap = outputs.length > 0 ? outputs[0] : inputs.length > 0 ? Math.max(...inputs) + 1 : 0;
    return {
      id: box.id,
      category: box.category,
      type: box.type,
      name: box.name,
      title: box.title,
      path: box.path,
      tracks: Math.max(1, inputs.length, outputs.length),
      flip: false,
      gap,
      series: true,
      names: new Set(),
      ports: [
        ...inputs.map((rail, k) => ({ side: "L", track: k, ground: false, rails: [{ rail, tag: "" }], label: box.inputs[k].label })),
        ...outputs.map((rail, k) => ({ side: "R", track: k, ground: false, rails: [{ rail, tag: "" }], label: box.outputs[k].label })),
      ],
      chips: box.chips,
      severity: box.severity,
    };
  });
  return { kind: "controls", reduced: false, note: "", rails, items, signalRail: railOf };
}

// ---- placing ----

/// Places a face: every rail gets its x and its length, every item its
/// cell, and the wires, the dots where a wire ends on a rail, the tags and
/// the earth symbols their coordinates.
///
/// `slots`: for the results of a run (overlay.js, `slotsOf`), the slots to
/// keep room for, by the id of the item or rail. Without it the face is
/// placed as the model alone gives it.
export function place(face, { slots = new Map(), sidecar = null, overrides = null } = {}) {
  const M = MEASURES;
  const cellWidth = M.column - 2 * M.pad;
  const inset = (cellWidth - SYMBOL_WIDTH) / 2;
  const railCount = face.rails.length;

  // The height of every item and the spaces it needs.
  const plans = face.items.map((item, index) => {
    const detours = [];
    for (const port of item.ports) {
      const against = port.rails.filter((r) => (port.side === "L" ? r.rail >= item.gap : r.rail < item.gap));
      if (against.length > 0) detours.push(port);
    }
    const body = 2 * M.half + (item.tracks - 1) * TRACK;
    const escapes = detours.length > 0 ? 5 + (detours.length - 1) * M.escape + 3 : 0;
    const single = item.category === "reduced" && item.members.length === 1;
    const labelLines = single ? [] : wrapLabel(item.name, ITEM_CHARS);
    const label = M.label + Math.max(0, labelLines.length - 1) * M.line;
    const wanted = slots.get(item.id);
    const valueRows = wanted ? VALUE_ROWS.map((row) => row.filter((slot) => wanted.has(slot))).filter((row) => row.length > 0) : [];
    const flow = flowChips(item.chips, CHIP_CHARS, 12);
    const reached = item.ports.flatMap((p) => p.rails.map((r) => r.rail));
    const from = Math.min(item.gap, ...reached.map((r) => r + 1));
    const to = Math.max(item.gap, ...reached);
    const height = label + body + escapes + valueRows.length * M.value + flow.rows * M.chip + M.spacing;
    return { item, index, detours, body, escapes, labelLines, label, valueRows, flow, height, from, to };
  });

  // The bands, packed from the top: for every space, what is taken.
  const taken = Array.from({ length: railCount + 1 }, () => []);
  const order = [...plans].sort(
    (a, b) => a.from - b.from || (face.kind === "circuit" ? Number(!a.item.series) - Number(!b.item.series) : 0) || a.index - b.index,
  );
  for (const plan of order) {
    const candidates = [0];
    for (let gap = plan.from; gap <= plan.to; gap += 1) for (const [, end] of taken[gap]) candidates.push(end);
    candidates.sort((a, b) => a - b);
    const free = (y) => {
      for (let gap = plan.from; gap <= plan.to; gap += 1) {
        for (const [start, end] of taken[gap]) if (y < end && start < y + plan.height) return false;
      }
      return true;
    };
    plan.y = candidates.find(free);
    for (let gap = plan.from; gap <= plan.to; gap += 1) taken[gap].push([plan.y, plan.y + plan.height]);
  }

  // Coordinates. The first space that is used is at the left margin.
  const usedFrom = plans.length > 0 ? Math.min(...plans.map((p) => p.item.gap)) : 0;
  const usedTo = plans.length > 0 ? Math.max(...plans.map((p) => p.item.gap)) : 0;
  let leftEdge = plans.length > 0 ? usedFrom * M.column : Infinity;
  if (railCount > 0) leftEdge = Math.min(leftEdge, M.column / 2);
  if (!Number.isFinite(leftEdge)) leftEdge = 0;
  const x0 = M.margin - leftEdge;
  const railX = (rail) => x0 + (rail + 1) * M.column;
  // Above the rails: their names, the values of a run, the links.
  const heads = face.rails.map((rail) => ({
    lines: wrapLabel(rail.name, RAIL_CHARS),
    slot: ["V", "S"].find((slot) => slots.get(rail.id)?.has(slot)) ?? null,
    flow: flowChips(rail.chips, CHIP_CHARS, 6),
  }));
  const headLines = Math.max(1, ...heads.map((head) => head.lines.length));
  const headValue = heads.some((head) => head.slot !== null) ? M.value : 0;
  const headChips = Math.max(0, ...heads.map((head) => head.flow.rows)) * M.chip;
  // The line of the last line of the names.
  const headBase = M.margin + M.header - 5 + (headLines - 1) * M.line;
  const top = headBase + 5 + headValue + headChips + 6;

  const wires = [];
  const dots = [];
  const tags = [];
  const grounds = [];
  const portLabels = [];
  const chips = [];
  const railEnd = new Array(railCount).fill(top + 10);
  const items = [];

  for (const plan of plans) {
    const { item } = plan;
    const cellX = x0 + item.gap * M.column + M.pad;
    const y = top + plan.y;
    const sx = cellX + inset;
    const line0 = y + plan.label + M.half;
    const bodyBottom = line0 + (item.tracks - 1) * TRACK + M.half;
    const own = { wires: [], dots: [], tags: [], grounds: [], portLabels: [], arrows: [] };
    let escape = 0;
    const jogs = { L: 0, R: 0 };
    const touch = (rail, yy) => {
      own.dots.push({ x: railX(rail), y: yy, rail });
      railEnd[rail] = Math.max(railEnd[rail], yy);
    };
    for (const port of item.ports) {
      const left = port.side === "L";
      const dir = left ? -1 : 1;
      const py = line0 + port.track * TRACK;
      const px = left ? sx : sx + SYMBOL_WIDTH;
      const straight = port.rails.filter((r) => (left ? r.rail < item.gap : r.rail >= item.gap));
      const against = port.rails.filter((r) => !straight.includes(r));
      // How far the lead goes on its own line.
      let reach = px + dir * 6;
      if (port.ground) {
        reach = px + dir * 12;
        own.grounds.push({ x: reach, y: py });
      }
      let jog = null;
      if (against.length > 0) {
        jog = (left ? cellX - 7 : cellX + cellWidth + 7) + dir * 5 * jogs[port.side];
        jogs[port.side] += 1;
        reach = left ? Math.min(reach, jog) : Math.max(reach, jog);
      }
      for (const r of straight) {
        const x = railX(r.rail);
        reach = left ? Math.min(reach, x) : Math.max(reach, x);
        touch(r.rail, py);
        if (r.tag !== "") own.tags.push({ x: x - dir * 6, y: py - 3, text: r.tag, anchor: left ? "start" : "end" });
      }
      own.wires.push([[px, py], [reach, py]]);
      // A signal flows into a block from the left.
      if (face.kind === "controls" && left) own.arrows.push({ x: px, y: py });
      if (jog !== null) {
        const ey = bodyBottom + 5 + escape * M.escape;
        escape += 1;
        const xs = against.map((r) => railX(r.rail));
        const far = left ? Math.max(...xs) : Math.min(...xs);
        own.wires.push([[jog, py], [jog, ey], [far, ey]]);
        if (reach !== jog) own.dots.push({ x: jog, y: py, rail: null });
        for (const r of against) {
          const x = railX(r.rail);
          touch(r.rail, ey);
          if (r.tag !== "") own.tags.push({ x: x + dir * 6, y: ey - 3, text: r.tag, anchor: left ? "end" : "start" });
        }
      }
      if (port.label) {
        // Above its own wire, as long as that wire is: the whole name is
        // in the tooltip, and for an output above its rail.
        const chars = Math.max(9, Math.floor((Math.abs(reach - px) - 8) / 4.7));
        own.portLabels.push({ x: px + dir * 3, y: py - 3, text: shorten(port.label.split(".").pop(), chars), title: port.label, anchor: left ? "end" : "start" });
      }
    }
    // The values of a run below the symbol: a slot of terminal 0 or 1 on
    // the side of that terminal, any other in the middle.
    const sideOf = (terminal) => {
      const port = item.ports.find((p) => (p.terminal ?? "").replace(/^\w+/, "").startsWith(`[${terminal}]`));
      return port?.side ?? ((terminal === 0) !== item.flip ? "L" : "R");
    };
    const valuesTop = bodyBottom + plan.escapes;
    const values = [];
    plan.valueRows.forEach((row, k) => {
      for (const slot of row) {
        const side = slot.length === 2 ? sideOf(Number(slot[1])) : null;
        values.push({
          slot: `${item.id}|${slot}`,
          kind: slot === "state" ? "state" : slot[0],
          x: side === null ? cellX + cellWidth / 2 : side === "L" ? cellX - M.pad + 5 : cellX + cellWidth + M.pad - 5,
          y: valuesTop + (k + 1) * M.value - 2,
          anchor: side === null ? "middle" : side === "L" ? "start" : "end",
        });
      }
    });
    // The links below them.
    const chipsTop = valuesTop + plan.valueRows.length * M.value;
    // They begin below the symbol; long ones begin nearer to the rail.
    const widest = Math.max(0, ...plan.flow.placed.map((chip) => chip.dx + Math.max(...chip.lines.map((l) => l.length)) * 5.6));
    const chipsLeft = widest <= cellWidth + M.pad - 4 ? cellX : cellX - M.pad + 6;
    const placedChips = plan.flow.placed.map((chip) => ({
      x: chipsLeft + chip.dx,
      y: chipsTop + (chip.row + 1) * M.chip - 4,
      text: chip.lines.join(""),
      lines: chip.lines,
      title: chip.title,
      goto: chip.goto,
    }));
    items.push({
      ...item,
      x: cellX,
      y,
      width: cellWidth,
      height: plan.height - M.spacing,
      label: plan.labelLines.join(""),
      labelLines: plan.labelLines,
      labelX: cellX + cellWidth / 2,
      labelY: y + M.label - 4,
      symbol: { x: sx, y: line0, top: line0 - 12, width: SYMBOL_WIDTH, height: 26 + (item.tracks - 1) * TRACK },
      ...own,
      values,
      stateful: slots.get(item.id)?.has("state") ?? false,
      columns: slots.has(item.id),
      chips: placedChips,
    });
    for (const wire of own.wires) wires.push({ item: item.id, points: wire });
    dots.push(...own.dots);
    tags.push(...own.tags);
    grounds.push(...own.grounds);
    portLabels.push(...own.portLabels);
    chips.push(...placedChips);
  }

  const bottom = Math.max(top + 10, ...items.map((i) => i.y + i.height), ...railEnd);
  const rails = face.rails.map((rail, k) => {
    const head = heads[k];
    const railChips = head.flow.placed.map((chip) => ({
      x: railX(k) - M.column / 2 + 4 + chip.dx,
      y: headBase + 5 + headValue + (chip.row + 1) * M.chip - 4,
      text: chip.lines.join(""),
      lines: chip.lines,
      title: chip.title,
      goto: chip.goto,
    }));
    return {
      ...rail,
      x: railX(k),
      y1: top,
      y2: railEnd[k],
      label: head.lines.join(""),
      labelLines: head.lines,
      // The last line of the name is just above the values and the links.
      labelY: headBase - (head.lines.length - 1) * M.line,
      value: head.slot === null ? null : { slot: `${rail.id}|${head.slot}`, kind: head.slot, x: railX(k), y: headBase + M.value - 1, anchor: "middle" },
      columns: slots.has(rail.id),
      chips: railChips,
    };
  });
  const right = Math.max(
    railCount > 0 ? railX(railCount - 1) + M.column / 2 : 0,
    x0 + (usedTo + 1) * M.column,
    ...wires.flatMap((w) => w.points.map((p) => p[0] + 4)),
  );
  const placed = {
    kind: face.kind,
    reduced: face.reduced,
    note: face.note,
    width: Math.ceil(right + M.margin),
    height: Math.ceil(bottom + M.margin),
    rails,
    items,
    wires,
    dots,
    tags,
    grounds,
    portLabels,
  };
  const rawSidecar = sidecar ?? overrides;
  if (!rawSidecar) return placed;
  const resolved = typeof rawSidecar === "string" ? parseLayoutSidecar(rawSidecar).layout : rawSidecar;
  if (!resolved || typeof resolved !== "object") return placed;
  const faceSidecar = resolved.faces && typeof resolved.faces === "object" ? resolved.faces[face.kind] : resolved;
  if (!faceSidecar || typeof faceSidecar !== "object") return placed;
  const hasRails = faceSidecar.rails && typeof faceSidecar.rails === "object" && Object.keys(faceSidecar.rails).length > 0;
  const hasItems = faceSidecar.items && typeof faceSidecar.items === "object" && Object.keys(faceSidecar.items).length > 0;
  if (!hasRails && !hasItems) return placed;
  return applyLayoutOverrides(placed, faceSidecar, { measures: M });
}

// ---- checks of a placed face (for the tests and the build) ----

/// The wires and rails that run through the symbol of a component: none,
/// when the arrangement holds what it promises. A wire may end on the edge
/// of its own symbol.
export function wiresThroughSymbols(placed) {
  const found = [];
  const boxes = placed.items.map((item) => ({
    id: item.id,
    x1: item.symbol.x + 0.5,
    x2: item.symbol.x + item.symbol.width - 0.5,
    y1: item.symbol.top + 0.5,
    y2: item.symbol.top + item.symbol.height - 0.5,
  }));
  const crosses = (box, [ax, ay], [bx, by]) => {
    const x1 = Math.min(ax, bx);
    const x2 = Math.max(ax, bx);
    const y1 = Math.min(ay, by);
    const y2 = Math.max(ay, by);
    return x1 < box.x2 && x2 > box.x1 && y1 < box.y2 && y2 > box.y1;
  };
  for (const wire of placed.wires) {
    for (let k = 1; k < wire.points.length; k += 1) {
      for (const box of boxes) {
        if (crosses(box, wire.points[k - 1], wire.points[k])) found.push(`a wire of ${wire.item} runs through ${box.id}`);
      }
    }
  }
  for (const rail of placed.rails) {
    for (const box of boxes) {
      if (crosses(box, [rail.x, rail.y1], [rail.x, rail.y2])) found.push(`the rail ${rail.id} runs through ${box.id}`);
    }
  }
  // Two symbols on one another.
  const sorted = [...boxes].sort((a, b) => a.y1 - b.y1);
  for (let a = 0; a < sorted.length; a += 1) {
    for (let b = a + 1; b < sorted.length && sorted[b].y1 < sorted[a].y2; b += 1) {
      if (sorted[a].x1 < sorted[b].x2 && sorted[b].x1 < sorted[a].x2) found.push(`${sorted[a].id} lies on ${sorted[b].id}`);
    }
  }
  return found;
}

/// Where the diagram does not show what the document has: a component
/// without a symbol of its own, a node without a rail, a terminal whose
/// wire does not end on the rail of its node. Empty when they agree.
export function compareWithTopology(topology, circuit, controls) {
  const found = [];
  const placedCircuit = place(circuit);
  const drawn = new Map(placedCircuit.items.filter((i) => i.category === "circuit").map((i) => [i.name, i]));
  if (drawn.size !== topology.components.length) {
    found.push(`${topology.components.length} components, ${drawn.size} symbols`);
  }
  const railsWanted = topology.buses.length + topology.nodes.filter((node) => !node.ground && node.bus === undefined).length;
  if (placedCircuit.rails.length !== railsWanted) {
    found.push(`${railsWanted} buses and nodes, ${placedCircuit.rails.length} rails`);
  }
  for (const node of topology.nodes) {
    if (!node.ground && !circuit.nodeRail.has(node.name)) found.push(`the node ${node.name} has no rail`);
  }
  for (const c of topology.components) {
    const item = drawn.get(c.name);
    if (!item) {
      found.push(`${c.name} is not drawn`);
      continue;
    }
    c.terminals.forEach((terminal, k) => {
      const port = item.ports[k];
      const wanted = new Set(terminal.nodes.filter((node) => circuit.nodeRail.has(node)).map((node) => circuit.nodeRail.get(node).rail));
      const reached = new Set(port.rails.map((r) => r.rail));
      const same = wanted.size === reached.size && [...wanted].every((rail) => reached.has(rail));
      const grounded = terminal.nodes.some((node) => !circuit.nodeRail.has(node));
      if (!same || grounded !== port.ground) found.push(`${c.name}.${terminal.name} is not drawn at its nodes`);
      // Every rail of the port has a dot of this item on it.
      for (const rail of reached) {
        const x = placedCircuit.rails[rail].x;
        if (!item.dots.some((dot) => dot.rail === rail && dot.x === x)) found.push(`${c.name}.${terminal.name} does not reach ${placedCircuit.rails[rail].id}`);
      }
    });
  }
  const blocks = topology.controls.filter((c) => c.owner === undefined);
  const boxes = controls.items.filter((i) => i.category === "control");
  if (boxes.length !== blocks.length) found.push(`${blocks.length} blocks, ${boxes.length} boxes`);
  const owners = new Set(topology.controls.filter((c) => c.owner !== undefined).map((c) => c.owner));
  const ownerBoxes = controls.items.filter((i) => i.category === "owner");
  if (ownerBoxes.length !== owners.size) found.push(`${owners.size} components with blocks, ${ownerBoxes.length} boxes`);
  for (const block of blocks) {
    const item = controls.items.find((i) => i.id === `control:${block.name}`);
    if (!item) {
      found.push(`the block ${block.name} is not drawn`);
      continue;
    }
    const signals = [...block.inputs.map((i) => i.signal), ...block.outputs];
    item.ports.forEach((port, k) => {
      if (controls.rails[port.rails[0].rail].name !== signals[k]) found.push(`a port of ${block.name} is not at ${signals[k]}`);
    });
    if (item.ports.length !== signals.length) found.push(`${block.name} has ${signals.length} signals, ${item.ports.length} ports`);
  }
  return found;
}
