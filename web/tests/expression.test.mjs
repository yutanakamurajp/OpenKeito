// Pure JS tests for the `expression` control block (model format 0.8):
// symbol definition, catalog/schema field metadata, InspectorPanel editing
// and round-trip JSON sync, and E243 diagnostic field mapping.
//
//   node --test web/tests/expression.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addItem,
  buildCatalogIndex,
  mapDiagnosticsToFields,
  setItemField,
  updateItem,
} from "../src/inspector.js";
import { InspectorPanel } from "../src/inspector_panel.js";
import {
  CIRCUIT,
  CONTROL,
  controlSymbol,
  symbolsWithoutType,
  typesWithoutSymbol,
} from "../src/symbols.js";

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

function buildMockExpressionCatalogAndSchema({ nullableOptional = false } = {}) {
  const catalog = [
    {
      type: "constant",
      category: "control",
      summary: "Constant signal",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Signal name" },
        { name: "value", unit: "", required: true, description: "Constant value" },
      ],
      example: { type: "constant", name: "c1", value: 1 },
    },
    {
      type: "expression",
      category: "control",
      summary: "Mathematical expression of signals and time",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Output signal name" },
        { name: "inputs", unit: "", required: false, description: "Input signal names" },
        { name: "variables", unit: "", required: false, description: "Variable-to-signal mapping" },
        { name: "expression", unit: "", required: true, description: "Mathematical expression" },
      ],
      example: {
        type: "expression",
        name: "expr1",
        inputs: ["v_in"],
        variables: { u: "v_in" },
        expression: "2 * u",
      },
    },
  ];

  const inputsSchema = nullableOptional
    ? { anyOf: [{ type: "array", items: { type: "string" } }, { type: "null" }], description: "Input signal names" }
    : { type: "array", items: { type: "string" }, description: "Input signal names" };
  const variablesSchema = nullableOptional
    ? {
        anyOf: [{ type: "object", additionalProperties: { type: "string" } }, { type: "null" }],
        description: "Variable-to-signal mapping",
      }
    : {
        type: "object",
        additionalProperties: { type: "string" },
        description: "Variable-to-signal mapping",
      };

  const schema = {
    $defs: {
      Simulation: {
        type: "object",
        required: ["time_step_s", "end_time_s"],
        properties: {
          time_step_s: { type: "number" },
          end_time_s: { type: "number" },
        },
      },
      Component: { oneOf: [] },
      Control: {
        oneOf: [
          {
            type: "object",
            required: ["type", "name", "value"],
            properties: {
              type: { const: "constant" },
              name: { type: "string" },
              value: { type: "number" },
            },
          },
          {
            type: "object",
            required: ["type", "name", "expression"],
            properties: {
              type: { const: "expression" },
              name: { type: "string" },
              inputs: inputsSchema,
              variables: variablesSchema,
              expression: { type: "string" },
            },
          },
        ],
      },
      Output: { oneOf: [] },
    },
  };

  return { catalog, schema };
}

test("CONTROL.expression has an explicit symbol and matches 63-type catalog parity", () => {
  assert.equal(Object.keys(CIRCUIT).length, 31);
  assert.equal(Object.keys(CONTROL).length, 32);
  assert.ok(Object.hasOwn(CONTROL, "expression"));

  const svg = controlSymbol("expression", 1);
  assert.ok(svg.includes('class="block-box"'));
  assert.ok(svg.includes(">expr</text>"));
  assert.equal(svg.includes('class="unknown-box"'), false);

  const multiTrackSvg = controlSymbol("expression", 3);
  assert.ok(multiTrackSvg.includes('height="56"'));
  assert.ok(multiTrackSvg.includes('translate(0 16)'));

  const mockCatalog = [
    ...Object.keys(CIRCUIT).map((type) => ({ type, category: "circuit" })),
    ...Object.keys(CONTROL).map((type) => ({ type, category: "control" })),
  ];
  assert.equal(mockCatalog.length, 63);
  assert.deepEqual(typesWithoutSymbol(mockCatalog), []);
  assert.deepEqual(symbolsWithoutType(mockCatalog), []);
});

test("buildCatalogIndex builds expression metadata for name, inputs, variables, and expression", () => {
  for (const nullableOptional of [false, true]) {
    const { catalog, schema } = buildMockExpressionCatalogAndSchema({ nullableOptional });
    const index = buildCatalogIndex(catalog, schema);
    const expr = index.get("expression");
    assert.ok(expr);
    assert.equal(expr.category, "control");

    const nameField = expr.fields.find((f) => f.name === "name");
    assert.equal(nameField.kind, "string");
    assert.equal(nameField.required, true);

    const inputsField = expr.fields.find((f) => f.name === "inputs");
    assert.equal(inputsField.kind, "array");
    assert.equal(inputsField.required, false);
    assert.equal(inputsField.itemSchema?.kind, "string");

    const varsField = expr.fields.find((f) => f.name === "variables");
    assert.equal(varsField.kind, "object");
    assert.equal(varsField.required, false);
    assert.equal(varsField.additionalProperties?.kind, "string");

    const exprField = expr.fields.find((f) => f.name === "expression");
    assert.equal(exprField.kind, "string");
    assert.equal(exprField.required, true);
  }
});

test("expression round-trip editing and E243 diagnostic field mapping", () => {
  const { catalog, schema } = buildMockExpressionCatalogAndSchema();
  const index = buildCatalogIndex(catalog, schema);
  const base = JSON.stringify({
    format: "openkeito-model",
    version: "0.8",
    simulation: { time_step_s: 1e-6, end_time_s: 0.01 },
    components: [],
    controls: [{ type: "constant", name: "v_in", value: 3 }],
  });

  let res = addItem(base, "controls", "expression", index);
  assert.equal(res.ok, true);
  assert.equal(res.model.controls[1].type, "expression");
  assert.deepEqual(res.model.controls[1].inputs, ["v_in"]);
  assert.deepEqual(res.model.controls[1].variables, { u: "v_in" });
  assert.equal(res.model.controls[1].expression, "2 * u");

  res = updateItem(res.text, "controls", 1, {
    inputs: ["v_in", "v_ref"],
    variables: { u: "v_in", r: "v_ref" },
    expression: "u - r",
  });
  assert.equal(res.ok, true);
  assert.deepEqual(res.model.controls[1].inputs, ["v_in", "v_ref"]);
  assert.deepEqual(res.model.controls[1].variables, { u: "v_in", r: "v_ref" });
  assert.equal(res.model.controls[1].expression, "u - r");

  res = setItemField(res.text, "controls", 1, "variables", undefined);
  assert.equal(res.ok, true);
  assert.equal("variables" in res.model.controls[1], false);

  const mapped = mapDiagnosticsToFields({
    errors: [
      {
        severity: "error",
        code: "E243",
        path: "controls[1].expression",
        message: "Unknown identifier `z` in expression",
        hint: "Use an input or variable name.",
      },
      {
        severity: "error",
        code: "E243",
        path: "controls[1].variables.bad_var",
        message: "Unknown signal `missing_sig`",
        hint: "Reference a defined control or component signal.",
      },
      {
        severity: "error",
        code: "E243",
        json_path: "controls[1].variables",
        message: "Invalid variables map",
      },
    ],
    warnings: [],
  });

  const exprDiags = mapped.forField("controls", 1, "expression");
  assert.equal(exprDiags.length, 1);
  assert.equal(exprDiags[0].code, "E243");

  const varDiags = mapped.forField("controls", 1, "variables");
  assert.equal(varDiags.length, 2);
  assert.ok(varDiags.some((d) => d.relativePath === "variables.bad_var"));
  assert.ok(varDiags.some((d) => d.relativePath === "variables"));
});

test("InspectorPanel edits expression block fields, validates variables JSON, and binds E243 diagnostics", async () => {
  const { catalog, schema } = buildMockExpressionCatalogAndSchema({ nullableOptional: true });
  const initialModel = JSON.stringify({
    format: "openkeito-model",
    version: "0.8",
    simulation: { time_step_s: 1e-6, end_time_s: 0.01 },
    components: [],
    controls: [
      { type: "constant", name: "v_in", value: 2 },
      { type: "constant", name: "v_ref", value: 1 },
    ],
  }, null, 2);

  await withMockDom(async (dom) => {
    const root = dom.document.createElement("div");
    const changes = [];
    const panel = new InspectorPanel(root, {
      onChange: (text) => changes.push(text),
    });

    panel.setCatalog(catalog, schema);
    panel.setModelText(initialModel);

    // Add an expression block via header bar.
    panel.addControlSelect.value = "expression";
    panel.addControlButton.click();
    assert.equal(panel.selection, "controls[2]");
    assert.equal(JSON.parse(changes.at(-1)).controls[2].type, "expression");

    // Verify all 4 fields render and signal datalist contains existing control names.
    const signalOptions = [...panel.signalsList.querySelectorAll("option")].map((o) => o.value);
    assert.ok(signalOptions.includes("v_in"));
    assert.ok(signalOptions.includes("v_ref"));

    const nameInput = root.querySelector('[data-field="name"] input');
    const exprInput = root.querySelector('[data-field="expression"] input');
    const varsArea = root.querySelector('[data-field="variables"] textarea');
    const inputsRow = root.querySelector('[data-field="inputs"]');
    assert.ok(nameInput && exprInput && varsArea && inputsRow);
    assert.equal(inputsRow.querySelector("input").getAttribute("list"), `${panel.uid}-signals`);

    // Edit name, inputs, variables, and expression.
    nameInput.value = "diff_expr";
    nameInput.dispatchEvent({ type: "change", target: nameInput });
    assert.equal(JSON.parse(changes.at(-1)).controls[2].name, "diff_expr");

    inputsRow.querySelector('[data-array-action="add"]').click();
    const input1 = root.querySelector('[data-field="inputs"] input[data-array-index="1"]');
    input1.value = "v_ref";
    input1.dispatchEvent({ type: "change", target: input1 });
    assert.deepEqual(JSON.parse(changes.at(-1)).controls[2].inputs, ["v_in", "v_ref"]);

    const liveVarsArea = root.querySelector('[data-field="variables"] textarea');
    liveVarsArea.value = JSON.stringify({ u: "v_in", r: "v_ref" });
    liveVarsArea.dispatchEvent({ type: "change", target: liveVarsArea });
    assert.deepEqual(JSON.parse(changes.at(-1)).controls[2].variables, { u: "v_in", r: "v_ref" });

    const liveExprInput = root.querySelector('[data-field="expression"] input');
    liveExprInput.value = "u - r";
    liveExprInput.dispatchEvent({ type: "change", target: liveExprInput });
    assert.equal(JSON.parse(changes.at(-1)).controls[2].expression, "u - r");

    // Invalid variables edits do not corrupt JSON and show inline parse error.
    const commitCountBeforeBad = changes.length;
    for (const badRaw of ["{ broken", "[1, 2]", '{"u": 123}']) {
      liveVarsArea.value = badRaw;
      liveVarsArea.dispatchEvent({ type: "change", target: liveVarsArea });
      assert.equal(changes.length, commitCountBeforeBad);
      const errEl = root.querySelector('[data-field="variables"] .inspector-parse-error');
      assert.equal(errEl.hidden, false);
    }

    // Undo clears stale parse error and restores previous valid variables.
    panel.undo();
    assert.equal(root.querySelector('[data-field="variables"] .inspector-parse-error').hidden, true);
    assert.equal(JSON.parse(changes.at(-1)).controls[2].expression, "2 * u");
    panel.redo();
    assert.equal(JSON.parse(changes.at(-1)).controls[2].expression, "u - r");

    // Clearing optional variables removes the key cleanly.
    const clearedVarsArea = root.querySelector('[data-field="variables"] textarea');
    clearedVarsArea.value = "";
    clearedVarsArea.dispatchEvent({ type: "change", target: clearedVarsArea });
    assert.equal("variables" in JSON.parse(changes.at(-1)).controls[2], false);

    // Removing all inputs removes the optional inputs key cleanly.
    root.querySelector('[data-field="inputs"] [data-array-action="remove"][data-array-index="1"]').click();
    root.querySelector('[data-field="inputs"] [data-array-action="remove"][data-array-index="0"]').click();
    assert.equal("inputs" in JSON.parse(changes.at(-1)).controls[2], false);

    // E243 diagnostics on controls[2].expression and controls[2].variables.<k> attach to each field.
    panel.setDiagnostics(
      [
        {
          severity: "error",
          code: "E243",
          path: "controls[2].expression",
          message: "Syntax error in expression",
          hint: "Check operators and parentheses.",
        },
        {
          severity: "error",
          code: "E243",
          path: "controls[2].variables.bad_key",
          message: "Unknown signal referenced by variable",
          hint: "Use an existing signal name.",
        },
      ],
      [],
    );

    const exprField = root.querySelector('[data-field="expression"]');
    const varsField = root.querySelector('[data-field="variables"]');
    assert.equal(exprField.classList.contains("has-error"), true);
    assert.ok(exprField.querySelector(".inspector-diagnostic").textContent.includes("E243"));
    assert.equal(varsField.classList.contains("has-error"), true);
    assert.ok(varsField.querySelector(".inspector-diagnostic").textContent.includes("E243"));
    assert.ok(varsField.querySelector(".inspector-diagnostic").textContent.includes("controls[2].variables.bad_key"));
  });
});
