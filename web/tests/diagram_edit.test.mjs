// Browser-free tests for InspectorPanel and Diagram layout editing / sidecar
// persistence, using a lightweight DOM mock and openkeito-wasm.
//
//   node --test web/tests/diagram_edit.test.mjs

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

import { Diagram } from "../src/diagram.js";
import { InspectorPanel } from "../src/inspector_panel.js";
import { LAYOUT_FORMAT, LAYOUT_VERSION, parseLayoutSidecar } from "../src/layout_sidecar.js";

const root = new URL("../../", import.meta.url);
const pkg = new URL("crates/openkeito-wasm/pkg/", root);
const rlcText = readFileSync(new URL("examples/rlc.json", root), "utf8");

let loadedWasm = null;
async function loadWasm() {
  assert.ok(
    existsSync(new URL("openkeito_wasm_bg.wasm", pkg)),
    "the WebAssembly module is not built: see crates/openkeito-wasm/README.md, \"Build\"",
  );
  if (!loadedWasm) {
    loadedWasm = await import(new URL("openkeito_wasm.js", pkg).href);
    loadedWasm.initSync({ module: readFileSync(new URL("openkeito_wasm_bg.wasm", pkg)) });
  }
  return loadedWasm;
}

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
  let rest = selector.trim();
  if (!rest) return false;
  let tag = null;
  const tagMatch = /^([a-zA-Z0-9_-]+)/.exec(rest);
  if (tagMatch) {
    tag = tagMatch[1].toUpperCase();
    rest = rest.slice(tagMatch[0].length);
  }
  if (tag && el.tagName !== tag) return false;
  while (rest.length > 0) {
    if (rest.startsWith(".")) {
      const clsMatch = /^\.([a-zA-Z0-9_-]+)/.exec(rest);
      if (!clsMatch) return false;
      if (!el.classList.contains(clsMatch[1])) return false;
      rest = rest.slice(clsMatch[0].length);
    } else if (rest.startsWith("[")) {
      const end = rest.indexOf("]");
      if (end < 0) return false;
      const inner = rest.slice(1, end);
      const eq = inner.indexOf("=");
      if (eq < 0) {
        if (!el.hasAttribute(inner)) return false;
      } else {
        const attr = inner.slice(0, eq);
        const val = inner.slice(eq + 1).replace(/^"|"$/g, "");
        if (el.getAttribute(attr) !== val) return false;
      }
      rest = rest.slice(end + 1);
    } else {
      return false;
    }
  }
  return true;
}

function matchesSelector(el, selector) {
  return selector
    .split(",")
    .map((part) => part.trim())
    .some((part) => {
      const chain = part.split(/\s+/).filter(Boolean);
      if (chain.length === 0) return false;
      if (!matchesOne(el, chain[chain.length - 1])) return false;
      let cur = el.parentNode;
      for (let i = chain.length - 2; i >= 0; i -= 1) {
        while (cur && !matchesOne(cur, chain[i])) cur = cur.parentNode;
        if (!cur) return false;
        cur = cur.parentNode;
      }
      return true;
    });
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
      this.files = [];
      this.clientWidth = 800;
      this.clientHeight = 600;
      this.scrollWidth = 800;
      this.scrollHeight = 600;
      this.scrollLeft = 0;
      this.scrollTop = 0;
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

    removeAttribute(name) {
      this.attributes.delete(name);
      if (name.startsWith("data-")) delete this.dataset[camelCase(name.slice(5))];
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

    click() {
      this.dispatchEvent({ type: "click", target: this, bubbles: true });
    }

    focus() {
      dom.activeElement = this;
      this.dispatchEvent({ type: "focusin", target: this, bubbles: true });
    }

    scrollIntoView() {}
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

function withMockDom(fn) {
  const dom = createMockDom();
  const prevDocument = globalThis.document;
  globalThis.document = dom.document;
  return Promise.resolve()
    .then(() => fn(dom))
    .finally(() => {
      globalThis.document = prevDocument;
    });
}

test("InspectorPanel loads all 58 catalog types, edits rlc.json fields, and emits valid JSON", async () => {
  const wasm = await loadWasm();
  const catalog = JSON.parse(wasm.components());
  const schema = JSON.parse(wasm.schema());
  assert.equal(catalog.length, 58);

  await withMockDom(async (dom) => {
    const root = dom.document.createElement("div");
    const changes = [];
    const selectedPaths = [];
    const panel = new InspectorPanel(root, {
      onChange: (text) => changes.push(text),
      onSelectPath: (path) => selectedPaths.push(path),
    });

    panel.setCatalog(catalog, schema);
    panel.setModelText(rlcText);

    // Header selects contain all 26 circuit components and 32 control blocks.
    assert.equal(panel.addComponentSelect.querySelectorAll("option").length, 26);
    assert.equal(panel.addControlSelect.querySelectorAll("option").length, 32);

    // Simulation fields rendered with units and enums.
    const simRow = root.querySelector('[data-field="time_step_s"]');
    assert.ok(simRow);
    assert.ok(simRow.textContent.includes("(s)"));
    assert.ok(simRow.textContent.includes("*"));
    const methodSelect = root.querySelector('[data-field="method"] select');
    assert.ok(methodSelect);
    assert.equal(methodSelect.value, "2s-dirk");

    // Select R1 (components[1]) and change resistance_ohm.
    panel.setSelection("components[1]");
    const resRow = root.querySelector('[data-field="resistance_ohm"]');
    assert.ok(resRow);
    assert.ok(resRow.textContent.includes("(ohm)"));
    assert.ok(resRow.getAttribute("title").length > 0);
    const resInput = resRow.querySelector("input");
    assert.equal(resInput.value, "10");

    resInput.value = "25.5";
    resInput.dispatchEvent({ type: "change", target: resInput });

    assert.equal(changes.length, 1);
    const updatedModel = JSON.parse(changes[0]);
    assert.equal(updatedModel.components[1].resistance_ohm, 25.5);
    assert.equal(JSON.parse(wasm.validate(changes[0])).valid, true);

    // Node datalist candidates include existing nodes + gnd, and editing nodes updates connections.
    const nodeOptions = [...panel.nodesList.querySelectorAll("option")].map((o) => o.value);
    assert.ok(nodeOptions.includes("gnd"));
    assert.ok(nodeOptions.includes("n1"));
    assert.ok(nodeOptions.includes("n3"));
    const node0Input = root.querySelector('[data-field="nodes"] input[data-array-index="0"]');
    assert.ok(node0Input);
    node0Input.value = "n_mid";
    node0Input.dispatchEvent({ type: "change", target: node0Input });
    assert.equal(JSON.parse(changes.at(-1)).components[1].nodes[0], "n_mid");
    node0Input.value = "n1";
    node0Input.dispatchEvent({ type: "change", target: node0Input });

    // Target selector invokes onSelectPath when changed by user.
    panel.targetSelect.value = "components[0]";
    panel.targetSelect.dispatchEvent({ type: "change", target: panel.targetSelect });
    assert.equal(selectedPaths.at(-1), "components[0]");

    // Select E1 (components[0]) and edit compound waveform object via JSON textarea.
    panel.setSelection("components[0]");
    const waveInput = root.querySelector('[data-field="waveform"] textarea');
    assert.ok(waveInput);
    waveInput.value = JSON.stringify({ type: "dc", value: 5.0 });
    waveInput.dispatchEvent({ type: "change", target: waveInput });
    assert.equal(JSON.parse(changes.at(-1)).components[0].waveform.value, 5.0);
    assert.equal(JSON.parse(wasm.validate(changes.at(-1))).valid, true);

    // Outputs section: list, add, and remove.
    panel.setSelection("outputs");
    assert.equal(root.querySelectorAll(".inspector-output-item").length, 2);
    const outTypeSelect = root.querySelector(".inspector-output-type");
    outTypeSelect.value = "node_voltage";
    outTypeSelect.dispatchEvent({ type: "change", target: outTypeSelect });
    const outNodeInput = root.querySelector('[data-output-field="node"] input');
    outNodeInput.value = "n1";
    outNodeInput.dispatchEvent({ type: "input", target: outNodeInput });
    root.querySelector(".inspector-output-add-button").click();
    assert.equal(JSON.parse(changes.at(-1)).outputs.length, 3);
    assert.equal(JSON.parse(wasm.validate(changes.at(-1))).valid, true);
    root.querySelector(".inspector-output-remove").click();
    assert.equal(JSON.parse(changes.at(-1)).outputs.length, 2);

    // Add a control block and verify simulation diagnostic mapping (E110).
    panel.addControlSelect.value = "constant";
    panel.addControlButton.click();
    assert.equal(JSON.parse(changes.at(-1)).controls.length, 1);
    assert.equal(panel.selection, "controls[0]");
    panel.setSelection("simulation");
    const dtInput = root.querySelector('[data-field="time_step_s"] input');
    dtInput.value = "-1e-6";
    dtInput.dispatchEvent({ type: "change", target: dtInput });
    const simReport = JSON.parse(wasm.validate(changes.at(-1)));
    assert.equal(simReport.valid, false);
    panel.setDiagnostics(simReport.errors, simReport.warnings);
    const dtField = root.querySelector('[data-field="time_step_s"]');
    assert.equal(dtField.classList.contains("has-error"), true);
    assert.ok(dtField.querySelector(".inspector-diagnostic").textContent.includes("E110"));
  });
});

test("InspectorPanel supports add/delete, undo/redo, syntax error recovery, and field diagnostics (E130)", async () => {
  const wasm = await loadWasm();
  const catalog = JSON.parse(wasm.components());
  const schema = JSON.parse(wasm.schema());

  await withMockDom(async (dom) => {
    const root = dom.document.createElement("div");
    const changes = [];
    const panel = new InspectorPanel(root, {
      onChange: (text) => changes.push(text),
    });
    panel.setCatalog(catalog, schema);
    panel.setModelText(rlcText);

    assert.equal(panel.canUndo(), false);
    assert.equal(panel.canRedo(), false);

    // Add a resistor via header bar.
    panel.addComponentSelect.value = "resistor";
    panel.addComponentButton.click();
    assert.equal(panel.canUndo(), true);
    let current = JSON.parse(changes.at(-1));
    assert.equal(current.components.length, 5);
    assert.equal(current.components[4].name, "R2");
    assert.equal(panel.selection, "components[4]");

    // Undo and Redo restore the component list.
    panel.undo();
    assert.equal(JSON.parse(changes.at(-1)).components.length, 4);
    assert.equal(panel.canUndo(), false);
    assert.equal(panel.canRedo(), true);

    panel.redo();
    assert.equal(JSON.parse(changes.at(-1)).components.length, 5);
    assert.equal(panel.canRedo(), false);

    // Delete the newly added resistor.
    panel.setSelection("components[4]");
    panel.deleteButton.click();
    assert.equal(JSON.parse(changes.at(-1)).components.length, 4);
    assert.equal(JSON.parse(wasm.validate(changes.at(-1))).valid, true);

    // Trigger E130 (negative resistance) and verify diagnostic message + hint render beside the field.
    panel.setSelection("components[1]");
    const resInput = root.querySelector('[data-field="resistance_ohm"] input');
    resInput.value = "-5";
    resInput.dispatchEvent({ type: "change", target: resInput });

    const invalidText = changes.at(-1);
    const report = JSON.parse(wasm.validate(invalidText));
    assert.equal(report.valid, false);
    const e130 = report.errors.find((e) => e.code === "E130");
    assert.ok(e130);

    panel.setDiagnostics(report.errors, report.warnings);
    const resField = root.querySelector('[data-field="resistance_ohm"]');
    assert.equal(resField.classList.contains("has-error"), true);
    const diagBox = resField.querySelector(".inspector-diagnostic");
    assert.ok(diagBox);
    assert.ok(diagBox.textContent.includes("E130"));
    assert.ok(diagBox.textContent.includes(e130.message));
    assert.ok(diagBox.textContent.includes(e130.hint));

    // Unparseable JSON disables structured form, then recovers once fixed.
    panel.setModelText("{ broken json");
    assert.ok(root.querySelector(".inspector-unparseable"));
    assert.equal(panel.deleteButton.disabled, true);
    panel.setModelText(rlcText);
    assert.equal(root.querySelector(".inspector-unparseable"), null);
    assert.ok(root.querySelector('[data-field="resistance_ohm"]'));
  });
});

test("Diagram supports setSidecar, drag editing with 3px threshold, export/import, and resetSidecar", async () => {
  const wasm = await loadWasm();
  const topology = JSON.parse(wasm.topology(rlcText));

  await withMockDom(async (dom) => {
    const root = dom.document.createElement("div");
    const selectedPaths = [];
    const sidecarChanges = [];
    const diagram = new Diagram(root, {
      onSelect: (path) => selectedPaths.push(path),
      onSidecarChange: (sc) => sidecarChanges.push(sc),
    });
    diagram.setTopology(topology);

    assert.equal(diagram.editLayout, false);
    assert.equal(diagram.getSidecar(), null);
    assert.ok(diagram.baselinePlaced?.circuit);

    const baseR1 = diagram.baselinePlaced.circuit.items.find((i) => i.id === "component:R1");
    const baseN2 = diagram.baselinePlaced.circuit.rails.find((r) => r.id === "node:n2");
    assert.ok(baseR1 && baseN2);

    // Toggle layout edit mode via Move button.
    assert.equal(diagram.moveButton.getAttribute("aria-pressed"), "false");
    diagram.moveButton.click();
    assert.equal(diagram.editLayout, true);
    assert.equal(diagram.moveButton.getAttribute("aria-pressed"), "true");

    // 1. Sub-threshold pointer movement (< 3px) does NOT modify sidecar and allows click selection.
    diagram.scale = 1;
    let r1El = diagram.element.querySelector('[data-item="component:R1"]');
    r1El.dispatchEvent({ type: "pointerdown", pointerId: 1, button: 0, clientX: 100, clientY: 100, target: r1El });
    r1El.dispatchEvent({ type: "pointermove", pointerId: 1, clientX: 101, clientY: 101, target: r1El });
    r1El.dispatchEvent({ type: "pointerup", pointerId: 1, clientX: 101, clientY: 101, target: r1El });
    assert.equal(diagram.getSidecar(), null);
    assert.equal(sidecarChanges.length, 0);
    r1El.click();
    assert.deepEqual(selectedPaths, ["components[1]"]);

    // 2. Drag >= 3px at scale=2 moves R1 and suppresses the trailing click.
    diagram.scale = 2;
    r1El = diagram.element.querySelector('[data-item="component:R1"]');
    r1El.dispatchEvent({ type: "pointerdown", pointerId: 2, button: 0, clientX: 100, clientY: 100, target: r1El });
    r1El.dispatchEvent({ type: "pointermove", pointerId: 2, clientX: 140, clientY: 180, target: r1El });
    r1El.dispatchEvent({ type: "pointerup", pointerId: 2, clientX: 140, clientY: 180, target: r1El });
    r1El.click(); // Trailing browser click must be suppressed after drag.
    assert.equal(selectedPaths.length, 1);

    const movedR1 = diagram.placed.circuit.items.find((i) => i.id === "component:R1");
    assert.equal(movedR1.x, baseR1.x + 20);
    assert.equal(movedR1.y, baseR1.y + 40);
    assert.equal(sidecarChanges.length, 1);
    assert.deepEqual(diagram.getSidecar().faces.circuit.items["component:R1"], { dx: 20, dy: 40 });

    // 3. Drag a rail (node:n2) horizontally at scale=1.
    diagram.scale = 1;
    const n2El = diagram.element.querySelector('[data-rail="node:n2"]');
    assert.ok(n2El);
    n2El.dispatchEvent({ type: "pointerdown", pointerId: 3, button: 0, clientX: 50, clientY: 50, target: n2El });
    n2El.dispatchEvent({ type: "pointermove", pointerId: 3, clientX: 85, clientY: 70, target: n2El });
    n2El.dispatchEvent({ type: "pointerup", pointerId: 3, clientX: 85, clientY: 70, target: n2El });

    const movedN2 = diagram.placed.circuit.rails.find((r) => r.id === "node:n2");
    assert.equal(movedN2.x, baseN2.x + 35);
    assert.deepEqual(diagram.getSidecar().faces.circuit.rails["node:n2"], { dx: 35 });

    // 4. Export layout JSON, reset, and re-import to restore identical positions.
    const exportedJson = diagram.exportLayout();
    const parsedExport = parseLayoutSidecar(exportedJson);
    assert.equal(parsedExport.valid, true);
    assert.equal(parsedExport.layout.format, LAYOUT_FORMAT);
    assert.equal(parsedExport.layout.version, LAYOUT_VERSION);

    const svgBeforeReset = diagram.svg.circuit;
    diagram.resetLayoutButton.click();
    assert.equal(diagram.getSidecar(), null);
    assert.equal(diagram.placed.circuit.items.find((i) => i.id === "component:R1").x, baseR1.x);
    assert.equal(diagram.placed.circuit.rails.find((r) => r.id === "node:n2").x, baseN2.x);

    // Re-import via <input type="file" accept=".json">.
    const fileInput = root.querySelector('input[type="file"]');
    assert.equal(fileInput.getAttribute("accept"), ".json");
    fileInput.files = [{ text: async () => exportedJson }];
    fileInput.dispatchEvent({ type: "change", target: fileInput });
    await new Promise((resolve) => setTimeout(resolve, 0));

    assert.equal(diagram.svg.circuit, svgBeforeReset);
    assert.equal(diagram.placed.circuit.items.find((i) => i.id === "component:R1").x, baseR1.x + 20);
    assert.equal(diagram.placed.circuit.items.find((i) => i.id === "component:R1").y, baseR1.y + 40);
    assert.equal(diagram.placed.circuit.rails.find((r) => r.id === "node:n2").x, baseN2.x + 35);

    // Direct setSidecar(null) clears overrides cleanly.
    diagram.setSidecar(null, { tell: true });
    assert.equal(diagram.getSidecar(), null);
    assert.equal(diagram.placed.circuit.items.find((i) => i.id === "component:R1").x, baseR1.x);
  });
});

test("InspectorPanel preserves focus across Tab field edits, setModelText, setDiagnostics, and rebuilds", async () => {
  const wasm = await loadWasm();
  const catalog = JSON.parse(wasm.components());
  const schema = JSON.parse(wasm.schema());

  await withMockDom(async (dom) => {
    const root = dom.document.createElement("div");
    const changes = [];
    const panel = new InspectorPanel(root, {
      onChange: (text) => changes.push(text),
    });
    panel.setCatalog(catalog, schema);
    panel.setModelText(rlcText);

    const dtBefore = root.querySelector('[data-field="time_step_s"] input');
    const endBefore = root.querySelector('[data-field="end_time_s"] input');
    assert.ok(dtBefore && endBefore);

    // Edit end_time_s and Tab to time_step_s before change commits.
    endBefore.focus();
    endBefore.value = "0.02";
    dtBefore.focus();
    endBefore.dispatchEvent({ type: "change", target: endBefore });

    const dtAfter = root.querySelector('[data-field="time_step_s"] input');
    assert.equal(dtAfter, dtBefore, "in-place sync must keep the same DOM input element");
    assert.equal(dom.activeElement, dtBefore, "focus must stay on next field after change");

    // Trailing validateNow sequence (`setModelText` with same text + `setDiagnostics`) must not steal focus.
    panel.setModelText(changes.at(-1), { recordHistory: true });
    assert.equal(dom.activeElement, dtBefore);

    dtBefore.value = "-1e-6";
    dtBefore.dispatchEvent({ type: "change", target: dtBefore });
    const report = JSON.parse(wasm.validate(changes.at(-1)));
    panel.setDiagnostics(report.errors, report.warnings);
    assert.equal(dom.activeElement, dtBefore, "setDiagnostics must keep focused element in DOM");
    assert.equal(root.querySelector('[data-field="time_step_s"]').classList.contains("has-error"), true);

    // Structural DOM rebuild (e.g. changing Output type in Outputs section) restores focus to the rebuilt control.
    panel.setSelection("outputs");
    const outputTypeSelect = root.querySelector(".inspector-output-type");
    assert.ok(outputTypeSelect);
    outputTypeSelect.focus();
    outputTypeSelect.value = "branch_current";
    outputTypeSelect.dispatchEvent({ type: "change", target: outputTypeSelect });
    const rebuiltTypeSelect = root.querySelector(".inspector-output-type");
    assert.ok(rebuiltTypeSelect);
    assert.equal(dom.activeElement, rebuiltTypeSelect);
  });
});
