// The diagram in the page: the two faces (circuit, controls) of a model as
// SVG, with zoom, an overview, the keyboard, the links between the diagram
// and the text of the model, and the results of a run on it.
//
// The diagram is made from the document of `topology` alone (layout.js and
// render.js): positions are not stored anywhere, the same model gives the
// same picture. A run adds its values at one row and the states of the
// switches, diodes and faults there (overlay.js); the row is the cursor of
// the waveforms.

import { h } from "./dom.js";
import { formatValue } from "./format.js";
import { circuitFace, controlsFace, place } from "./layout.js";
import { extractLayoutOverrides, parseLayoutSidecar, serializeLayoutSidecar } from "./layout_sidecar.js";
import {
  PHASES,
  bindColumns,
  columnsOf,
  computeNominalScales,
  describeValues,
  frameAt,
  itemOwners,
  neighbouringEvent,
  rowAtOrBefore,
  slotsOf,
  stateTimeline,
} from "./overlay.js";
import { overviewSvg, stateClasses, toSvg } from "./render.js";

const FACES = ["circuit", "controls"];
const MIN_SCALE = 0.25;
const MAX_SCALE = 3;
/// The smallest scale "Fit" chooses: below it the names cannot be read.
const MIN_FIT = 0.75;
/// The largest: a small circuit is shown larger than drawn.
const MAX_FIT = 1.5;
/// The overview is at most this high, in px.
const OVERVIEW_HEIGHT = 110;
/// Minimum pointer travel (in screen px) before a press in layout edit mode
/// counts as a drag rather than a click.
const DRAG_THRESHOLD_PX = 3;

function roundCoord(value) {
  return Math.round(value * 100) / 100;
}

/// Moves focus onto `el` without scrolling when supported, falling back to
/// plain `focus()` in browsers that reject the options object.
function focusSafely(el) {
  if (!el?.focus) return;
  try {
    el.focus({ preventScroll: true });
  } catch {
    el.focus();
  }
}

export class Diagram {
  /// `root`: an empty element.
  /// `onSelect(path)`: a component or block was chosen in the diagram;
  /// `path` is its place in the model.
  /// `onRow(row)`: another row of the result was chosen in the diagram.
  /// `onColumns(names)`: a component, a block, a node, a bus or a signal
  /// with these columns in the result was pressed.
  /// `onSidecarChange(sidecar)`: layout overrides changed via drag, import, or reset.
  constructor(root, { onSelect = () => {}, onRow = () => {}, onColumns = () => {}, onSidecarChange = () => {} } = {}) {
    this.onSelect = onSelect;
    this.onRow = onRow;
    this.onColumns = onColumns;
    this.onSidecarChange = onSidecarChange;
    this.topology = null;
    this.faces = { circuit: null, controls: null };
    this.placed = { circuit: null, controls: null };
    this.baselinePlaced = null;
    this.sidecar = null;
    this.editLayout = false;
    this.expandThreePhase = false;
    this.showHeat = true;
    this.drag = null;
    this.suppressClick = false;
    this.svg = { circuit: "", controls: "" };
    this.face = "circuit";
    this.scale = 1;
    this.fitted = true;
    this.selected = null; // id of the item chosen
    this.cursorPath = null;
    this.lastMs = 0;
    // The results of a run: { table, columns, events, model }, and what is
    // made of them for the topology shown.
    this.result = null;
    this.binding = null;
    this.timeline = null;
    this.scales = null;
    this.row = -1;
    this.phase = "a";
    this.frame = null;
    this.slotElements = [];
    this.stateElements = [];
    this.heatElements = [];

    this.faceButtons = Object.fromEntries(
      FACES.map((face) => [
        face,
        h("button", { type: "button", class: "face-button", "aria-pressed": "false", onclick: () => this.showFace(face, true) }),
      ]),
    );
    const zoom = (label, title, action) => h("button", { type: "button", class: "quiet-button", title, "aria-label": title, onclick: action }, label);
    this.moveButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button layout-move-button",
        "aria-pressed": "false",
        title: "Move items and rails by dragging",
        onclick: () => this.setLayoutEditMode(!this.editLayout),
      },
      "Move",
    );
    this.exportLayoutButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button layout-export-button",
        title: "Export layout sidecar JSON",
        onclick: () => this.exportLayout(),
      },
      "Export layout",
    );
    this.importLayoutInput = h("input", {
      type: "file",
      accept: ".json",
      class: "layout-import-input",
      "aria-label": "Import layout",
      onchange: () => this.importLayoutFromInput(),
    });
    this.resetLayoutButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button layout-reset-button",
        title: "Reset layout overrides",
        onclick: () => this.resetSidecar({ tell: true }),
      },
      "Reset layout",
    );
    this.expandButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button diagram-expand-button",
        "aria-pressed": "false",
        title: "Expand three-phase buses into phase rails",
        onclick: () => this.setExpandThreePhase(!this.expandThreePhase),
      },
      "3-phase",
    );
    this.heatButton = h(
      "button",
      {
        type: "button",
        class: "quiet-button diagram-heat-button",
        "aria-pressed": "true",
        title: "Color buses and components by voltage and current magnitude",
        onclick: () => this.setShowHeat(!this.showHeat),
      },
      "Levels",
    );
    this.view = h("div", {
      class: "diagram-view",
      tabindex: "0",
      role: "group",
      "aria-label": "Diagram of the model. Tab or Enter moves to the components; the arrow keys move between them.",
    });
    this.empty = h("p", { class: "empty" });
    this.note = h("p", { class: "note", hidden: true });
    this.status = h("p", { class: "hint diagram-status", role: "status", "aria-live": "polite" });

    // The row of the result that the diagram shows.
    this.slider = h("input", {
      type: "range",
      class: "time-slider",
      min: "0",
      max: "0",
      step: "1",
      value: "0",
      "aria-label": "Time of the values in the diagram (row of the result)",
      oninput: () => this.setRow(Number(this.slider.value), true),
    });
    this.readout = h("output", { class: "time-readout" });
    this.previousEvent = h(
      "button",
      { type: "button", class: "quiet-button", title: "To the event before", "aria-label": "To the event before", onclick: () => this.toEvent(-1) },
      "◀ event",
    );
    this.nextEvent = h(
      "button",
      { type: "button", class: "quiet-button", title: "To the next event", "aria-label": "To the next event", onclick: () => this.toEvent(1) },
      "event ▶",
    );
    this.phaseSelect = h(
      "select",
      {
        class: "phase-select",
        "aria-label": "Phase of the three-phase values in the diagram",
        onchange: () => {
          this.phase = this.phaseSelect.value;
          this.showRow();
        },
      },
      PHASES.map((p) => h("option", { value: p, text: `phase ${p}` })),
    );
    this.timeBar = h(
      "div",
      { class: "diagram-time", role: "group", "aria-label": "Values of the run in the diagram", hidden: true },
      this.previousEvent,
      this.slider,
      this.nextEvent,
      this.phaseSelect,
      this.readout,
    );

    // The whole diagram in little, when it does not fit into its frame.
    this.overview = h("div", { class: "diagram-overview", hidden: true, title: "The whole diagram. Press or drag to move there." });

    root.append(
      h(
        "div",
        { class: "diagram-bar" },
        h("div", { class: "face-buttons", role: "group", "aria-label": "Face of the diagram" }, FACES.map((f) => this.faceButtons[f])),
        h(
          "div",
          { class: "zoom-buttons", role: "group", "aria-label": "Zoom" },
          zoom("−", "Zoom out (−)", () => this.zoomBy(1 / 1.25)),
          zoom("+", "Zoom in (+)", () => this.zoomBy(1.25)),
          zoom("Fit", "Fit the width (0)", () => this.fit()),
          zoom("1:1", "Full size (1)", () => this.zoomBy(1 / this.scale)),
        ),
        h(
          "div",
          { class: "layout-buttons", role: "group", "aria-label": "Layout and view" },
          this.expandButton,
          this.heatButton,
          this.moveButton,
          this.exportLayoutButton,
          h("label", { class: "quiet-button layout-import-button" }, "Import layout", this.importLayoutInput),
          this.resetLayoutButton,
        ),
      ),
      this.timeBar,
      this.note,
      this.empty,
      this.overview,
      this.view,
      this.status,
    );

    this.view.addEventListener("click", (event) => {
      if (this.suppressClick) {
        this.suppressClick = false;
        return;
      }
      this.activate(event.target);
    });
    this.view.addEventListener("pointerdown", (event) => this.onViewPointerDown(event));
    this.view.addEventListener("pointermove", (event) => this.onViewPointerMove(event));
    this.view.addEventListener("pointerup", (event) => this.onViewPointerEnd(event, true));
    this.view.addEventListener("pointercancel", (event) => this.onViewPointerEnd(event, false));
    this.view.addEventListener("keydown", (event) => this.onKey(event));
    this.view.addEventListener("focusin", (event) => {
      const target = event.target.closest?.("[data-item], .chip, .has-columns");
      if (target) this.setCurrent(target);
    });
    this.view.addEventListener("scroll", () => this.showViewport(), { passive: true });
    this.view.addEventListener(
      "wheel",
      (event) => {
        if (!event.ctrlKey && !event.metaKey) return;
        event.preventDefault();
        this.zoomBy(event.deltaY < 0 ? 1.1 : 1 / 1.1);
      },
      { passive: false },
    );
    let draggingOverview = false;
    const moveTo = (event) => {
      if (event.type === "pointermove" && !draggingOverview && !(event.buttons & 1)) return;
      this.scrollToOverview(event);
    };
    const endOverview = (event) => {
      draggingOverview = false;
      try {
        if (this.overview.hasPointerCapture?.(event.pointerId)) {
          this.overview.releasePointerCapture?.(event.pointerId);
        }
      } catch {
        // Ignore missing capture in synthetic events or browsers without support.
      }
    };
    this.overview.addEventListener("pointerdown", (event) => {
      if (event.pointerType === "mouse" && event.button !== undefined && event.button !== 0) return;
      draggingOverview = true;
      try {
        this.overview.setPointerCapture?.(event.pointerId);
      } catch {
        // No such pointer (an event made by a script): the press still moves.
      }
      moveTo(event);
    });
    this.overview.addEventListener("pointermove", moveTo);
    this.overview.addEventListener("pointerup", endOverview);
    this.overview.addEventListener("pointercancel", endOverview);
    if (typeof ResizeObserver === "function") {
      new ResizeObserver(() => {
        if (this.fitted) this.fit();
        else this.showViewport();
      }).observe(this.view);
    }
    this.setTopology(null, "The diagram appears when the model can be read.");
  }

  /// Shows the model of a `topology` document; without one (the text is not
  /// a model), `message` in its place. `result`: the run of this very model
  /// ({ table, columns, events, model }), or null.
  setTopology(topology, message = "", result = null) {
    const started = performance.now();
    const usable = topology && Array.isArray(topology.components);
    this.topology = usable ? topology : null;
    this.result = usable ? result : null;
    if (!usable) {
      this.baselinePlaced = null;
      this.cancelDrag();
      this.view.hidden = true;
      this.note.hidden = true;
      this.overview.hidden = true;
      this.timeBar.hidden = true;
      this.empty.hidden = false;
      this.empty.textContent = message || "The text is not a model yet: the diagram returns when it can be read.";
      this.status.textContent = "";
      for (const face of FACES) this.faceButtons[face].textContent = face === "circuit" ? "Circuit" : "Controls";
      return;
    }
    this.expandButton.disabled = topology.buses.length === 0;
    this.faces = {
      circuit: circuitFace(topology, { expandThreePhase: this.expandThreePhase }),
      controls: controlsFace(topology),
    };
    this.build();
    const blocks = this.placed.controls.items.length;
    this.faceButtons.circuit.textContent = `Circuit (${topology.components.length.toLocaleString("en-US")})`;
    this.faceButtons.controls.textContent = `Controls (${blocks.toLocaleString("en-US")})`;
    this.faceButtons.controls.disabled = blocks === 0;
    if (blocks === 0) this.face = "circuit";
    this.view.hidden = false;
    this.empty.hidden = true;
    this.draw();
    this.lastMs = performance.now() - started;
  }

  /// The run of the model shown, or null to take the values away again.
  setResult(result) {
    if (!this.topology || result === this.result) return;
    const started = performance.now();
    this.result = result;
    this.build();
    this.draw();
    this.lastMs = performance.now() - started;
  }

  /// Places both faces, with room for the values of the result.
  build() {
    const result = this.result;
    this.binding = result ? bindColumns(this.topology, result.columns, result.table.header) : null;
    this.timeline = result ? stateTimeline(this.topology, result.model, result.events) : null;
    this.scales = result
      ? computeNominalScales(this.binding, result.table, { model: result.model, summary: result.summary })
      : null;
    const slots = slotsOf(this.binding, this.timeline);
    this.baselinePlaced = { circuit: null, controls: null };
    for (const face of FACES) {
      const baseline = place(this.faces[face], { slots });
      this.baselinePlaced[face] = baseline;
      this.placed[face] = this.sidecar ? place(this.faces[face], { slots, sidecar: this.sidecar }) : baseline;
      this.svg[face] = toSvg(this.placed[face], face === "circuit" ? "Circuit diagram" : "Block diagram of the controls");
    }
    const rows = result ? result.table.rows : 0;
    // Another run starts at its first row.
    if (result !== this.builtFor) this.row = 0;
    this.builtFor = result;
    this.timeBar.hidden = rows === 0;
    if (rows === 0) {
      this.row = -1;
      this.frame = null;
      return;
    }
    this.slider.max = String(rows - 1);
    this.row = Math.min(rows - 1, Math.max(0, this.row));
    const phased = this.binding.entries.some((entry) => PHASES.includes(entry.phase));
    this.phaseSelect.hidden = !phased;
    const anyEvent = this.timeline.times.length > 0;
    this.previousEvent.hidden = !anyEvent;
    this.nextEvent.hidden = !anyEvent;
  }

  // ---- layout sidecar and drag editing ----

  setSidecar(sidecar, { tell = false } = {}) {
    if (sidecar === null || sidecar === undefined) {
      this.sidecar = null;
    } else {
      const parsed = parseLayoutSidecar(sidecar);
      if (!parsed.valid) {
        throw new Error(parsed.errors.join("; "));
      }
      this.sidecar = parsed.layout;
    }
    if (this.topology) {
      this.build();
      this.draw();
    }
    if (tell) this.onSidecarChange(this.getSidecar());
    return this.getSidecar();
  }

  getSidecar() {
    return this.sidecar ? structuredClone(this.sidecar) : null;
  }

  resetSidecar({ tell = true } = {}) {
    this.sidecar = null;
    if (this.topology) {
      this.build();
      this.draw();
    }
    if (tell) this.onSidecarChange(null);
  }

  setLayoutEditMode(enabled) {
    this.editLayout = Boolean(enabled);
    this.moveButton.setAttribute("aria-pressed", String(this.editLayout));
    this.view.classList.toggle("layout-editing", this.editLayout);
    if (!this.editLayout) this.cancelDrag();
  }

  setExpandThreePhase(enabled) {
    const next = Boolean(enabled);
    if (next === this.expandThreePhase) return;
    this.expandThreePhase = next;
    this.expandButton.setAttribute("aria-pressed", String(this.expandThreePhase));
    if (!this.topology) return;
    this.faces = {
      circuit: circuitFace(this.topology, { expandThreePhase: this.expandThreePhase }),
      controls: controlsFace(this.topology),
    };
    this.build();
    this.draw();
  }

  setShowHeat(enabled) {
    this.showHeat = Boolean(enabled);
    this.heatButton.setAttribute("aria-pressed", String(this.showHeat));
    this.showRow();
  }

  exportLayout() {
    const text = serializeLayoutSidecar(this.sidecar ?? {});
    if (typeof document !== "undefined" && typeof URL !== "undefined" && typeof URL.createObjectURL === "function") {
      const blob = new Blob([text], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const link = h("a", { href: url, download: "layout.json" });
      link.click?.();
      setTimeout(() => URL.revokeObjectURL(url), 0);
    }
    return text;
  }

  async importLayoutFromInput() {
    const file = this.importLayoutInput.files?.[0];
    if (!file) return;
    try {
      const text = typeof file.text === "function" ? await file.text() : String(file.content ?? "");
      const parsed = parseLayoutSidecar(text);
      if (!parsed.valid) {
        this.status.textContent = `Could not load layout: ${parsed.errors[0]}`;
        return;
      }
      this.setSidecar(parsed.layout, { tell: true });
      this.status.textContent = "Loaded layout overrides.";
    } catch (err) {
      this.status.textContent = `Could not load layout: ${err instanceof Error ? err.message : String(err)}`;
    } finally {
      this.importLayoutInput.value = "";
    }
  }

  svgScale() {
    const matrix = this.element?.getScreenCTM?.();
    if (
      matrix &&
      Number.isFinite(matrix.a) &&
      Number.isFinite(matrix.d) &&
      Math.abs(matrix.a) > 1e-12 &&
      Math.abs(matrix.d) > 1e-12
    ) {
      return { x: matrix.a, y: matrix.d };
    }
    const fallback = this.scale > 0 ? this.scale : 1;
    return { x: fallback, y: fallback };
  }

  cancelDrag() {
    if (!this.drag) return;
    this.drag.el.classList.remove("dragging");
    this.drag.el.removeAttribute?.("transform");
    this.drag = null;
  }

  onViewPointerDown(event) {
    this.suppressClick = false;
    if (!this.editLayout || !this.topology || !this.element) return;
    if (event.pointerType === "mouse" && event.button !== undefined && event.button !== 0) return;
    if (event.target?.closest?.(".chip")) return;
    const itemEl = event.target?.closest?.(".item[data-item]");
    const railEl = itemEl ? null : event.target?.closest?.(".rail[data-rail]");
    const el = itemEl ?? railEl;
    if (!el) return;
    const kind = itemEl ? "item" : "rail";
    const id = itemEl ? itemEl.dataset.item : railEl.dataset.rail;
    if (!id) return;
    this.drag = {
      kind,
      id,
      el,
      pointerId: event.pointerId ?? null,
      startClientX: event.clientX ?? 0,
      startClientY: event.clientY ?? 0,
      lastClientX: event.clientX ?? 0,
      lastClientY: event.clientY ?? 0,
      moved: false,
    };
    try {
      this.view.setPointerCapture?.(event.pointerId);
    } catch {
      // Ignore synthetic events or browsers without pointer capture support.
    }
  }

  onViewPointerMove(event) {
    if (!this.drag) return;
    if (this.drag.pointerId !== null && event.pointerId !== undefined && event.pointerId !== this.drag.pointerId) return;
    if (event.pointerType === "mouse" && event.buttons === 0) return;
    const clientX = event.clientX ?? this.drag.startClientX;
    const clientY = event.clientY ?? this.drag.startClientY;
    this.drag.lastClientX = clientX;
    this.drag.lastClientY = clientY;
    const rawDx = clientX - this.drag.startClientX;
    const rawDy = clientY - this.drag.startClientY;
    if (!this.drag.moved && Math.hypot(rawDx, rawDy) < DRAG_THRESHOLD_PX) return;
    this.drag.moved = true;
    const scale = this.svgScale();
    const dxSvg = roundCoord(rawDx / scale.x);
    const dySvg = this.drag.kind === "rail" ? 0 : roundCoord(rawDy / scale.y);
    this.drag.el.classList.add("dragging");
    this.drag.el.setAttribute("transform", `translate(${dxSvg} ${dySvg})`);
  }

  onViewPointerEnd(event, commit) {
    if (!this.drag) return;
    if (this.drag.pointerId !== null && event.pointerId !== undefined && event.pointerId !== this.drag.pointerId) return;
    const drag = this.drag;
    this.drag = null;
    try {
      if (this.view.hasPointerCapture?.(event.pointerId)) {
        this.view.releasePointerCapture?.(event.pointerId);
      }
    } catch {
      // Ignore missing capture.
    }
    drag.el.classList.remove("dragging");
    drag.el.removeAttribute?.("transform");
    if (!commit) return;

    const clientX = event.clientX ?? drag.lastClientX;
    const clientY = event.clientY ?? drag.lastClientY;
    const rawDx = clientX - drag.startClientX;
    const rawDy = clientY - drag.startClientY;
    if (!drag.moved && Math.hypot(rawDx, rawDy) < DRAG_THRESHOLD_PX) return;

    this.suppressClick = true;
    setTimeout(() => {
      this.suppressClick = false;
    }, 0);

    const scale = this.svgScale();
    const dxSvg = roundCoord(rawDx / scale.x);
    const dySvg = drag.kind === "rail" ? 0 : roundCoord(rawDy / scale.y);
    this.applyDragDelta(drag.kind, drag.id, dxSvg, dySvg);
  }

  applyDragDelta(kind, id, dxSvg, dySvg) {
    if (!this.topology || !this.baselinePlaced) return;
    const currentFace = this.placed[this.face];
    if (!currentFace) return;
    const nextFace = {
      ...currentFace,
      rails: currentFace.rails.map((r) => (kind === "rail" && r.id === id ? { ...r, x: roundCoord(r.x + dxSvg) } : r)),
      items: currentFace.items.map((i) =>
        kind === "item" && i.id === id ? { ...i, x: roundCoord(i.x + dxSvg), y: roundCoord(i.y + dySvg) } : i,
      ),
    };
    const nextSidecar = extractLayoutOverrides({ ...this.placed, [this.face]: nextFace }, this.baselinePlaced);
    this.sidecar = nextSidecar;
    this.build();
    this.draw();
    this.onSidecarChange(this.getSidecar());
  }

  /// Puts the SVG of the current face into the page.
  draw() {
    const scroll = { left: this.view.scrollLeft, top: this.view.scrollTop };
    // The text is made by render.js from escaped values: no markup of the
    // model gets into the page.
    this.view.innerHTML = this.svg[this.face];
    this.element = this.view.firstElementChild;
    for (const face of FACES) this.faceButtons[face].setAttribute("aria-pressed", String(face === this.face));
    const placed = this.placed[this.face];
    this.note.hidden = placed.note === "";
    this.note.textContent = placed.note;
    this.overview.innerHTML = overviewSvg(placed);
    this.viewport = this.overview.querySelector(".viewport");
    // What a frame of the result fills in.
    this.slotElements = [...this.element.querySelectorAll("[data-slot]")];
    const stateful = new Set(placed.items.filter((item) => item.stateful).map((item) => item.id));
    this.stateElements = [...this.element.querySelectorAll("[data-item]")].filter((el) => stateful.has(el.dataset.item));
    this.heatElements = [...this.element.querySelectorAll("[data-item], [data-rail]")];
    if (this.fitted) this.fit();
    else this.setScale(this.scale);
    this.view.scrollLeft = scroll.left;
    this.view.scrollTop = scroll.top;
    this.mark();
    // One stop for the Tab key; the arrow keys move from there.
    const current = this.currentStop();
    if (current) current.setAttribute("tabindex", "0");
    this.showRow();
  }

  showFace(face, focus = false) {
    if (!this.topology || face === this.face || this.faceButtons[face].disabled) return;
    this.face = face;
    this.view.scrollLeft = 0;
    this.view.scrollTop = 0;
    this.draw();
    if (focus) this.faceButtons[face].focus();
  }

  // ---- the results of a run ----

  /// Shows the row `row` of the result: its values, and the states the row
  /// was computed with. `tell`: the row was chosen here, tell `onRow`.
  setRow(row, tell = false) {
    if (!this.result) return;
    const rows = this.result.table.rows;
    const next = Math.min(rows - 1, Math.max(0, Math.round(row)));
    if (!(next >= 0)) return;
    if (next !== this.row) {
      this.row = next;
      this.showRow();
    }
    if (tell) this.onRow(next);
  }

  /// Writes the frame of the current row into the SVG that is in the page:
  /// the same texts and classes that render.js writes for that frame.
  showRow() {
    if (!this.result || this.row < 0 || !this.element) {
      this.frame = null;
      return;
    }
    const { table } = this.result;
    const frame = frameAt({ binding: this.binding, timeline: this.timeline, table, row: this.row, phase: this.phase, scales: this.scales });
    this.frame = frame;
    for (const el of this.slotElements) {
      const text = frame.texts.get(el.dataset.slot) ?? "";
      if (el.textContent !== text) el.textContent = text;
    }
    for (const el of this.stateElements) {
      const classes = stateClasses(frame.states.get(el.dataset.item));
      for (const name of ["state-on", "state-off", "acting"]) el.classList.toggle(name, classes.includes(name));
    }
    for (const el of this.heatElements) {
      const ownerId = el.dataset.item ?? el.dataset.rail;
      const level = this.showHeat && ownerId ? frame.levels?.get(ownerId)?.level ?? null : null;
      if (level) el.setAttribute("data-heat", level);
      else el.removeAttribute?.("data-heat");
    }
    if (Number(this.slider.value) !== this.row) this.slider.value = String(this.row);
    const acting = [...frame.states].filter(([, state]) => state.acting).map(([id, state]) => `${id.slice(id.indexOf(":") + 1)}: ${state.word}`);
    this.readout.textContent =
      `t = ${formatValue(frame.time)} s (row ${(this.row + 1).toLocaleString("en-US")} of ${table.rows.toLocaleString("en-US")})` +
      (acting.length > 0 ? `. ${acting.slice(0, 3).join("; ")}${acting.length > 3 ? `; and ${acting.length - 3} more` : ""}` : "");
    const item = this.selected === null ? null : this.placed[this.face].items.find((i) => i.id === this.selected);
    if (item) this.status.textContent = this.describe(item);
  }

  /// To the row at which the next event (`direction` 1) or the one before
  /// (-1) is about to act.
  toEvent(direction) {
    if (!this.result) return;
    const time = this.result.table.columns[0];
    const at = neighbouringEvent(this.timeline, time, this.row, direction);
    if (at !== null) this.setRow(rowAtOrBefore(time, at), true);
  }

  /// Shows the instant `time_s` (the row at which an event at that time is
  /// about to act) and the component `name` of an event ("F1.a" is F1).
  showEvent(time_s, name) {
    if (!this.result || !this.topology) return;
    this.setRow(rowAtOrBefore(this.result.table.columns[0], time_s), true);
    const component = this.topology.components.find((c) => c.name === name || name.startsWith(`${c.name}.`));
    if (!component) return;
    if (this.face !== "circuit") {
      this.face = "circuit";
      this.draw();
    }
    this.select(`component:${component.name}`, { scroll: true, tell: false });
  }

  /// The owners of columns that an item stands for: a component itself, a
  /// block the signals it gives, or the members of a reduced line.
  ownersOf(item) {
    return itemOwners(this.placed[this.face], item);
  }

  // ---- zoom and the overview ----

  setScale(scale) {
    if (!this.element) return;
    this.scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
    const placed = this.placed[this.face];
    this.element.setAttribute("width", String(Math.round(placed.width * this.scale)));
    this.element.setAttribute("height", String(Math.round(placed.height * this.scale)));
    this.showViewport();
  }

  zoomBy(factor) {
    this.fitted = false;
    this.setScale(this.scale * factor);
  }

  fit() {
    if (!this.element) return;
    this.fitted = true;
    const room = this.view.clientWidth - 4;
    const placed = this.placed[this.face];
    this.setScale(room > 0 ? Math.max(MIN_FIT, Math.min(MAX_FIT, room / placed.width)) : 1);
  }

  /// The overview is there while the diagram is larger than its frame by
  /// more than a quarter, in width or in height;
  /// its rectangle is what the frame shows.
  showViewport() {
    const view = this.view;
    const placed = this.placed[this.face];
    if (!this.element || !this.viewport || !placed) return;
    // Not for a diagram that is only a little larger than its frame.
    const scrolls = view.scrollWidth > view.clientWidth * 1.25 || view.scrollHeight > view.clientHeight * 1.25;
    if (this.overview.hidden === scrolls) {
      this.overview.hidden = !scrolls;
    }
    if (!scrolls) return;
    const width = this.overview.clientWidth;
    if (width > 0) {
      const height = Math.round(Math.min(OVERVIEW_HEIGHT, Math.max(28, (width * placed.height) / placed.width)));
      // Set through the object model: the page allows no inline styles.
      if (this.overview.style.height !== `${height}px`) this.overview.style.height = `${height}px`;
    }
    const s = this.scale;
    const set = (name, value) => this.viewport.setAttribute(name, String(Math.round(value * 10) / 10));
    set("x", view.scrollLeft / s);
    set("y", view.scrollTop / s);
    set("width", Math.min(placed.width, view.clientWidth / s));
    set("height", Math.min(placed.height, view.clientHeight / s));
  }

  /// Scrolls the diagram so that the place pressed in the overview is in
  /// the middle of the frame.
  scrollToOverview(event) {
    const svg = this.overview.firstElementChild;
    const placed = this.placed[this.face];
    if (!svg || !placed || !(placed.width > 0) || !(placed.height > 0)) return;
    let at = null;
    const matrix = svg.getScreenCTM?.();
    if (matrix && Math.abs(matrix.a * matrix.d - matrix.b * matrix.c) > 1e-12 && typeof DOMPoint === "function") {
      try {
        at = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
      } catch {
        at = null;
      }
    }
    if (!at) {
      const rect = svg.getBoundingClientRect?.();
      if (!rect || !(rect.width > 0) || !(rect.height > 0)) return;
      const k = Math.min(rect.width / placed.width, rect.height / placed.height);
      if (!(k > 0)) return;
      const offsetX = rect.left + (rect.width - placed.width * k) / 2;
      const offsetY = rect.top + (rect.height - placed.height * k) / 2;
      at = {
        x: (event.clientX - offsetX) / k,
        y: (event.clientY - offsetY) / k,
      };
    }
    if (!Number.isFinite(at.x) || !Number.isFinite(at.y)) return;
    this.view.scrollLeft = at.x * this.scale - this.view.clientWidth / 2;
    this.view.scrollTop = at.y * this.scale - this.view.clientHeight / 2;
  }

  // ---- what is chosen ----

  stops() {
    return this.element ? [...this.element.querySelectorAll("[data-item], .chip, .has-columns")] : [];
  }

  currentStop() {
    const stops = this.stops();
    if (stops.length === 0) return null;
    return (
      stops.find((el) => this.selected !== null && el.dataset?.item === this.selected) ??
      stops.find((el) => el.classList?.contains("selected")) ??
      stops.find((el) => el.getAttribute?.("tabindex") === "0") ??
      stops[0]
    );
  }

  setCurrent(target) {
    for (const el of this.element.querySelectorAll('[tabindex="0"]')) el.setAttribute("tabindex", "-1");
    target.setAttribute("tabindex", "0");
  }

  /// Sets the classes of the item chosen in the diagram and of the one the
  /// cursor of the editor is in.
  mark() {
    if (!this.element) return;
    for (const el of this.element.querySelectorAll(".selected, .at-cursor")) el.classList.remove("selected", "at-cursor");
    for (const el of this.element.querySelectorAll("[data-item]")) {
      if (el.dataset.item === this.selected) el.classList.add("selected");
      if (this.cursorPath !== null && el.dataset.path === this.cursorPath) el.classList.add("at-cursor");
    }
  }

  describe(item) {
    const words = { circuit: item.type, control: `${item.type} block`, owner: `control blocks of ${item.name}`, sink: `${item.type}, in the circuit`, reduced: item.title };
    const nets = item.ports.flatMap((p) => [...p.rails.map((r) => this.placed[this.face].rails[r.rail].name + (r.tag ? ` (${r.tag})` : "")), ...(p.ground ? ["gnd"] : [])]);
    const state = this.frame?.states.get(item.id);
    const values = this.ownersOf(item)
      .map((owner) => describeValues(this.binding, this.result?.table, this.row, owner))
      .filter((text) => text !== "");
    return (
      `${item.name}: ${words[item.category]}, ${item.path}` +
      (nets.length > 0 ? `. Connected to ${[...new Set(nets)].join(", ")}.` : ".") +
      (state ? ` State: ${state.word}.` : "") +
      (values.length > 0 ? ` ${values.join(", ")}.` : "")
    );
  }

  select(id, { scroll = false, tell = true } = {}) {
    this.selected = id;
    this.mark();
    const item = this.placed[this.face]?.items.find((i) => i.id === id);
    const el = this.element?.querySelector(".item.selected");
    if (!item || !el) return;
    this.setCurrent(el);
    if (scroll) el.scrollIntoView({ block: "nearest", inline: "nearest" });
    this.status.textContent = this.describe(item);
    if (tell) this.onSelect(item.path);
  }

  /// A press on a component, a block, a link to the other face, or a node,
  /// bus or signal that has columns in the result.
  activate(target) {
    const chip = target.closest?.(".chip");
    if (chip) {
      this.follow(chip.dataset);
      return;
    }
    const element = target.closest?.("[data-item]");
    if (element) {
      this.select(element.dataset.item);
      const item = this.placed[this.face].items.find((i) => i.id === element.dataset.item);
      const names = item ? this.ownersOf(item).flatMap((owner) => columnsOf(this.binding, owner)) : [];
      if (names.length > 0) {
        this.status.textContent += " Its columns are switched on in the waveforms.";
        this.onColumns([...new Set(names)]);
      }
      return;
    }
    const rail = target.closest?.(".has-columns");
    if (rail) {
      const names = columnsOf(this.binding, rail.dataset.rail);
      for (const el of this.element.querySelectorAll(".rail.selected")) el.classList.remove("selected");
      rail.classList.add("selected");
      this.setCurrent(rail);
      this.selected = null;
      this.mark();
      this.status.textContent =
        `${rail.getAttribute("aria-label") ?? ""}: ${describeValues(this.binding, this.result?.table, this.row, rail.dataset.rail)}.` +
        " Its columns are switched on in the waveforms.";
      if (names.length > 0) this.onColumns(names);
    }
  }

  /// Goes to what a link names on the other face.
  follow({ face, control, signal, owner, component, node, bus }) {
    if (!this.topology || !FACES.includes(face)) return;
    let id = null;
    let rail = null;
    if (face === "controls") {
      const block = control ?? this.topology.signals.find((s) => s.name === signal)?.from ?? null;
      const giver = block === null ? null : this.topology.controls.find((c) => c.name === block);
      if (owner !== undefined) id = `owner:${owner}`;
      else if (giver) id = giver.owner === undefined ? `control:${giver.name}` : `owner:${giver.owner}`;
      else if (signal !== undefined) rail = `signal:${signal}`;
    } else if (component !== undefined) {
      id = `component:${component}`;
    } else if (bus !== undefined) {
      rail = `bus:${bus}`;
      if (this.expandThreePhase && !this.placed[face]?.rails.some((r) => r.id === rail)) {
        rail = `node:${bus}.a`;
      }
    } else if (node !== undefined) {
      const of = this.topology.nodes.find((x) => x.name === node);
      if (this.expandThreePhase && this.placed[face]?.rails.some((r) => r.id === `node:${node}`)) {
        rail = `node:${node}`;
      } else {
        rail = of?.bus !== undefined ? `bus:${of.bus}` : `node:${node}`;
      }
    }
    this.face = face;
    this.selected = null;
    this.draw();
    if (id !== null && this.placed[face].items.some((i) => i.id === id)) {
      this.select(id, { scroll: true });
      focusSafely(this.element.querySelector(".item.selected"));
      return;
    }
    const target = rail === null ? null : [...this.element.querySelectorAll("[data-rail]")].find((el) => el.dataset.rail === rail);
    if (target) {
      target.classList.add("selected");
      target.scrollIntoView({ block: "nearest", inline: "center" });
      this.status.textContent = target.querySelector("title")?.textContent ?? "";
    }
    focusSafely(this.view);
  }

  /// Shows the component or block at `path` ("components[3]"), where the
  /// cursor of the editor is; null when it is in none.
  showPath(path) {
    if (path === this.cursorPath) return;
    this.cursorPath = path;
    if (!this.topology || path === null) {
      this.mark();
      return;
    }
    // The face that has it: a block on the controls, a component on the
    // circuit.
    const wanted = path.startsWith("controls[") ? "controls" : "circuit";
    if (wanted !== this.face && this.placed[wanted].items.some((i) => i.path === path)) {
      this.face = wanted;
      this.draw();
    } else {
      this.mark();
    }
    const el = this.element?.querySelector(".at-cursor");
    if (el) {
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
      const item = this.placed[this.face].items.find((i) => i.id === el.dataset.item);
      if (item) this.status.textContent = this.describe(item);
    }
  }

  // ---- keyboard ----

  onKey(event) {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target?.closest?.("[data-item], .chip, .has-columns") ?? null;
    if (event.key === "+" || event.key === "=") this.zoomBy(1.25);
    else if (event.key === "-") this.zoomBy(1 / 1.25);
    else if (event.key === "0") this.fit();
    else if (event.key === "1") this.zoomBy(1 / this.scale);
    else if (this.result && (event.key === "," || event.key === ".")) this.setRow(this.row + (event.key === "," ? -1 : 1), true);
    else if (this.result && (event.key === "[" || event.key === "]")) this.toEvent(event.key === "[" ? -1 : 1);
    else if (target && (event.key === "Enter" || event.key === " ")) this.activate(target);
    else if (["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)) {
      const stops = this.stops();
      if (stops.length === 0) return;
      let next;
      if (target) {
        const at = stops.indexOf(target);
        const to = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: stops.length - 1 }[event.key];
        next = stops[Math.min(stops.length - 1, Math.max(0, to))];
      } else {
        // Focus is on the diagram container itself: enter at the current stop (or first/last).
        if (event.key === "Home") next = stops[0];
        else if (event.key === "End") next = stops[stops.length - 1];
        else next = this.currentStop();
      }
      if (!next) return;
      this.setCurrent(next);
      focusSafely(next);
      next.scrollIntoView?.({ block: "nearest", inline: "nearest" });
      if (!target && (event.key === "Enter" || event.key === " ") && typeof document !== "undefined" && document.activeElement !== next) {
        this.activate(next);
      }
    } else return;
    event.preventDefault();
  }
}
