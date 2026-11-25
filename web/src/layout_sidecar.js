// Sidecar layout overrides (`openkeito-layout` 0.1): schema validation,
// deterministic serialization, non-destructive merge onto placed faces, and
// minimal diff extraction.

import { SYMBOL_WIDTH, TRACK } from "./symbols.js";

export const LAYOUT_FORMAT = "openkeito-layout";
export const LAYOUT_VERSION = "0.1";

const FACES = ["circuit", "controls"];

const DEFAULT_MEASURES = {
  column: 124,
  pad: 20,
  half: 14,
  escape: 8,
  margin: 14,
};

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function cleanZero(value) {
  return value === 0 ? 0 : value;
}

function compareKeys(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (isPlainObject(value)) {
    const out = {};
    for (const key of Object.keys(value).sort(compareKeys)) {
      const child = value[key];
      if (child !== undefined) out[key] = sortKeys(child);
    }
    return out;
  }
  return typeof value === "number" ? cleanZero(value) : value;
}

function shorten(text, max) {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function emptyFace() {
  return { rails: {}, items: {} };
}

function emptyLayout() {
  return {
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: {
      circuit: emptyFace(),
      controls: emptyFace(),
    },
  };
}

function validateNumberField(obj, field, path, errors) {
  if (!Object.hasOwn(obj, field) || obj[field] === undefined) return 0;
  const v = obj[field];
  if (typeof v !== "number" || !Number.isFinite(v)) {
    errors.push(`${path}.${field} must be a finite number`);
    return null;
  }
  return cleanZero(v);
}

function parseFace(rawFace, faceName, errors, warnings) {
  if (rawFace === undefined) return emptyFace();
  if (!isPlainObject(rawFace)) {
    errors.push(`faces.${faceName} must be an object`);
    return emptyFace();
  }
  for (const key of Object.keys(rawFace)) {
    if (key !== "rails" && key !== "items") {
      warnings.push(`Unknown property faces.${faceName}.${key} ignored`);
    }
  }
  const rails = {};
  if (rawFace.rails !== undefined) {
    if (!isPlainObject(rawFace.rails)) {
      errors.push(`faces.${faceName}.rails must be an object`);
    } else {
      for (const id of Object.keys(rawFace.rails).sort(compareKeys)) {
        const entry = rawFace.rails[id];
        const path = `faces.${faceName}.rails[${JSON.stringify(id)}]`;
        if (!isPlainObject(entry)) {
          errors.push(`${path} must be an object`);
          continue;
        }
        for (const key of Object.keys(entry)) {
          if (key !== "dx") warnings.push(`Unknown property ${path}.${key} ignored`);
        }
        const dx = validateNumberField(entry, "dx", path, errors);
        if (dx !== null) rails[id] = { dx };
      }
    }
  }
  const items = {};
  if (rawFace.items !== undefined) {
    if (!isPlainObject(rawFace.items)) {
      errors.push(`faces.${faceName}.items must be an object`);
    } else {
      for (const id of Object.keys(rawFace.items).sort(compareKeys)) {
        const entry = rawFace.items[id];
        const path = `faces.${faceName}.items[${JSON.stringify(id)}]`;
        if (!isPlainObject(entry)) {
          errors.push(`${path} must be an object`);
          continue;
        }
        for (const key of Object.keys(entry)) {
          if (key !== "dx" && key !== "dy") warnings.push(`Unknown property ${path}.${key} ignored`);
        }
        const dx = validateNumberField(entry, "dx", path, errors);
        const dy = validateNumberField(entry, "dy", path, errors);
        if (dx !== null && dy !== null) items[id] = { dx, dy };
      }
    }
  }
  return { rails, items };
}

/// Validates and normalizes a layout sidecar JSON string or object.
/// Returns `{ valid, errors, warnings, layout }`.
export function parseLayoutSidecar(input) {
  const errors = [];
  const warnings = [];
  let raw = input;
  if (typeof input === "string") {
    try {
      raw = JSON.parse(input);
    } catch (err) {
      return {
        valid: false,
        errors: [`Invalid JSON: ${err.message}`],
        warnings: [],
        layout: null,
      };
    }
  }
  if (!isPlainObject(raw)) {
    return {
      valid: false,
      errors: ["Layout sidecar root must be an object"],
      warnings: [],
      layout: null,
    };
  }
  if (raw.format !== LAYOUT_FORMAT) {
    errors.push(
      raw.format === undefined
        ? `Missing required field "format" (expected "${LAYOUT_FORMAT}")`
        : `Unsupported format ${JSON.stringify(raw.format)} (expected "${LAYOUT_FORMAT}")`,
    );
  }
  if (raw.version !== LAYOUT_VERSION) {
    errors.push(
      raw.version === undefined
        ? `Missing required field "version" (expected "${LAYOUT_VERSION}")`
        : `Unsupported version ${JSON.stringify(raw.version)} (expected "${LAYOUT_VERSION}")`,
    );
  }
  for (const key of Object.keys(raw)) {
    if (key !== "format" && key !== "version" && key !== "faces") {
      warnings.push(`Unknown property ${key} ignored`);
    }
  }

  const faces = { circuit: emptyFace(), controls: emptyFace() };
  if (raw.faces !== undefined) {
    if (!isPlainObject(raw.faces)) {
      errors.push("faces must be an object");
    } else {
      for (const key of Object.keys(raw.faces)) {
        if (!FACES.includes(key)) {
          warnings.push(`Unknown face faces.${key} ignored`);
        }
      }
      for (const faceName of FACES) {
        faces[faceName] = parseFace(raw.faces[faceName], faceName, errors, warnings);
      }
    }
  }

  if (errors.length > 0) {
    return { valid: false, errors, warnings, layout: null };
  }
  return {
    valid: true,
    errors: [],
    warnings,
    layout: {
      format: LAYOUT_FORMAT,
      version: LAYOUT_VERSION,
      faces,
    },
  };
}

function assertFiniteNumbers(value, path) {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new RangeError(`${path} must be a finite number`);
    }
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertFiniteNumbers(item, `${path}[${index}]`));
    return;
  }
  if (isPlainObject(value)) {
    for (const key of Object.keys(value)) {
      assertFiniteNumbers(value[key], `${path}.${key}`);
    }
  }
}

/// Serializes a layout sidecar object as deterministically key-sorted JSON
/// with a trailing newline.
export function serializeLayoutSidecar(layout) {
  if (!isPlainObject(layout)) {
    throw new TypeError("layout must be an object");
  }
  assertFiniteNumbers(layout, "layout");
  const base = emptyLayout();
  const normalized = {
    ...layout,
    format: layout.format ?? base.format,
    version: layout.version ?? base.version,
    faces: {
      ...base.faces,
      ...(isPlainObject(layout.faces) ? layout.faces : {}),
    },
  };
  for (const faceName of FACES) {
    const face = normalized.faces[faceName];
    normalized.faces[faceName] = isPlainObject(face)
      ? {
          ...face,
          rails: isPlainObject(face.rails) ? face.rails : {},
          items: isPlainObject(face.items) ? face.items : {},
        }
      : emptyFace();
  }
  return `${JSON.stringify(sortKeys(normalized), null, 2)}\n`;
}

function resolveFaceOverrides(placed, faceSidecar) {
  if (!isPlainObject(faceSidecar)) return { rails: {}, items: {} };
  if (isPlainObject(faceSidecar.faces)) {
    const face = faceSidecar.faces[placed.kind];
    return isPlainObject(face) ? face : { rails: {}, items: {} };
  }
  return faceSidecar;
}

function finiteOrZero(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function countDetours(ports, railX, symbolX, isLeftPredicate) {
  let detours = 0;
  for (const port of ports) {
    const left = port.side === "L";
    const px = left ? symbolX : symbolX + SYMBOL_WIDTH;
    const hasAgainst = port.rails.some((r) => !isLeftPredicate(left, railX[r.rail], px));
    if (hasAgainst) detours += 1;
  }
  return detours;
}

/// Non-destructively applies `faceSidecar` (`rails[id].dx`, `items[id].dx/dy`)
/// to a `placed` face returned by `place()`, keeping all wires, dots,
/// grounds, tags, port labels, arrows, values, chips, rail spans, and canvas
/// bounds consistent.
export function applyLayoutOverrides(placed, faceSidecar, { measures = DEFAULT_MEASURES } = {}) {
  const M = measures;
  const overrides = resolveFaceOverrides(placed, faceSidecar);
  const railMap = isPlainObject(overrides.rails) ? overrides.rails : {};
  const itemMap = isPlainObject(overrides.items) ? overrides.items : {};

  const oldRailX = placed.rails.map((rail) => rail.x);
  const newRailX = placed.rails.map((rail) => {
    const entry = railMap[rail.id];
    const dx = isPlainObject(entry) ? finiteOrZero(entry.dx) : 0;
    return rail.x + dx;
  });

  const isOnPortSide = (left, rx, px) => (left ? rx <= px : rx >= px);

  const wires = [];
  const dots = [];
  const tags = [];
  const grounds = [];
  const portLabels = [];

  const items = placed.items.map((item) => {
    const entry = itemMap[item.id];
    const idx = isPlainObject(entry) ? finiteOrZero(entry.dx) : 0;
    const idy = isPlainObject(entry) ? finiteOrZero(entry.dy) : 0;

    const x = item.x + idx;
    const y = item.y + idy;
    const sx = item.symbol.x + idx;
    const line0 = item.symbol.y + idy;
    const bodyBottom = line0 + (item.tracks - 1) * TRACK + M.half;

    const oldDetours = countDetours(item.ports, oldRailX, item.symbol.x, isOnPortSide);
    const newDetours = countDetours(item.ports, newRailX, sx, isOnPortSide);
    const oldEscapes = oldDetours > 0 ? 5 + (oldDetours - 1) * M.escape + 3 : 0;
    const newEscapes = newDetours > 0 ? 5 + (newDetours - 1) * M.escape + 3 : 0;
    const escapeDelta = newEscapes - oldEscapes;

    const own = { wires: [], dots: [], tags: [], grounds: [], portLabels: [], arrows: [] };
    let escape = 0;
    const jogs = { L: 0, R: 0 };

    for (const port of item.ports) {
      const left = port.side === "L";
      const dir = left ? -1 : 1;
      const py = line0 + port.track * TRACK;
      const px = left ? sx : sx + SYMBOL_WIDTH;
      const straight = port.rails.filter((r) => isOnPortSide(left, newRailX[r.rail], px));
      const against = port.rails.filter((r) => !straight.includes(r));

      let reach = px + dir * 6;
      if (port.ground) {
        reach = px + dir * 12;
        own.grounds.push({ x: reach, y: py });
      }
      let jog = null;
      if (against.length > 0) {
        jog = (left ? x - 7 : x + item.width + 7) + dir * 5 * jogs[port.side];
        jogs[port.side] += 1;
        reach = left ? Math.min(reach, jog) : Math.max(reach, jog);
      }
      for (const r of straight) {
        const rx = newRailX[r.rail];
        reach = left ? Math.min(reach, rx) : Math.max(reach, rx);
        own.dots.push({ x: rx, y: py, rail: r.rail });
        if (r.tag !== "") {
          own.tags.push({ x: rx - dir * 6, y: py - 3, text: r.tag, anchor: left ? "start" : "end" });
        }
      }
      own.wires.push([[px, py], [reach, py]]);
      if (placed.kind === "controls" && left) own.arrows.push({ x: px, y: py });

      if (jog !== null) {
        const ey = bodyBottom + 5 + escape * M.escape;
        escape += 1;
        const xs = against.map((r) => newRailX[r.rail]);
        const far = left ? Math.max(...xs) : Math.min(...xs);
        own.wires.push([[jog, py], [jog, ey], [far, ey]]);
        if (reach !== jog) own.dots.push({ x: jog, y: py, rail: null });
        for (const r of against) {
          const rx = newRailX[r.rail];
          own.dots.push({ x: rx, y: ey, rail: r.rail });
          if (r.tag !== "") {
            own.tags.push({ x: rx + dir * 6, y: ey - 3, text: r.tag, anchor: left ? "end" : "start" });
          }
        }
      }
      if (port.label) {
        const chars = Math.max(9, Math.floor((Math.abs(reach - px) - 8) / 4.7));
        own.portLabels.push({
          x: px + dir * 3,
          y: py - 3,
          text: shorten(port.label.split(".").pop(), chars),
          title: port.label,
          anchor: left ? "end" : "start",
        });
      }
    }

    for (const wire of own.wires) wires.push({ item: item.id, points: wire });
    dots.push(...own.dots);
    tags.push(...own.tags);
    grounds.push(...own.grounds);
    portLabels.push(...own.portLabels);

    return {
      ...item,
      x,
      y,
      height: item.height + escapeDelta,
      labelLines: item.labelLines ? [...item.labelLines] : undefined,
      labelX: item.labelX + idx,
      labelY: item.labelY + idy,
      symbol: {
        ...item.symbol,
        x: sx,
        y: line0,
        top: item.symbol.top + idy,
      },
      ports: item.ports.map((p) => ({ ...p, rails: p.rails.map((r) => ({ ...r })) })),
      ...own,
      values: (item.values ?? []).map((v) => ({ ...v, x: v.x + idx, y: v.y + idy + escapeDelta })),
      chips: (item.chips ?? []).map((c) => ({ ...c, x: c.x + idx, y: c.y + idy + escapeDelta })),
    };
  });

  const railMinY = new Map();
  const railMaxY = new Map();
  for (const dot of dots) {
    if (dot.rail === null || dot.rail === undefined) continue;
    railMinY.set(dot.rail, railMinY.has(dot.rail) ? Math.min(railMinY.get(dot.rail), dot.y) : dot.y);
    railMaxY.set(dot.rail, railMaxY.has(dot.rail) ? Math.max(railMaxY.get(dot.rail), dot.y) : dot.y);
  }

  const rails = placed.rails.map((rail, k) => {
    const dx = newRailX[k] - rail.x;
    const minY = railMinY.get(k);
    const maxY = railMaxY.get(k);
    const y1 = minY !== undefined ? Math.min(rail.y1, minY) : rail.y1;
    const y2 = maxY !== undefined ? Math.max(rail.y1 + 10, maxY) : rail.y1 + 10;
    return {
      ...rail,
      x: newRailX[k],
      y1,
      y2,
      labelLines: rail.labelLines ? [...rail.labelLines] : undefined,
      value: rail.value ? { ...rail.value, x: rail.value.x + dx } : null,
      chips: (rail.chips ?? []).map((c) => ({ ...c, x: c.x + dx })),
    };
  });

  let width = placed.width;
  let height = placed.height;
  if (rails.length > 0 || items.length > 0) {
    const right = Math.max(
      0,
      ...rails.map((r) => r.x + M.column / 2),
      ...items.map((i) => i.x + i.width + M.pad),
      ...wires.flatMap((w) => w.points.map((p) => p[0] + 4)),
    );
    const baseTop = rails.length > 0 ? Math.min(...placed.rails.map((r) => r.y1)) : 0;
    const bottom = Math.max(
      rails.length > 0 ? baseTop + 10 : placed.height - M.margin,
      ...items.map((i) => i.y + i.height),
      ...rails.map((r) => r.y2),
    );
    width = Math.ceil(right + M.margin);
    height = Math.ceil(bottom + M.margin);
  }

  return {
    ...placed,
    width,
    height,
    rails,
    items,
    wires,
    dots,
    tags,
    grounds,
    portLabels,
  };
}

function diffFace(placed, baselinePlaced) {
  const baseRails = new Map((baselinePlaced?.rails ?? []).map((r) => [r.id, r]));
  const baseItems = new Map((baselinePlaced?.items ?? []).map((i) => [i.id, i]));

  const rails = {};
  for (const rail of [...(placed?.rails ?? [])].sort((a, b) => compareKeys(a.id, b.id))) {
    const base = baseRails.get(rail.id);
    if (!base) continue;
    const dx = rail.x - base.x;
    if (Number.isFinite(dx) && Math.abs(dx) > 1e-9) {
      rails[rail.id] = { dx: cleanZero(dx) };
    }
  }

  const items = {};
  for (const item of [...(placed?.items ?? [])].sort((a, b) => compareKeys(a.id, b.id))) {
    const base = baseItems.get(item.id);
    if (!base) continue;
    const dx = item.x - base.x;
    const dy = item.y - base.y;
    const hasDx = Number.isFinite(dx) && Math.abs(dx) > 1e-9;
    const hasDy = Number.isFinite(dy) && Math.abs(dy) > 1e-9;
    if (hasDx || hasDy) {
      items[item.id] = {
        dx: hasDx ? cleanZero(dx) : 0,
        dy: hasDy ? cleanZero(dy) : 0,
      };
    }
  }
  return { rails, items };
}

/// Extracts a minimal, deterministic sidecar object from the coordinate
/// differences between `placed` and `baselinePlaced` (accepts either a
/// single placed face or a `{ circuit, controls }` map).
export function extractLayoutOverrides(placed, baselinePlaced) {
  const out = emptyLayout();
  if (isPlainObject(placed) && Array.isArray(placed.rails) && Array.isArray(placed.items)) {
    const faceName = FACES.includes(placed.kind)
      ? placed.kind
      : FACES.includes(baselinePlaced?.kind)
        ? baselinePlaced.kind
        : "circuit";
    out.faces[faceName] = diffFace(placed, baselinePlaced);
    return out;
  }
  if (isPlainObject(placed)) {
    for (const faceName of FACES) {
      if (placed[faceName] && baselinePlaced?.[faceName]) {
        out.faces[faceName] = diffFace(placed[faceName], baselinePlaced[faceName]);
      }
    }
  }
  return out;
}
