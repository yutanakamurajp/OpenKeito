// Browser-free accessibility and cross-browser tests for the diagram and
// charts: SVG focus attributes, keyboard navigation from the diagram frame
// and its items, overview coordinate fallback without getScreenCTM, and safe
// pointer-capture handling.
//
//   node --test web/tests/a11y.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";

import { Chart } from "../src/chart.js";
import { Diagram } from "../src/diagram.js";
import { circuitFace, controlsFace, place } from "../src/layout.js";
import { bindColumns, slotsOf, stateTimeline } from "../src/overlay.js";
import { toSvg } from "../src/render.js";

function unescapeXml(text) {
  return text
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

function camelCase(name) {
  return name.replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
}

function matchesOne(el, selector) {
  if (selector.startsWith(".")) {
    const parts = selector.slice(1).split(".");
    return parts.every((cls) => el.classList.contains(cls));
  }
  if (selector.startsWith("[") && selector.endsWith("]")) {
    const inner = selector.slice(1, -1);
    const eq = inner.indexOf("=");
    if (eq < 0) return el.hasAttribute(inner);
    const attr = inner.slice(0, eq);
    const val = inner.slice(eq + 1).replace(/^"|"$/g, "");
    return el.getAttribute(attr) === val;
  }
  return el.tagName === selector.toUpperCase();
}

function matchesSelector(el, selector) {
  return selector
    .split(",")
    .map((part) => part.trim())
    .some((part) => matchesOne(el, part));
}

function createMockDom() {
  const dom = { activeElement: null };

  class ClassList {
    constructor(el) {
      this.el = el;
    }
    _list() {
      const raw = this.el.getAttribute("class") ?? "";
      return raw.split(/\s+/).filter(Boolean);
    }
    _write(list) {
      this.el.setAttribute("class", list.join(" "));
    }
    add(...names) {
      const set = new Set(this._list());
      for (const name of names) if (name) set.add(name);
      this._write([...set]);
    }
    remove(...names) {
      const drop = new Set(names);
      this._write(this._list().filter((c) => !drop.has(c)));
    }
    toggle(name, force) {
      const has = this.contains(name);
      const want = force === undefined ? !has : Boolean(force);
      if (want) this.add(name);
      else this.remove(name);
      return want;
    }
    contains(name) {
      return this._list().includes(name);
    }
  }

  class MockElement {
    constructor(tag) {
      this.tagName = tag.toUpperCase();
      this.attributes = new Map();
      this.dataset = {};
      this.style = {};
      this.children = [];
      this.parentNode = null;
      this.listeners = new Map();
      this.classList = new ClassList(this);
      this._textContent = "";
      this.hidden = false;
      this.disabled = false;
      this.value = "";
      this.checked = false;
      this.clientWidth = 800;
      this.clientHeight = 600;
      this.scrollWidth = 800;
      this.scrollHeight = 600;
      this.scrollLeft = 0;
      this.scrollTop = 0;
      this.focusCalls = [];
      this.scrollIntoViewCalls = [];
      this.rejectPreventScroll = false;
    }

    get firstElementChild() {
      return this.children[0] ?? null;
    }

    get className() {
      return this.getAttribute("class") ?? "";
    }

    set className(value) {
      this.setAttribute("class", String(value));
    }

    get textContent() {
      if (this.children.length === 0) return this._textContent;
      return this._textContent + this.children.map((c) => c.textContent).join("");
    }

    set textContent(value) {
      this.children = [];
      this._textContent = String(value ?? "");
    }

    setAttribute(name, value) {
      const s = String(value);
      this.attributes.set(name, s);
      if (name.startsWith("data-")) this.dataset[camelCase(name.slice(5))] = s;
      if (name === "value") this.value = s;
    }

    getAttribute(name) {
      return this.attributes.get(name) ?? null;
    }

    hasAttribute(name) {
      return this.attributes.has(name);
    }

    append(...nodes) {
      for (const node of nodes) {
        if (typeof node === "string") {
          this._textContent += node;
        } else if (node) {
          node.parentNode = this;
          this.children.push(node);
        }
      }
    }

    replaceChildren(...nodes) {
      this.children = [];
      this._textContent = "";
      this.append(...nodes);
    }

    set innerHTML(markup) {
      this.children = [];
      this._textContent = "";
      const stack = [this];
      const tokenRe = /<(\/?)([a-zA-Z0-9:-]+)([^>]*?)(\/?)>|([^<]+)/g;
      let match;
      while ((match = tokenRe.exec(markup)) !== null) {
        if (match[5] !== undefined) {
          stack[stack.length - 1]._textContent += unescapeXml(match[5]);
          continue;
        }
        const [, closing, tag, rawAttrs, selfClosing] = match;
        if (closing) {
          if (stack.length > 1) stack.pop();
          continue;
        }
        const el = new MockElement(tag);
        const attrRe = /([a-zA-Z0-9:-]+)="([^"]*)"/g;
        let attr;
        while ((attr = attrRe.exec(rawAttrs)) !== null) {
          el.setAttribute(attr[1], unescapeXml(attr[2]));
        }
        stack[stack.length - 1].append(el);
        if (!selfClosing) stack.push(el);
      }
    }

    querySelectorAll(selector) {
      const out = [];
      const walk = (node) => {
        for (const child of node.children) {
          if (matchesSelector(child, selector)) out.push(child);
          walk(child);
        }
      };
      walk(this);
      return out;
    }

    querySelector(selector) {
      return this.querySelectorAll(selector)[0] ?? null;
    }

    closest(selector) {
      let cur = this;
      while (cur) {
        if (matchesSelector(cur, selector)) return cur;
        cur = cur.parentNode;
      }
      return null;
    }

    addEventListener(type, fn) {
      const list = this.listeners.get(type) ?? [];
      list.push(fn);
      this.listeners.set(type, list);
    }

    dispatchEvent(event) {
      const ev = {
        target: this,
        defaultPrevented: false,
        preventDefault() {
          this.defaultPrevented = true;
        },
        ...event,
      };
      let cur = ev.target;
      while (cur) {
        for (const fn of cur.listeners.get(ev.type) ?? []) fn(ev);
        if (ev.bubbles === false) break;
        cur = cur.parentNode;
      }
      return ev;
    }

    focus(options) {
      this.focusCalls.push(options);
      if (this.rejectPreventScroll && options && typeof options === "object") {
        throw new TypeError("focus options unsupported");
      }
      dom.activeElement = this;
      this.dispatchEvent({ type: "focusin", target: this, bubbles: true });
    }

    scrollIntoView(options) {
      this.scrollIntoViewCalls.push(options);
    }

    getContext() {
      return {
        setTransform() {},
        clearRect() {},
        beginPath() {},
        moveTo() {},
        lineTo() {},
        stroke() {},
        fill() {},
        rect() {},
        clip() {},
        save() {},
        restore() {},
        arc() {},
        fillText() {},
        fillRect() {},
        setLineDash() {},
        measureText: (t) => ({ width: t.length * 6 }),
      };
    }
  }

  dom.document = {
    documentElement: new MockElement("html"),
    get activeElement() {
      return dom.activeElement;
    },
    createElement: (tag) => new MockElement(tag),
  };
  return dom;
}

function sampleModel() {
  const topology = {
    components: [
      {
        name: "E1",
        type: "voltage_source",
        path: "components[0]",
        terminals: [
          { name: "nodes[0]", path: "components[0].nodes[0]", nodes: ["n1"] },
          { name: "nodes[1]", path: "components[0].nodes[1]", nodes: ["gnd"] },
        ],
      },
      {
        name: "S1",
        type: "controlled_switch",
        path: "components[1]",
        terminals: [
          { name: "nodes[0]", path: "components[1].nodes[0]", nodes: ["n1"] },
          { name: "nodes[1]", path: "components[1].nodes[1]", nodes: ["out"] },
        ],
        signals: [{ field: "gate", path: "components[1].gate", signal: "pwm" }],
      },
      {
        name: "R1",
        type: "resistor",
        path: "components[2]",
        terminals: [
          { name: "nodes[0]", path: "components[2].nodes[0]", nodes: ["out"] },
          { name: "nodes[1]", path: "components[2].nodes[1]", nodes: ["gnd"] },
        ],
      },
    ],
    nodes: [
      { name: "n1", components: ["E1", "S1"] },
      { name: "out", components: ["S1", "R1"] },
      { name: "gnd", ground: true, components: ["E1", "R1"] },
    ],
    buses: [],
    controls: [
      { name: "pwm", type: "constant", path: "controls[0]", inputs: [], outputs: ["pwm"] },
    ],
    signals: [
      { name: "pwm", from: "pwm", to: [{ component: "S1", field: "gate" }] },
    ],
    diagnostics: [],
  };
  const result = {
    model: { components: [{}, { initially_closed: false }, {}] },
    columns: [
      { name: "V(out)", kind: "node_voltage", unit: "V", target: "out" },
      { name: "I(R1)", kind: "branch_current", unit: "A", target: "R1" },
      { name: "S(pwm)", kind: "signal", unit: "", target: "pwm" },
    ],
    events: [
      { time_s: 0.002, component: "S1", action: "close" },
      { time_s: 0.005, component: "S1", action: "open" },
    ],
    table: {
      header: ["time_s", "V(out)", "I(R1)", "S(pwm)"],
      columns: [
        Float64Array.of(0, 0.001, 0.002, 0.003, 0.005, 0.006),
        Float64Array.of(0, 0, 0, 10, 10, 0),
        Float64Array.of(0, 0, 0, 1, 1, 0),
        Float64Array.of(0, 0, 1, 1, 0, 0),
      ],
      rows: 6,
    },
  };
  return { topology, result };
}

test("toSvg sets tabindex, role, and focusable attributes on interactive elements while preserving class contracts", () => {
  const { topology, result } = sampleModel();
  const binding = bindColumns(topology, result.columns, result.table.header);
  const timeline = stateTimeline(topology, result.model, result.events);
  const slots = slotsOf(binding, timeline);
  const placed = place(circuitFace(topology), { slots });
  const svg = toSvg(placed, "Circuit");

  assert.match(svg, /<g class="item circuit" tabindex="-1" role="button" focusable="true" data-item="component:E1"/);
  assert.match(svg, /<g class="chip" tabindex="-1" role="link" focusable="true" data-face="controls" data-signal="pwm"/);
  assert.match(svg, /class="rail node has-columns" data-rail="node:out" tabindex="-1" role="button" focusable="true"/);
  assert.ok(!/data-rail="node:n1"[^>]*tabindex/.test(svg));
  assert.ok(!/data-rail="node:n1"[^>]*focusable/.test(svg));
});

test("Diagram keyboard handler navigates from view container and between stops, handles zoom/row keys, and falls back when preventScroll throws", () => {
  const dom = createMockDom();
  const prevDocument = globalThis.document;
  globalThis.document = dom.document;
  try {
    const { topology, result } = sampleModel();
    const selectedPaths = [];
    const selectedColumns = [];
    const chosenRows = [];
    const root = dom.document.createElement("div");
    const diagram = new Diagram(root, {
      onSelect: (path) => selectedPaths.push(path),
      onColumns: (names) => selectedColumns.push(names),
      onRow: (row) => chosenRows.push(row),
    });
    diagram.setTopology(topology, "", result);

    assert.equal(diagram.view.getAttribute("tabindex"), "0");
    const stops = diagram.stops();
    assert.ok(stops.length >= 4);
    assert.equal(stops[0].getAttribute("tabindex"), "0");

    // 1. Focus on diagram.view itself: ArrowRight enters the current stop (stops[0]).
    diagram.view.focus();
    const enterFromView = diagram.view.dispatchEvent({ type: "keydown", key: "ArrowRight", target: diagram.view });
    assert.equal(enterFromView.defaultPrevented, true);
    assert.equal(dom.activeElement, stops[0]);
    assert.equal(stops[0].getAttribute("tabindex"), "0");

    // 2. Focus on diagram.view with an already-selected item: Enter moves focus to .selected stop.
    diagram.select("component:R1", { tell: false });
    const r1Stop = stops.find((el) => el.dataset.item === "component:R1");
    assert.ok(r1Stop);
    diagram.view.focus();
    diagram.view.dispatchEvent({ type: "keydown", key: "Enter", target: diagram.view });
    assert.equal(dom.activeElement, r1Stop);

    // 3. Focus on diagram.view with End / Home moves to last / first stop.
    diagram.view.focus();
    diagram.view.dispatchEvent({ type: "keydown", key: "End", target: diagram.view });
    assert.equal(dom.activeElement, stops[stops.length - 1]);
    diagram.view.focus();
    diagram.view.dispatchEvent({ type: "keydown", key: "Home", target: diagram.view });
    assert.equal(dom.activeElement, stops[0]);

    // 4. Arrow navigation from an item stop, including preventScroll fallback.
    stops[1].rejectPreventScroll = true;
    stops[0].dispatchEvent({ type: "keydown", key: "ArrowRight", target: stops[0] });
    assert.equal(dom.activeElement, stops[1]);
    assert.equal(stops[1].focusCalls.length, 2);
    assert.deepEqual(stops[1].focusCalls[0], { preventScroll: true });
    assert.equal(stops[1].focusCalls[1], undefined);

    stops[1].dispatchEvent({ type: "keydown", key: "ArrowLeft", target: stops[1] });
    assert.equal(dom.activeElement, stops[0]);
    stops[0].dispatchEvent({ type: "keydown", key: "End", target: stops[0] });
    assert.equal(dom.activeElement, stops[stops.length - 1]);
    stops[stops.length - 1].dispatchEvent({ type: "keydown", key: "Home", target: stops[stops.length - 1] });
    assert.equal(dom.activeElement, stops[0]);

    // 5. Enter / Space on an item activates it.
    r1Stop.dispatchEvent({ type: "keydown", key: "Enter", target: r1Stop });
    assert.equal(selectedPaths.at(-1), "components[2]");
    assert.deepEqual(selectedColumns.at(-1), ["I(R1)"]);

    // 6. Zoom and row-stepping shortcuts.
    const beforeZoom = diagram.scale;
    diagram.view.dispatchEvent({ type: "keydown", key: "+", target: r1Stop });
    assert.ok(diagram.scale > beforeZoom);
    diagram.view.dispatchEvent({ type: "keydown", key: "-", target: r1Stop });
    assert.ok(Math.abs(diagram.scale - beforeZoom) < 1e-9);
    diagram.view.dispatchEvent({ type: "keydown", key: "1", target: r1Stop });
    assert.equal(diagram.scale, 1);
    diagram.view.dispatchEvent({ type: "keydown", key: "0", target: r1Stop });
    assert.equal(diagram.fitted, true);

    diagram.view.dispatchEvent({ type: "keydown", key: ".", target: r1Stop });
    diagram.view.dispatchEvent({ type: "keydown", key: ".", target: r1Stop });
    diagram.view.dispatchEvent({ type: "keydown", key: ",", target: r1Stop });
    assert.deepEqual(chosenRows, [1, 2, 1]);

    // 7. Modifier shortcuts are left alone.
    const modEvent = diagram.view.dispatchEvent({ type: "keydown", key: "+", ctrlKey: true, target: r1Stop });
    assert.equal(modEvent.defaultPrevented, false);
  } finally {
    globalThis.document = prevDocument;
  }
});

test("scrollToOverview falls back to getBoundingClientRect when getScreenCTM is missing or singular", () => {
  const dom = createMockDom();
  const prevDocument = globalThis.document;
  const prevDOMPoint = globalThis.DOMPoint;
  globalThis.document = dom.document;
  globalThis.DOMPoint = class {
    constructor(x, y) {
      this.x = x;
      this.y = y;
    }
    matrixTransform(m) {
      return { x: m.a * this.x + m.c * this.y + m.e, y: m.b * this.x + m.d * this.y + m.f };
    }
  };
  try {
    const { topology } = sampleModel();
    const root = dom.document.createElement("div");
    const diagram = new Diagram(root, { onSelect: () => {} });
    diagram.setTopology(topology);
    diagram.scale = 2;
    diagram.view.clientWidth = 400;
    diagram.view.clientHeight = 200;

    const placed = diagram.placed[diagram.face];
    const svg = diagram.overview.firstElementChild;
    // Aspect-preserving box: 500x200 with left=10, top=20.
    svg.getBoundingClientRect = () => ({ left: 10, top: 20, width: 500, height: 200 });

    const k = Math.min(500 / placed.width, 200 / placed.height);
    const offsetX = 10 + (500 - placed.width * k) / 2;
    const offsetY = 20 + (200 - placed.height * k) / 2;
    const targetSvgX = placed.width * 0.75;
    const targetSvgY = placed.height * 0.25;
    const clientX = offsetX + targetSvgX * k;
    const clientY = offsetY + targetSvgY * k;

    // Case 1: getScreenCTM returns null (e.g. hidden/Firefox/Safari edge case).
    svg.getScreenCTM = () => null;
    diagram.scrollToOverview({ clientX, clientY });
    assert.ok(Math.abs(diagram.view.scrollLeft - (targetSvgX * 2 - 200)) < 1e-6);
    assert.ok(Math.abs(diagram.view.scrollTop - (targetSvgY * 2 - 100)) < 1e-6);

    // Case 2: getScreenCTM returns a singular matrix (det === 0).
    diagram.view.scrollLeft = 0;
    diagram.view.scrollTop = 0;
    svg.getScreenCTM = () => ({ a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, inverse: () => { throw new Error("singular"); } });
    diagram.scrollToOverview({ clientX, clientY });
    assert.ok(Math.abs(diagram.view.scrollLeft - (targetSvgX * 2 - 200)) < 1e-6);
    assert.ok(Math.abs(diagram.view.scrollTop - (targetSvgY * 2 - 100)) < 1e-6);

    // Case 3: overview pointer drag works without setPointerCapture and with touch pointermove (buttons === 0).
    diagram.view.scrollLeft = 0;
    diagram.overview.setPointerCapture = () => {
      throw new Error("InvalidPointerId");
    };
    diagram.overview.dispatchEvent({ type: "pointerdown", pointerId: 7, pointerType: "touch", button: 0, clientX, clientY });
    assert.ok(Math.abs(diagram.view.scrollLeft - (targetSvgX * 2 - 200)) < 1e-6);
    const nextClientX = offsetX + placed.width * 0.5 * k;
    diagram.overview.dispatchEvent({ type: "pointermove", pointerId: 7, pointerType: "touch", buttons: 0, clientX: nextClientX, clientY });
    assert.ok(Math.abs(diagram.view.scrollLeft - (placed.width * 0.5 * 2 - 200)) < 1e-6);
    diagram.overview.dispatchEvent({ type: "pointerup", pointerId: 7, pointerType: "touch" });
  } finally {
    globalThis.document = prevDocument;
    globalThis.DOMPoint = prevDOMPoint;
  }
});

test("Chart pointer handlers tolerate missing or throwing setPointerCapture", () => {
  const dom = createMockDom();
  const prevDocument = globalThis.document;
  const prevRaf = globalThis.requestAnimationFrame;
  globalThis.document = dom.document;
  globalThis.requestAnimationFrame = () => 1;
  try {
    const root = dom.document.createElement("div");
    const chart = new Chart(root);
    chart.setData({
      x: { data: Float64Array.of(0, 1, 2, 3), label: "t", unit: "s" },
      panels: [{ title: "V", unit: "V", series: [{ name: "V(n1)", data: Float64Array.of(0, 1, 0, -1) }] }],
    });
    const panel = chart.panels[0];
    panel.plot = { left: 58, top: 20, width: 200, height: 100, sx: 200 / 3 };
    panel.wrap.getBoundingClientRect = () => ({ left: 0, top: 0, width: 272, height: 144 });
    panel.wrap.setPointerCapture = () => {
      throw new Error("InvalidPointerId");
    };

    panel.wrap.dispatchEvent({ type: "pointerdown", pointerId: 1, button: 0, clientX: 60 });
    assert.equal(chart.drag?.mode, "zoom");
    panel.wrap.dispatchEvent({ type: "pointermove", pointerId: 1, clientX: 160 });
    panel.wrap.dispatchEvent({ type: "pointerup", pointerId: 1, clientX: 160 });
    assert.equal(chart.drag, null);
    assert.ok(chart.view[1] - chart.view[0] < 3);
  } finally {
    globalThis.document = prevDocument;
    globalThis.requestAnimationFrame = prevRaf;
  }
});
