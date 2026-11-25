// Structured model editing panel: catalog/schema-driven parameter forms for
// `simulation`, `components`, `controls`, and `outputs`, item add/delete,
// node/signal/bus connection `<datalist>` hints, undo/redo history, and inline
// diagnostics mapped to each field.

import { fill, h } from "./dom.js";
import {
  addItem,
  buildCatalogIndex,
  createEditHistory,
  getOutputTypes,
  getSimulationFields,
  mapDiagnosticsToFields,
  parseModelForEdit,
  removeItem,
  setItemField,
  toggleOutputForTarget,
  updateItem,
  updateSimulationField,
} from "./inspector.js";

let nextPanelId = 1;

const NODE_FIELDS = new Set(["neutral", "hv_neutral", "lv_neutral", "node"]);
const SIGNAL_FIELDS = new Set([
  "gate",
  "signal",
  "field_voltage_signal",
  "mechanical_torque_signal",
  "input",
  "condition",
  "angle",
  "d",
  "q",
  "zero",
]);
const SIGNAL_ARRAY_FIELDS = new Set(["signals", "inputs", "feedforward"]);
const NUMBER_RE = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/;

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function parseSelection(path) {
  if (typeof path !== "string" || path.length === 0) return null;
  if (path === "simulation" || path.startsWith("simulation.")) return "simulation";
  if (path === "outputs" || path.startsWith("outputs[")) return "outputs";
  const match = /^(components|controls)\[(\d+)\]/.exec(path);
  return match ? `${match[1]}[${match[2]}]` : null;
}

function splitItemSelection(selection) {
  const match = /^(components|controls)\[(\d+)\]$/.exec(selection ?? "");
  if (!match) return null;
  return { section: match[1], index: Number(match[2]) };
}

function collectCandidates(model) {
  const nodes = new Set(["gnd"]);
  const buses = new Set();
  const signals = new Set();

  const addNode = (val) => {
    if (typeof val === "string" && val.length > 0) nodes.add(val);
    else if (Array.isArray(val)) val.forEach(addNode);
  };

  for (const c of Array.isArray(model?.components) ? model.components : []) {
    if (!isPlainObject(c)) continue;
    addNode(c.nodes);
    addNode(c.neutral);
    addNode(c.hv_neutral);
    addNode(c.lv_neutral);
    if (typeof c.bus === "string" && c.bus.length > 0) buses.add(c.bus);
    if (Array.isArray(c.buses)) {
      for (const b of c.buses) if (typeof b === "string" && b.length > 0) buses.add(b);
    }
    for (const key of ["gate", "signal", "field_voltage_signal", "mechanical_torque_signal"]) {
      if (typeof c[key] === "string" && c[key].length > 0) signals.add(c[key]);
    }
    if (Array.isArray(c.signals)) {
      for (const s of c.signals) if (typeof s === "string" && s.length > 0) signals.add(s);
    }
  }

  for (const ctrl of Array.isArray(model?.controls) ? model.controls : []) {
    if (!isPlainObject(ctrl) || typeof ctrl.name !== "string" || ctrl.name.length === 0) continue;
    signals.add(ctrl.name);
    if (ctrl.type === "abc_to_dq0" || ctrl.type === "dq_limiter" || ctrl.type === "pi_dq") {
      signals.add(`${ctrl.name}.d`);
      signals.add(`${ctrl.name}.q`);
      if (ctrl.type === "abc_to_dq0") signals.add(`${ctrl.name}.zero`);
    } else if (ctrl.type === "dq0_to_abc" || ctrl.type === "three_phase_measurement") {
      signals.add(`${ctrl.name}.a`);
      signals.add(`${ctrl.name}.b`);
      signals.add(`${ctrl.name}.c`);
    } else if (ctrl.type === "pll") {
      signals.add(`${ctrl.name}.theta`);
      signals.add(`${ctrl.name}.frequency_hz`);
      signals.add(`${ctrl.name}.d`);
      signals.add(`${ctrl.name}.q`);
    }
  }

  return {
    nodes: [...nodes].sort(),
    buses: [...buses].sort(),
    signals: [...signals].sort(),
  };
}

function fallbackFieldsForObject(obj) {
  return Object.keys(obj ?? {})
    .filter((k) => k !== "type")
    .map((name) => {
      const value = obj[name];
      let kind = "string";
      if (typeof value === "boolean") kind = "boolean";
      else if (typeof value === "number") kind = "number";
      else if (Array.isArray(value)) kind = name === "nodes" ? "nodes" : "array";
      else if (isPlainObject(value)) kind = "object";
      return { name, unit: "", required: false, description: "", kind };
    });
}

function formatOutputSummary(entry) {
  if (!isPlainObject(entry)) return String(entry);
  const parts = Object.entries(entry)
    .filter(([k, v]) => k !== "type" && v !== undefined)
    .map(([k, v]) => `${k}: ${String(v)}`);
  return parts.length > 0 ? `${entry.type} (${parts.join(", ")})` : String(entry.type ?? "output");
}

function diagnosticNode(diag) {
  const severity = diag.severity === "warning" ? "warning" : "error";
  const label = `${severity === "error" ? "Error" : "Warning"}${diag.code ? ` ${diag.code}` : ""}`;
  return h(
    "div",
    { class: `diagnostic inspector-diagnostic ${severity}` },
    h(
      "div",
      { class: "diagnostic-head" },
      h("span", { class: "badge", text: label }),
      diag.path && h("span", { class: "diagnostic-path", text: diag.path }),
    ),
    h("p", { class: "diagnostic-message", text: diag.message ?? "" }),
    diag.hint && h("p", { class: "diagnostic-hint" }, h("strong", { text: "Hint: " }), diag.hint),
  );
}

export class InspectorPanel {
  /// `root`: container element for the structured editing panel.
  /// `onChange(nextText)`: invoked whenever form editing, undo, or redo produces updated JSON text.
  /// `onSelectPath(path)`: invoked when the user changes the edited target in the panel.
  constructor(root, { onChange = () => {}, onSelectPath = () => {} } = {}) {
    this.root = root;
    this.onChange = onChange;
    this.onSelectPath = onSelectPath;
    this.uid = `inspector-${nextPanelId++}`;
    this.catalogIndex = null;
    this.text = "";
    this.parsed = { ok: false, error: "No model loaded" };
    this.history = createEditHistory("");
    this.hasModel = false;
    this.selection = "simulation";
    this.diagnostics = mapDiagnosticsToFields([]);
    this.outputDraft = { type: "", values: {} };
    this.jsonDrafts = new Map();
    this.renderedState = null;
    this.recentFocus = null;
    this.inPanelCommit = false;

    this.undoButton = h(
      "button",
      { type: "button", class: "quiet-button inspector-undo", onclick: () => this.undo() },
      "Undo",
    );
    this.redoButton = h(
      "button",
      { type: "button", class: "quiet-button inspector-redo", onclick: () => this.redo() },
      "Redo",
    );
    this.targetSelect = h("select", {
      class: "inspector-target",
      "aria-label": "Edit target",
      onchange: () => {
        this.selection = this.targetSelect.value;
        this.render();
        this.onSelectPath(this.selection);
      },
    });
    this.addComponentSelect = h("select", {
      class: "inspector-add-type",
      "aria-label": "Component type to add",
    });
    this.addComponentButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button inspector-add-component",
        onclick: () => this.handleAdd("components", this.addComponentSelect.value),
      },
      "+ Component",
    );
    this.addControlSelect = h("select", {
      class: "inspector-add-type",
      "aria-label": "Control type to add",
    });
    this.addControlButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button inspector-add-control",
        onclick: () => this.handleAdd("controls", this.addControlSelect.value),
      },
      "+ Control",
    );
    this.deleteButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button inspector-delete",
        onclick: () => this.handleDelete(),
      },
      "Delete",
    );

    this.headerBar = h(
      "div",
      { class: "inspector-header" },
      h("div", { class: "actions inspector-history" }, this.undoButton, this.redoButton),
      h("div", { class: "field inspector-target-field" }, this.targetSelect),
      h(
        "div",
        { class: "actions inspector-add-bar" },
        this.addComponentSelect,
        this.addComponentButton,
        this.addControlSelect,
        this.addControlButton,
        this.deleteButton,
      ),
    );

    this.nodesList = h("datalist", { id: `${this.uid}-nodes` });
    this.busesList = h("datalist", { id: `${this.uid}-buses` });
    this.signalsList = h("datalist", { id: `${this.uid}-signals` });
    this.datalistBox = h("div", { class: "inspector-datalists", hidden: true }, this.nodesList, this.busesList, this.signalsList);
    this.body = h("div", { class: "inspector-body" });
    this.body.addEventListener("focusin", (ev) => {
      const desc = this.descriptorFromElement(ev.target);
      if (desc) this.recentFocus = desc;
    });

    this.root.append(this.headerBar, this.datalistBox, this.body);
    this.syncCatalogSelects();
    this.render();
  }

  getActiveElement() {
    return this.root?.ownerDocument?.activeElement ?? (typeof document !== "undefined" ? document.activeElement : null);
  }

  isInsideBody(el) {
    let cur = el;
    while (cur) {
      if (cur === this.body) return true;
      cur = cur.parentNode ?? null;
    }
    return false;
  }

  descriptorFromElement(el) {
    if (!el || !this.isInsideBody(el)) return null;
    const fieldRow = el.closest?.("[data-field]");
    if (fieldRow?.dataset?.field) {
      return {
        kind: "field",
        selection: this.selection,
        field: fieldRow.dataset.field,
        arrayIndex: el.dataset?.arrayIndex ?? null,
        arrayAction: el.dataset?.arrayAction ?? null,
      };
    }
    const outputRow = el.closest?.("[data-output-field]");
    if (outputRow?.dataset?.outputField) {
      return {
        kind: "output-field",
        selection: "outputs",
        field: outputRow.dataset.outputField,
      };
    }
    if (el.classList?.contains("inspector-output-type")) {
      return { kind: "output-type", selection: "outputs" };
    }
    if (el.classList?.contains("inspector-output-add-button")) {
      return { kind: "output-add", selection: "outputs" };
    }
    const outputItem = el.closest?.("[data-output-index]");
    if (outputItem?.dataset?.outputIndex !== undefined) {
      return { kind: "output-remove", selection: "outputs", index: outputItem.dataset.outputIndex };
    }
    return null;
  }

  captureFocus() {
    const active = this.getActiveElement();
    const direct = this.descriptorFromElement(active);
    if (direct) return direct;
    if (this.inPanelCommit && this.recentFocus && this.recentFocus.selection === this.selection) {
      return this.recentFocus;
    }
    return null;
  }

  restoreFocus(desc) {
    if (!desc || desc.selection !== this.selection) return;
    let target = null;
    if (desc.kind === "field") {
      const row = this.body.querySelector?.(`[data-field="${desc.field}"]`);
      if (row) {
        if (desc.arrayAction === "add") {
          target = row.querySelector?.('[data-array-action="add"]');
        } else if (desc.arrayAction === "remove" && desc.arrayIndex !== null) {
          target =
            row.querySelector?.(`[data-array-action="remove"][data-array-index="${desc.arrayIndex}"]`) ??
            row.querySelector?.(`[data-array-index="${desc.arrayIndex}"]`) ??
            row.querySelector?.("input, select, textarea, button");
        } else if (desc.arrayIndex !== null) {
          target =
            row.querySelector?.(`input[data-array-index="${desc.arrayIndex}"]`) ??
            row.querySelector?.(`[data-array-index="${desc.arrayIndex}"]`) ??
            row.querySelector?.("input, select, textarea, button");
        } else {
          target = row.querySelector?.("input, select, textarea, button");
        }
      }
    } else if (desc.kind === "output-field") {
      target = this.body.querySelector?.(`[data-output-field="${desc.field}"] input`);
    } else if (desc.kind === "output-type") {
      target = this.body.querySelector?.(".inspector-output-type");
    } else if (desc.kind === "output-add") {
      target = this.body.querySelector?.(".inspector-output-add-button");
    } else if (desc.kind === "output-remove") {
      target =
        this.body.querySelector?.(`[data-output-index="${desc.index}"] .inspector-output-remove`) ??
        this.body.querySelector?.(".inspector-output-add-button");
    }
    if (!target || target === this.getActiveElement()) return;
    try {
      target.focus?.({ preventScroll: true });
    } catch {
      target.focus?.();
    }
  }

  /// Sets the component/control catalog and JSON schema metadata.
  setCatalog(catalog, schema) {
    this.catalogIndex = catalog ? buildCatalogIndex(catalog, schema) : null;
    this.syncCatalogSelects();
    const outputTypes = getOutputTypes(this.catalogIndex);
    if (!outputTypes.some((o) => o.type === this.outputDraft.type)) {
      this.outputDraft = { type: outputTypes[0]?.type ?? "", values: {} };
    }
    this.renderedState = null;
    this.render();
  }

  /// Updates the panel with model JSON text. When `recordHistory` is true,
  /// pushes a snapshot onto the undo stack unless unchanged.
  setModelText(text, { recordHistory = true } = {}) {
    const nextText = String(text ?? "");
    if (!this.hasModel) {
      this.history = createEditHistory(nextText);
      this.hasModel = true;
    } else if (recordHistory) {
      this.history.push(nextText);
    } else if (nextText !== this.history.current()) {
      this.history.reset(nextText);
    }
    if (this.hasModel && nextText === this.text && this.parsed.ok) {
      this.updateHeaderControls();
      return;
    }
    this.jsonDrafts.clear();
    this.applyParsedText(nextText);
  }

  /// Selects an item or section by model path (e.g. `"components[1]"`,
  /// `"controls[0]"`, `"simulation"`, `"outputs"`).
  setSelection(path) {
    const normalized = parseSelection(path);
    if (!normalized) return;
    this.selection = normalized;
    this.normalizeSelection();
    this.render();
  }

  /// Displays validation errors and warnings next to their matching fields.
  setDiagnostics(errors = [], warnings = []) {
    if (errors && !Array.isArray(errors) && typeof errors === "object") {
      this.diagnostics = mapDiagnosticsToFields(errors);
    } else {
      this.diagnostics = mapDiagnosticsToFields({
        errors: Array.isArray(errors) ? errors : [],
        warnings: Array.isArray(warnings) ? warnings : [],
      });
    }
    if (this.patchDiagnosticsDom()) return;
    this.render();
  }

  canUndo() {
    return this.hasModel && this.history.canUndo();
  }

  canRedo() {
    return this.hasModel && this.history.canRedo();
  }

  /// Reverts to the previous snapshot in history and notifies `onChange`.
  undo() {
    if (!this.canUndo()) return null;
    const prev = this.history.undo();
    if (prev === null) return null;
    this.jsonDrafts.clear();
    this.applyParsedText(prev);
    this.onChange(prev);
    return prev;
  }

  /// Re-applies a reverted snapshot and notifies `onChange`.
  redo() {
    if (!this.canRedo()) return null;
    const next = this.history.redo();
    if (next === null) return null;
    this.jsonDrafts.clear();
    this.applyParsedText(next);
    this.onChange(next);
    return next;
  }

  applyParsedText(text) {
    this.text = text;
    this.parsed = parseModelForEdit(text);
    this.normalizeSelection();
    this.render();
  }

  normalizeSelection() {
    if (!this.parsed.ok) return;
    const model = this.parsed.model;
    if (this.selection === "simulation" || this.selection === "outputs") return;
    const itemRef = splitItemSelection(this.selection);
    if (!itemRef) {
      this.selection = "simulation";
      return;
    }
    const list = model[itemRef.section];
    if (!Array.isArray(list) || list.length === 0) {
      this.selection = "simulation";
      return;
    }
    if (itemRef.index >= list.length) {
      this.selection = `${itemRef.section}[${list.length - 1}]`;
    }
  }

  commitEdit(result, { nextSelection } = {}) {
    if (!result || !result.ok) return false;
    this.inPanelCommit = true;
    try {
      this.history.push(result.text);
      this.jsonDrafts.clear();
      if (nextSelection !== undefined) this.selection = nextSelection;
      this.applyParsedText(result.text);
      this.onChange(result.text);
      if (nextSelection !== undefined) this.onSelectPath(this.selection);
      return true;
    } finally {
      this.inPanelCommit = false;
    }
  }

  syncCatalogSelects() {
    const components = this.catalogIndex?.components ?? [];
    const controls = this.catalogIndex?.controls ?? [];
    fill(
      this.addComponentSelect,
      components.map((c) => h("option", { value: c.type, text: c.type })),
    );
    if (components.length > 0 && !components.some((c) => c.type === this.addComponentSelect.value)) {
      this.addComponentSelect.value = components[0].type;
    }
    fill(
      this.addControlSelect,
      controls.map((c) => h("option", { value: c.type, text: c.type })),
    );
    if (controls.length > 0 && !controls.some((c) => c.type === this.addControlSelect.value)) {
      this.addControlSelect.value = controls[0].type;
    }
  }

  handleAdd(section, typeName) {
    if (!this.parsed.ok || !this.catalogIndex || !typeName) return;
    const prevList = Array.isArray(this.parsed.model[section]) ? this.parsed.model[section] : [];
    const nextIndex = prevList.length;
    const res = addItem(this.text, section, typeName, this.catalogIndex);
    this.commitEdit(res, { nextSelection: `${section}[${nextIndex}]` });
  }

  handleDelete() {
    if (!this.parsed.ok) return;
    const itemRef = splitItemSelection(this.selection);
    if (!itemRef) return;
    const list = this.parsed.model[itemRef.section];
    if (!Array.isArray(list) || list.length === 0) return;
    const nextLength = list.length - 1;
    const nextSelection =
      nextLength === 0
        ? "simulation"
        : `${itemRef.section}[${Math.min(itemRef.index, nextLength - 1)}]`;
    const res = removeItem(this.text, itemRef.section, itemRef.index);
    this.commitEdit(res, { nextSelection });
  }

  updateHeaderControls() {
    this.undoButton.disabled = !this.canUndo();
    this.redoButton.disabled = !this.canRedo();

    const usable = this.parsed.ok;
    const hasCatalog = Boolean(this.catalogIndex);
    this.targetSelect.disabled = !usable;
    this.addComponentSelect.disabled = !usable || !hasCatalog || (this.catalogIndex?.components?.length ?? 0) === 0;
    this.addComponentButton.disabled = this.addComponentSelect.disabled;
    this.addControlSelect.disabled = !usable || !hasCatalog || (this.catalogIndex?.controls?.length ?? 0) === 0;
    this.addControlButton.disabled = this.addControlSelect.disabled;

    const itemRef = splitItemSelection(this.selection);
    this.deleteButton.disabled = !usable || !itemRef;

    if (!usable) {
      fill(this.targetSelect, h("option", { value: "simulation", text: "Simulation" }));
      this.targetSelect.value = "simulation";
      return;
    }

    const model = this.parsed.model;
    const options = [h("option", { value: "simulation", text: "Simulation" })];
    for (const [section, label] of [["components", "Component"], ["controls", "Control"]]) {
      const list = Array.isArray(model[section]) ? model[section] : [];
      list.forEach((item, index) => {
        const name = item?.name ? `${item.name} ` : "";
        const type = item?.type ? `(${item.type})` : "";
        options.push(
          h("option", {
            value: `${section}[${index}]`,
            text: `${label} ${section}[${index}]: ${name}${type}`.trim(),
          }),
        );
      });
    }
    const outputCount = Array.isArray(model.outputs) ? model.outputs.length : 0;
    const outputLabel = Array.isArray(model.outputs) ? `Outputs (${outputCount})` : "Outputs (all)";
    options.push(h("option", { value: "outputs", text: outputLabel }));
    fill(this.targetSelect, options);
    this.targetSelect.value = this.selection;
  }

  updateDatalists(model) {
    const candidates = collectCandidates(model);
    fill(this.nodesList, candidates.nodes.map((n) => h("option", { value: n })));
    fill(this.busesList, candidates.buses.map((b) => h("option", { value: b })));
    fill(this.signalsList, candidates.signals.map((s) => h("option", { value: s })));
    return candidates;
  }

  listIdForField(field) {
    if (field.kind === "nodes" || NODE_FIELDS.has(field.name)) return `${this.uid}-nodes`;
    if (field.kind === "bus" || (field.kind === "array" && field.itemSchema?.kind === "bus")) {
      return `${this.uid}-buses`;
    }
    if (
      field.kind === "number_or_signal" ||
      SIGNAL_FIELDS.has(field.name) ||
      SIGNAL_ARRAY_FIELDS.has(field.name)
    ) {
      return `${this.uid}-signals`;
    }
    return undefined;
  }

  simulationFieldSpecs(model) {
    const sim = isPlainObject(model.simulation) ? model.simulation : {};
    const fields = getSimulationFields(this.catalogIndex);
    const effectiveFields = fields.length > 0 ? [...fields] : fallbackFieldsForObject(sim);
    const renderedNames = new Set(effectiveFields.map((f) => f.name));
    for (const k of Object.keys(sim)) {
      if (!renderedNames.has(k)) {
        effectiveFields.push({ name: k, unit: "", required: false, description: "", kind: "string" });
        renderedNames.add(k);
      }
    }
    return { source: sim, fields: effectiveFields, renderedNames };
  }

  itemFieldSpecs(model, section, index) {
    const list = model[section];
    if (!Array.isArray(list) || !isPlainObject(list[index])) return null;
    const item = list[index];
    const catalogEntry = this.catalogIndex?.get(item.type) ?? null;
    const fields = catalogEntry ? [...catalogEntry.fields] : fallbackFieldsForObject(item);
    const renderedNames = new Set(fields.map((f) => f.name));
    for (const key of Object.keys(item)) {
      if (key === "type" || renderedNames.has(key)) continue;
      fields.push(...fallbackFieldsForObject({ [key]: item[key] }));
      renderedNames.add(key);
    }
    for (const d of this.diagnostics.forItem(section, index)) {
      if (d.field && !renderedNames.has(d.field)) {
        fields.push({ name: d.field, unit: "", required: false, description: "", kind: "string" });
        renderedNames.add(d.field);
      }
    }
    return { item, catalogEntry, fields, renderedNames };
  }

  diagnosticsStateForCurrent(model) {
    if (!this.parsed.ok) return null;
    if (this.selection === "simulation") {
      const { renderedNames } = this.simulationFieldSpecs(model);
      const sectionDiags = this.diagnostics
        .forSection("simulation")
        .filter((d) => !d.field || !renderedNames.has(d.field));
      return {
        sectionDiags,
        forField: (name) => this.diagnostics.forSimulationField(name),
      };
    }
    const itemRef = splitItemSelection(this.selection);
    if (itemRef) {
      const spec = this.itemFieldSpecs(model, itemRef.section, itemRef.index);
      if (!spec) return null;
      const itemDiags = this.diagnostics.forItem(itemRef.section, itemRef.index);
      const missingField = itemDiags.some((d) => d.field && !this.renderedState?.controllers?.has(d.field));
      if (missingField) return null;
      return {
        sectionDiags: itemDiags.filter((d) => !d.field || !spec.renderedNames.has(d.field)),
        forField: (name) => this.diagnostics.forField(itemRef.section, itemRef.index, name),
      };
    }
    return null;
  }

  patchDiagnosticsDom() {
    if (!this.renderedState || !this.parsed.ok || this.renderedState.selection !== this.selection) {
      return false;
    }
    const diagState = this.diagnosticsStateForCurrent(this.parsed.model);
    if (!diagState) return false;
    fill(this.renderedState.sectionDiagEl, diagState.sectionDiags.map(diagnosticNode));
    for (const [name, ctrl] of this.renderedState.controllers) {
      const fieldDiags = diagState.forField(name);
      ctrl.rowEl.classList.toggle("has-error", fieldDiags.some((d) => d.severity !== "warning"));
      ctrl.rowEl.classList.toggle("has-warning", fieldDiags.length > 0 && !fieldDiags.some((d) => d.severity !== "warning"));
      fill(ctrl.diagBox, fieldDiags.map(diagnosticNode));
    }
    return true;
  }

  trySyncRenderedInPlace(model) {
    if (!this.renderedState || this.renderedState.selection !== this.selection) return false;
    let key;
    let fields;
    let source;
    let draftPrefix;
    if (this.selection === "simulation") {
      const spec = this.simulationFieldSpecs(model);
      key = "simulation";
      fields = spec.fields;
      source = spec.source;
      draftPrefix = "simulation";
    } else {
      const itemRef = splitItemSelection(this.selection);
      if (!itemRef) return false;
      const spec = this.itemFieldSpecs(model, itemRef.section, itemRef.index);
      if (!spec) return false;
      key = `${itemRef.section}[${itemRef.index}]:${spec.item.type ?? ""}`;
      fields = spec.fields;
      source = spec.item;
      draftPrefix = `${itemRef.section}[${itemRef.index}]`;
    }
    if (this.renderedState.key !== key) return false;
    const prevNames = [...this.renderedState.controllers.keys()];
    if (prevNames.length !== fields.length || fields.some((f, idx) => f.name !== prevNames[idx])) {
      return false;
    }
    const active = this.getActiveElement();
    for (const field of fields) {
      const ctrl = this.renderedState.controllers.get(field.name);
      if (!ctrl.canSync(source[field.name])) return false;
      ctrl.sync(source[field.name], active, `${draftPrefix}.${field.name}`);
    }
    if (this.renderedState.headingEl && source !== model.simulation) {
      this.renderedState.headingEl.textContent = `${source.name ?? this.selection} (${source.type ?? "unknown"})`;
    }
    this.patchDiagnosticsDom();
    return true;
  }

  render() {
    this.updateHeaderControls();
    if (!this.hasModel) {
      this.renderedState = null;
      fill(this.body, h("p", { class: "empty", text: "Load a model to edit its parameters." }));
      return;
    }
    if (!this.parsed.ok) {
      this.renderedState = null;
      fill(
        this.body,
        h(
          "p",
          { class: "note inspector-unparseable", role: "status" },
          `Structured editing is disabled while JSON cannot be parsed (${this.parsed.error}). Fix the JSON text to restore the form.`,
        ),
      );
      return;
    }

    const model = this.parsed.model;
    const candidates = this.updateDatalists(model);

    if (this.trySyncRenderedInPlace(model)) {
      return;
    }

    const capturedFocus = this.captureFocus();
    this.renderedState = null;

    if (this.selection === "simulation") {
      fill(this.body, this.renderSimulation(model));
      this.restoreFocus(capturedFocus);
      return;
    }
    if (this.selection === "outputs") {
      fill(this.body, this.renderOutputs(model, candidates));
      this.restoreFocus(capturedFocus);
      return;
    }
    const itemRef = splitItemSelection(this.selection);
    if (itemRef) {
      fill(this.body, this.renderItem(model, itemRef.section, itemRef.index));
      this.restoreFocus(capturedFocus);
      return;
    }
    fill(this.body, this.renderSimulation(model));
    this.restoreFocus(capturedFocus);
  }

  renderFieldRow(field, value, fieldDiags, onCommit, draftKey) {
    const labelChildren = [h("span", { class: "inspector-field-name", text: field.name })];
    if (field.unit) {
      labelChildren.push(" ", h("span", { class: "inspector-unit", text: `(${field.unit})` }));
    }
    if (field.required) {
      labelChildren.push(" ", h("span", { class: "inspector-required", title: "Required", text: "*" }));
    }

    const rowError = h("p", { class: "hint inspector-parse-error", hidden: true });
    const built = this.buildFieldControl(field, value, onCommit, draftKey, rowError);
    const labelEl = h("label", { title: field.description || undefined }, ...labelChildren);
    const diagBox = h("div", { class: "inspector-field-diagnostics" }, ...fieldDiags.map(diagnosticNode));
    const hasError = fieldDiags.some((d) => d.severity !== "warning");
    const hasWarning = fieldDiags.length > 0 && !hasError;

    const rowEl = h(
      "div",
      {
        class: `field inspector-field${hasError ? " has-error" : hasWarning ? " has-warning" : ""}`,
        "data-field": field.name,
        title: field.description || undefined,
      },
      labelEl,
      built.element,
      rowError,
      diagBox,
    );
    return {
      rowEl,
      diagBox,
      canSync: built.canSync,
      sync: built.sync,
    };
  }

  buildFieldControl(field, value, onCommit, draftKey, rowError) {
    const title = field.description || undefined;
    const listId = this.listIdForField(field);

    if (field.kind === "enum") {
      const select = h("select", {
        title,
        "aria-label": field.name,
        onchange: () => {
          const next = select.value;
          onCommit(next === "" ? undefined : next);
        },
      });
      if (!field.required || field.nullable) {
        select.append(h("option", { value: "", text: "(unset)" }));
      }
      for (const opt of field.enumValues ?? []) {
        select.append(h("option", { value: opt, text: opt }));
      }
      select.value = value === undefined || value === null ? "" : String(value);
      return {
        element: select,
        canSync: () => true,
        sync: (val, active) => {
          const next = val === undefined || val === null ? "" : String(val);
          if (select !== active && select.value !== next) select.value = next;
        },
      };
    }

    if (field.kind === "boolean") {
      const checkbox = h("input", {
        type: "checkbox",
        title,
        "aria-label": field.name,
        checked: Boolean(value),
        onchange: (ev) => onCommit(Boolean(ev.target.checked)),
      });
      return {
        element: checkbox,
        canSync: () => true,
        sync: (val, active) => {
          const next = Boolean(val);
          if (checkbox !== active && checkbox.checked !== next) checkbox.checked = next;
        },
      };
    }

    if (field.kind === "number" || field.kind === "integer") {
      const input = h("input", {
        type: "number",
        step: field.kind === "integer" ? "1" : "any",
        min: field.minimum !== undefined ? String(field.minimum) : undefined,
        max: field.maximum !== undefined ? String(field.maximum) : undefined,
        title,
        "aria-label": field.name,
        value: value === undefined || value === null ? "" : String(value),
        onchange: () => {
          const raw = input.value.trim();
          if (raw === "") {
            if (!field.required || field.nullable) {
              rowError.hidden = true;
              onCommit(undefined);
            } else {
              rowError.textContent = "Value is required.";
              rowError.hidden = false;
            }
            return;
          }
          const num = Number(raw);
          if (!Number.isFinite(num) || (field.kind === "integer" && !Number.isInteger(num))) {
            rowError.textContent = field.kind === "integer" ? "Enter an integer." : "Enter a finite number.";
            rowError.hidden = false;
            return;
          }
          rowError.hidden = true;
          onCommit(num);
        },
      });
      return {
        element: input,
        canSync: () => true,
        sync: (val, active) => {
          const next = val === undefined || val === null ? "" : String(val);
          if (input !== active && input.value !== next) input.value = next;
        },
      };
    }

    if (field.kind === "number_or_signal") {
      const input = h("input", {
        type: "text",
        list: listId,
        title,
        "aria-label": field.name,
        value: value === undefined || value === null ? "" : String(value),
        onchange: () => {
          const raw = input.value.trim();
          if (raw === "") {
            onCommit(undefined);
            return;
          }
          if (NUMBER_RE.test(raw)) {
            const num = Number(raw);
            if (Number.isFinite(num)) {
              onCommit(num);
              return;
            }
          }
          onCommit(raw);
        },
      });
      return {
        element: input,
        canSync: () => true,
        sync: (val, active) => {
          const next = val === undefined || val === null ? "" : String(val);
          if (input !== active && input.value !== next) input.value = next;
        },
      };
    }

    if (field.kind === "nodes" || (field.kind === "array" && (field.itemSchema?.kind === "bus" || field.itemSchema?.kind === "string"))) {
      return this.buildStringArrayControl(field, value, listId, onCommit);
    }

    if (field.kind === "bus") {
      const serializeBus = (v) => String(Array.isArray(v) ? JSON.stringify(v) : (v ?? ""));
      const input = h("input", {
        type: "text",
        list: listId,
        title,
        "aria-label": field.name,
        value: serializeBus(value),
        onchange: () => {
          const raw = input.value.trim();
          if (raw === "") {
            rowError.hidden = true;
            onCommit(undefined);
            return;
          }
          if (raw.startsWith("[")) {
            try {
              const parsed = JSON.parse(raw);
              if (!Array.isArray(parsed)) throw new Error("Expected array");
              rowError.hidden = true;
              onCommit(parsed);
            } catch (err) {
              rowError.textContent = `Invalid JSON array: ${err.message}`;
              rowError.hidden = false;
            }
            return;
          }
          rowError.hidden = true;
          onCommit(raw);
        },
      });
      return {
        element: input,
        canSync: () => true,
        sync: (val, active) => {
          const next = serializeBus(val);
          if (input !== active && input.value !== next) input.value = next;
        },
      };
    }

    if (field.kind === "object" || field.kind === "array") {
      const serializeJson = (v, key) =>
        this.jsonDrafts.has(key)
          ? this.jsonDrafts.get(key)
          : (v === undefined || v === null ? "" : JSON.stringify(v, null, 2));
      const initial = serializeJson(value, draftKey);
      const area = h("textarea", {
        class: "inspector-json-input",
        rows: "3",
        spellcheck: "false",
        title,
        "aria-label": field.name,
        value: initial,
        onchange: () => {
          const raw = area.value.trim();
          if (raw === "") {
            this.jsonDrafts.delete(draftKey);
            rowError.hidden = true;
            onCommit(undefined);
            return;
          }
          try {
            const parsed = JSON.parse(raw);
            if (field.kind === "object" && !isPlainObject(parsed)) {
              this.jsonDrafts.set(draftKey, area.value);
              rowError.textContent = "Expected a JSON object.";
              rowError.hidden = false;
              return;
            }
            if (field.kind === "array" && !Array.isArray(parsed)) {
              this.jsonDrafts.set(draftKey, area.value);
              rowError.textContent = "Expected a JSON array.";
              rowError.hidden = false;
              return;
            }
            if (
              field.kind === "object" &&
              field.additionalProperties?.kind === "string" &&
              Object.values(parsed).some((v) => typeof v !== "string")
            ) {
              this.jsonDrafts.set(draftKey, area.value);
              rowError.textContent = "All map values must be strings.";
              rowError.hidden = false;
              return;
            }
            this.jsonDrafts.delete(draftKey);
            rowError.hidden = true;
            onCommit(parsed);
          } catch (err) {
            this.jsonDrafts.set(draftKey, area.value);
            rowError.textContent = `Invalid JSON: ${err.message}`;
            rowError.hidden = false;
          }
        },
      });
      // Textarea value property needs explicit sync in some DOM implementations.
      area.textContent = initial;
      area.value = initial;
      return {
        element: area,
        canSync: () => true,
        sync: (val, active, nextDraftKey) => {
          if (area === active || this.jsonDrafts.has(nextDraftKey)) return;
          rowError.hidden = true;
          const next = val === undefined || val === null ? "" : JSON.stringify(val, null, 2);
          if (area.value !== next) {
            area.textContent = next;
            area.value = next;
          }
        },
      };
    }

    const input = h("input", {
      type: "text",
      list: listId,
      title,
      "aria-label": field.name,
      value: value === undefined || value === null ? "" : String(value),
      onchange: () => {
        const raw = input.value;
        onCommit(raw === "" && (!field.required || field.nullable) ? undefined : raw);
      },
    });
    return {
      element: input,
      canSync: () => true,
      sync: (val, active) => {
        const next = val === undefined || val === null ? "" : String(val);
        if (input !== active && input.value !== next) input.value = next;
      },
    };
  }

  buildStringArrayControl(field, value, listId, onCommit) {
    let list = Array.isArray(value) ? [...value] : [];
    const fixedLength =
      typeof field.minItems === "number" &&
      typeof field.maxItems === "number" &&
      field.minItems === field.maxItems;

    const inputs = [];
    const rows = list.map((entry, idx) => {
      const input = h("input", {
        type: "text",
        list: listId,
        "data-array-index": String(idx),
        "aria-label": `${field.name}[${idx}]`,
        value: entry === undefined || entry === null ? "" : String(entry),
        onchange: () => {
          const next = [...list];
          next[idx] = input.value.trim();
          onCommit(next);
        },
      });
      inputs.push(input);
      const removeBtn =
        !fixedLength &&
        (!field.minItems || list.length > field.minItems) &&
        h(
          "button",
          {
            type: "button",
            class: "quiet-button",
            "data-array-action": "remove",
            "data-array-index": String(idx),
            "aria-label": `Remove ${field.name}[${idx}]`,
            onclick: () => {
              const next = list.filter((_, k) => k !== idx);
              onCommit(next.length === 0 && !field.required ? undefined : next);
            },
          },
          "×",
        );
      return h("div", { class: "inspector-array-row" }, input, removeBtn);
    });

    const canAdd = !fixedLength && (field.maxItems === undefined || list.length < field.maxItems);
    const addBtn =
      canAdd &&
      h(
        "button",
        {
          type: "button",
          class: "quiet-button inspector-array-add",
          "data-array-action": "add",
          onclick: () => {
            onCommit([...list, ""]);
          },
        },
        `+ Add ${field.name === "nodes" ? "node" : "item"}`,
      );

    return {
      element: h("div", { class: "inspector-array" }, ...rows, addBtn),
      canSync: (val) => (Array.isArray(val) ? val.length : 0) === inputs.length,
      sync: (val, active) => {
        list = Array.isArray(val) ? [...val] : [];
        inputs.forEach((input, idx) => {
          const next = list[idx] === undefined || list[idx] === null ? "" : String(list[idx]);
          if (input !== active && input.value !== next) input.value = next;
        });
      },
    };
  }

  renderSimulation(model) {
    const { source: sim, fields: effectiveFields, renderedNames } = this.simulationFieldSpecs(model);
    const controllers = new Map();
    const rowEls = effectiveFields.map((field) => {
      const ctrl = this.renderFieldRow(
        field,
        sim[field.name],
        this.diagnostics.forSimulationField(field.name),
        (nextVal) => this.commitEdit(updateSimulationField(this.text, field.name, nextVal)),
        `simulation.${field.name}`,
      );
      controllers.set(field.name, ctrl);
      return ctrl.rowEl;
    });

    const sectionDiags = this.diagnostics
      .forSection("simulation")
      .filter((d) => !d.field || !renderedNames.has(d.field));
    const sectionDiagEl = h("div", { class: "inspector-section-diagnostics" }, ...sectionDiags.map(diagnosticNode));

    this.renderedState = {
      selection: "simulation",
      key: "simulation",
      sectionDiagEl,
      headingEl: null,
      controllers,
    };

    return h(
      "div",
      { class: "inspector-section", "data-section": "simulation" },
      h("h3", { text: "Simulation" }),
      sectionDiagEl,
      ...rowEls,
    );
  }

  renderItem(model, section, index) {
    const spec = this.itemFieldSpecs(model, section, index);
    if (!spec) {
      return h("p", { class: "empty", text: `No item at ${section}[${index}].` });
    }
    const { item, catalogEntry, fields, renderedNames } = spec;
    const controllers = new Map();
    const rowEls = fields.map((field) => {
      const ctrl = this.renderFieldRow(
        field,
        item[field.name],
        this.diagnostics.forField(section, index, field.name),
        (nextVal) =>
          this.commitEdit(
            field.kind === "object" || field.kind === "array"
              ? updateItem(this.text, section, index, { [field.name]: nextVal })
              : setItemField(this.text, section, index, field.name, nextVal),
          ),
        `${section}[${index}].${field.name}`,
      );
      controllers.set(field.name, ctrl);
      return ctrl.rowEl;
    });

    const itemDiags = this.diagnostics
      .forItem(section, index)
      .filter((d) => !d.field || !renderedNames.has(d.field));
    const sectionDiagEl = h("div", { class: "inspector-section-diagnostics" }, ...itemDiags.map(diagnosticNode));

    const headingEl = h("h3", { text: `${item.name ?? `${section}[${index}]`} (${item.type ?? "unknown"})` });
    const headerChildren = [headingEl];
    if (catalogEntry?.summary) {
      headerChildren.push(h("p", { class: "description", text: catalogEntry.summary }));
    }
    if (catalogEntry?.signConvention) {
      headerChildren.push(h("p", { class: "hint", text: `Sign convention: ${catalogEntry.signConvention}` }));
    }

    this.renderedState = {
      selection: `${section}[${index}]`,
      key: `${section}[${index}]:${item.type ?? ""}`,
      sectionDiagEl,
      headingEl,
      controllers,
    };

    return h(
      "div",
      { class: "inspector-section", "data-section": `${section}[${index}]` },
      ...headerChildren,
      sectionDiagEl,
      ...rowEls,
    );
  }

  renderOutputs(model, candidates) {
    const outputs = Array.isArray(model.outputs) ? model.outputs : null;
    const outputTypes = getOutputTypes(this.catalogIndex);
    const componentNames = (Array.isArray(model.components) ? model.components : [])
      .map((c) => c?.name)
      .filter((n) => typeof n === "string" && n.length > 0);
    const componentList = h(
      "datalist",
      { id: `${this.uid}-components` },
      componentNames.map((n) => h("option", { value: n })),
    );

    const listNodes = [];
    if (outputs === null) {
      listNodes.push(
        h(
          "p",
          { class: "hint", text: "Outputs is omitted: all node voltages, branch currents, and signals are recorded. Add an output below to record specific quantities." },
        ),
      );
    } else if (outputs.length === 0) {
      listNodes.push(h("p", { class: "empty", text: "No outputs configured." }));
    } else {
      outputs.forEach((entry, index) => {
        const itemDiags = this.diagnostics.forItem("outputs", index);
        listNodes.push(
          h(
            "li",
            { class: "inspector-output-item", "data-output-index": String(index) },
            h(
              "div",
              { class: "inspector-output-row" },
              h("span", { class: "inspector-output-summary", text: formatOutputSummary(entry) }),
              h(
                "button",
                {
                  type: "button",
                  class: "quiet-button inspector-output-remove",
                  onclick: () => this.commitEdit(removeItem(this.text, "outputs", index)),
                },
                "Remove",
              ),
            ),
            ...itemDiags.map(diagnosticNode),
          ),
        );
      });
    }

    const sectionDiags = this.diagnostics.forSection("outputs").filter((d) => d.index === null);

    // Simple add output form
    const selectedType =
      outputTypes.find((o) => o.type === this.outputDraft.type) ?? outputTypes[0] ?? null;
    if (selectedType && this.outputDraft.type !== selectedType.type) {
      this.outputDraft = { type: selectedType.type, values: {} };
    }

    const typeSelect = h(
      "select",
      {
        class: "inspector-output-type",
        "aria-label": "Output type",
        onchange: () => {
          this.outputDraft = { type: typeSelect.value, values: {} };
          this.render();
        },
      },
      outputTypes.map((o) => h("option", { value: o.type, text: o.type })),
    );
    if (selectedType) typeSelect.value = selectedType.type;

    const addFormFields = (selectedType?.fields ?? []).map((field) => {
      let listId;
      if (field.name === "node") listId = `${this.uid}-nodes`;
      else if (field.name === "bus") listId = `${this.uid}-buses`;
      else if (field.name === "signal") listId = `${this.uid}-signals`;
      else if (field.name === "component") listId = `${this.uid}-components`;

      const initialVal = this.outputDraft.values[field.name] ?? "";
      const input = h("input", {
        type: "text",
        list: listId,
        "aria-label": `Output ${field.name}`,
        placeholder: field.name === "node" ? candidates.nodes[0] ?? "n1" : field.name,
        value: initialVal,
        oninput: () => {
          this.outputDraft.values[field.name] = input.value;
        },
        onchange: () => {
          this.outputDraft.values[field.name] = input.value;
        },
      });
      return h(
        "div",
        { class: "field inspector-field", "data-output-field": field.name },
        h("label", { text: `${field.name}${field.required ? " *" : ""}` }),
        input,
      );
    });

    const addForm = selectedType
      ? h(
          "div",
          { class: "inspector-output-add" },
          h("h4", { text: "Add output" }),
          h("div", { class: "field" }, h("label", { text: "type" }), typeSelect),
          ...addFormFields,
          h(
            "button",
            {
              type: "button",
              class: "quiet-button inspector-output-add-button",
              onclick: () => {
                const spec = { type: selectedType.type };
                for (const field of selectedType.fields) {
                  const val = (this.outputDraft.values[field.name] ?? "").trim();
                  if (val !== "") spec[field.name] = val;
                }
                const alreadyExists = Array.isArray(outputs) && outputs.some(
                  (o) =>
                    o &&
                    o.type === spec.type &&
                    Object.keys(spec).every((k) => o[k] === spec[k]) &&
                    Object.keys(o).every((k) => spec[k] !== undefined),
                );
                if (alreadyExists) return;
                const res = toggleOutputForTarget(this.text, spec);
                if (this.commitEdit(res)) {
                  this.outputDraft.values = {};
                }
              },
            },
            "+ Add output",
          ),
        )
      : null;

    return h(
      "div",
      { class: "inspector-section", "data-section": "outputs" },
      componentList,
      h("h3", { text: "Outputs" }),
      ...sectionDiags.map(diagnosticNode),
      outputs === null ? listNodes[0] : h("ul", { class: "inspector-outputs-list" }, ...listNodes),
      addForm,
    );
  }
}
