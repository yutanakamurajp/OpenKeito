// Line charts on a canvas: several panels (one per unit) that share the
// horizontal axis, with columns to choose, zoom, pan and a cursor that reads
// the values.
//
// A result can have 100 000 rows and more. Before drawing, each column is
// reduced to what the pixels can show: per pixel column the first, lowest,
// highest and last value of the rows that fall into it. A redraw is then one
// pass over the rows in view per visible column (a few milliseconds for
// 100 000 rows), and the peaks stay.

import { decimate, toLog10 } from "./decimate.js";
import { h } from "./dom.js";
import { formatValue, logTicks, niceTicks, siScale, tickLabels, withPrefix } from "./format.js";

/// The categorical colors (--series-1 .. --series-8 in style.css). A panel
/// with more columns goes round again with a dashed line.
const SLOTS = 8;
const DASHES = [[], [6, 3], [2, 3]];

/// The space around the plot area, in CSS pixels.
const MARGIN = { left: 58, right: 14, top: 20, bottom: 24 };

const FONT = '11px system-ui, -apple-system, "Segoe UI", sans-serif';

/// The scale taken out of the tick labels of a linear axis. Values from 0.1
/// to 1000 stay as they are (0.5 reads better than 500 m).
function axisScale(maxAbs) {
  return maxAbs >= 0.1 && maxAbs < 1000 ? { exponent: 0, prefix: "" } : siScale(maxAbs);
}

function unitLabel(scale, unit) {
  if (unit) return `${scale.prefix}${unit}`;
  return scale.exponent === 0 ? "" : `×1e${scale.exponent}`;
}

function linearOrLogTicks(isLog, lo, hi, target, unit) {
  if (isLog) {
    const log = logTicks(lo, hi, target);
    if (log) return { ticks: log, labels: log.map((t) => withPrefix(10 ** t)), unit };
    const linear = niceTicks(10 ** lo, 10 ** hi, target).ticks.filter((t) => t > 0);
    return { ticks: linear.map(toLog10), labels: linear.map(withPrefix), unit };
  }
  const { ticks, step } = niceTicks(lo, hi, target);
  const scale = axisScale(Math.max(Math.abs(lo), Math.abs(hi)));
  return { ticks, labels: tickLabels(ticks, step, scale), unit: unitLabel(scale, unit) };
}

export class Chart {
  /// `root`: an empty element the chart fills.
  constructor(root) {
    this.root = root;
    root.classList.add("chart");
    this.panels = [];
    this.tx = new Float64Array(0);
    this.x = this.tx;
    this.full = [0, 1];
    this.view = [0, 1];
    this.cursor = -1;
    this.markers = new Float64Array(0);
    this.drag = null;
    this.frame = 0;

    const button = (label, title, action) =>
      h("button", { type: "button", class: "chart-button", title, "aria-label": title, onclick: action }, label);
    this.readout = h("output", { class: "chart-readout" });
    this.toolbar = h(
      "div",
      { class: "chart-toolbar", role: "toolbar", "aria-label": "Chart view" },
      button("+", "Zoom in", () => this.zoomBy(0.5)),
      button("−", "Zoom out", () => this.zoomBy(2)),
      button("◀", "Move left", () => this.panBy(-0.25)),
      button("▶", "Move right", () => this.panBy(0.25)),
      button("Reset", "Show everything", () => this.reset()),
      this.readout,
    );
    this.body = h("div", { class: "chart-panels" });
    this.help = h("p", {
      class: "chart-help",
      text:
        "Drag to zoom, Shift+drag to move, Ctrl+wheel or pinch to zoom, double-click to reset. " +
        "With the keyboard: ← → read values, Shift+← → move, + − zoom, 0 reset.",
    });
    root.append(this.toolbar, this.body, this.help);

    if (typeof ResizeObserver === "function") {
      new ResizeObserver(() => this.requestDraw()).observe(this.body);
    }
    // The colors come from the style sheet: draw again when the scheme changes.
    if (typeof matchMedia === "function") {
      matchMedia("(prefers-color-scheme: dark)").addEventListener?.("change", () => this.requestDraw());
    }
    if (typeof MutationObserver === "function" && typeof document !== "undefined") {
      new MutationObserver(() => this.requestDraw()).observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme"],
      });
    }
  }

  /// Shows new data.
  ///
  /// x:       { data, label, unit, log }
  /// panels:  [{ title, unit, log, series: [{ name, data, visible }] }]
  /// markers: values of x to mark with a vertical line (events)
  /// onVisibility(name, visible): a column was switched on or off
  /// onCursor(index): the cursor was put on another row from within the
  ///   chart (-1: on none)
  setData({ x, panels, markers = [], onVisibility = null, onCursor = null }) {
    this.onVisibility = onVisibility;
    this.onCursor = onCursor;
    this.x = x.data;
    this.xLabel = x.label;
    this.xUnit = x.unit ?? "";
    this.xLog = Boolean(x.log);
    this.tx = this.xLog ? Float64Array.from(x.data, toLog10) : x.data;
    const n = this.tx.length;
    let lo = n > 0 ? this.tx[0] : 0;
    let hi = n > 0 ? this.tx[n - 1] : 1;
    if (!(hi > lo)) {
      lo = (Number.isFinite(lo) ? lo : 0) - 0.5;
      hi = lo + 1;
    }
    this.full = [lo, hi];
    this.view = [lo, hi];
    this.cursor = -1;
    this.drag = null;
    this.markers = Float64Array.from(markers, (v) => (this.xLog ? toLog10(v) : v))
      .filter(Number.isFinite)
      .sort();

    this.panels = panels.map((spec) => this.buildPanel(spec));
    this.body.replaceChildren(...this.panels.map((p) => p.element));
    this.updateReadout();
    this.requestDraw();
  }

  buildPanel(spec) {
    const panel = {
      title: spec.title,
      unit: spec.unit ?? "",
      log: Boolean(spec.log),
      series: spec.series.map((s, k) => ({
        name: s.name,
        data: s.data,
        visible: s.visible !== false,
        slot: k % SLOTS,
        dash: Math.min(DASHES.length - 1, Math.floor(k / SLOTS)),
      })),
      y0: 0,
      sy: 1,
    };
    const setAll = (visible) => {
      for (const s of panel.series) {
        s.visible = visible;
        s.checkbox.checked = visible;
        this.onVisibility?.(s.name, visible);
      }
      this.requestDraw();
    };
    const chips = panel.series.map((s) => {
      s.checkbox = h("input", {
        type: "checkbox",
        checked: s.visible,
        onchange: () => {
          s.visible = s.checkbox.checked;
          this.onVisibility?.(s.name, s.visible);
          this.requestDraw();
        },
      });
      s.valueElement = h("span", { class: "chip-value" });
      return h(
        "label",
        { class: "chip" },
        s.checkbox,
        h("span", { class: `swatch slot-${s.slot + 1} dash-${s.dash}`, "aria-hidden": "true" }),
        h("span", { class: "chip-name", text: s.name }),
        s.valueElement,
      );
    });
    panel.canvas = h("canvas", {});
    panel.overlay = h("canvas", { class: "chart-overlay" });
    panel.wrap = h(
      "div",
      {
        class: "chart-canvas",
        tabindex: "0",
        role: "img",
        "aria-label":
          `${panel.title}: ${panel.series.length} columns over ${this.xLabel}. ` +
          "Arrow keys read the values, plus and minus zoom, Shift with arrow keys moves, 0 resets.",
      },
      panel.canvas,
      panel.overlay,
    );
    panel.ctx = panel.canvas.getContext("2d");
    panel.octx = panel.overlay.getContext("2d");
    this.listen(panel);
    panel.element = h(
      "section",
      { class: "chart-panel" },
      h(
        "div",
        { class: "chart-panel-head" },
        h("h3", { text: panel.title }),
        panel.series.length > 2 &&
          h("button", { type: "button", class: "link-button", onclick: () => setAll(true) }, "All"),
        panel.series.length > 2 &&
          h("button", { type: "button", class: "link-button", onclick: () => setAll(false) }, "None"),
      ),
      h("div", { class: "chart-legend", role: "group", "aria-label": `Columns of ${panel.title}` }, chips),
      panel.wrap,
    );
    return panel;
  }

  // ---- the view ----

  setView(a, b) {
    const [lo, hi] = this.full;
    const smallest = ((hi - lo) / Math.max(8, this.tx.length)) * 4;
    let span = Math.min(hi - lo, Math.max(smallest, b - a));
    let start = (a + b) / 2 - span / 2;
    start = Math.max(lo, Math.min(hi - span, start));
    this.view = [start, start + span];
    this.requestDraw();
  }

  /// Zooms by `factor` (< 1: in) around `center` (a value of the axis;
  /// default: the cursor when it is in view, else the middle).
  zoomBy(factor, center) {
    const [a, b] = this.view;
    let c = center;
    if (c === undefined) {
      const at = this.cursor >= 0 ? this.tx[this.cursor] : NaN;
      c = at >= a && at <= b ? at : (a + b) / 2;
    }
    this.setView(c - (c - a) * factor, c + (b - c) * factor);
  }

  panBy(fraction) {
    const [a, b] = this.view;
    const d = (b - a) * fraction;
    this.setView(a + d, b + d);
  }

  reset() {
    this.view = [...this.full];
    this.requestDraw();
  }

  /// Shows the surroundings of the value `x` and puts the cursor there: on
  /// the row `row`, or on the nearest one.
  focusOn(x, row = undefined) {
    if (!Number.isFinite(x)) return;
    const t = this.xLog ? toLog10(x) : x;
    if (!Number.isFinite(t)) return;
    const span = (this.full[1] - this.full[0]) / 20;
    this.setView(t - span / 2, t + span / 2);
    this.setCursor(row ?? this.nearest(t));
  }

  /// Puts the cursor on the row `index` for someone else (the diagram): the
  /// view follows when the row is outside it, and `onCursor` is not called.
  moveCursor(index) {
    const n = this.tx.length;
    if (n === 0 || index === this.cursor || !(index >= 0 && index < n)) return;
    const t = this.tx[index];
    const [a, b] = this.view;
    if (t < a) this.setView(t, t + (b - a));
    else if (t > b) this.setView(t - (b - a), t);
    this.setCursor(index, false);
  }

  /// Switches the columns `names` on, as their check boxes do. Returns
  /// those that were off.
  show(names) {
    const changed = [];
    for (const panel of this.panels) {
      for (const s of panel.series) {
        if (!names.includes(s.name) || s.visible) continue;
        s.visible = true;
        s.checkbox.checked = true;
        this.onVisibility?.(s.name, true);
        changed.push(s.name);
      }
    }
    if (changed.length > 0) this.requestDraw();
    return changed;
  }

  /// The index of the row nearest to the axis value `t`.
  nearest(t) {
    const tx = this.tx;
    const n = tx.length;
    if (n === 0) return -1;
    let lo = 0;
    let hi = n - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (tx[mid] <= t) lo = mid;
      else hi = mid;
    }
    return Math.abs(tx[hi] - t) < Math.abs(tx[lo] - t) ? hi : lo;
  }

  /// The rows to draw: those in view and one more on each side.
  indexRange() {
    const n = this.tx.length;
    if (n === 0) return [0, -1];
    const [a, b] = this.view;
    const i0 = Math.max(0, this.nearest(a) - 1);
    const i1 = Math.min(n - 1, this.nearest(b) + 1);
    return [i0, i1];
  }

  // ---- drawing ----

  requestDraw() {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  draw() {
    const css = getComputedStyle(this.root);
    const color = (name) => css.getPropertyValue(name).trim();
    this.colors = {
      series: Array.from({ length: SLOTS }, (_, k) => color(`--series-${k + 1}`)),
      grid: color("--chart-grid"),
      axis: color("--chart-axis"),
      muted: color("--chart-muted"),
      ink: color("--text"),
      surface: color("--surface"),
      accent: color("--accent"),
    };
    for (const panel of this.panels) this.drawPanel(panel);
    this.drawOverlays();
  }

  /// The ticks of the horizontal axis: positions in axis values, and labels.
  xTicks(width) {
    const [a, b] = this.view;
    const target = Math.max(2, Math.floor(width / (this.xLog ? 70 : 90)));
    const { ticks, labels, unit } = linearOrLogTicks(this.xLog, a, b, target, this.xUnit);
    return { ticks, labels, caption: this.caption(unit) };
  }

  caption(unit) {
    return unit ? `${this.xLabel} (${unit})` : this.xLabel;
  }

  prepareCanvas(panel) {
    const w = panel.wrap.clientWidth;
    const height = panel.wrap.clientHeight;
    if (w < 120 || height < 60) return null;
    const dpr = window.devicePixelRatio || 1;
    for (const canvas of [panel.canvas, panel.overlay]) {
      if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(height * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(height * dpr);
      }
    }
    const ctx = panel.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, height);
    const left = MARGIN.left;
    const top = MARGIN.top;
    const pw = w - MARGIN.left - MARGIN.right;
    const ph = height - MARGIN.top - MARGIN.bottom;
    const [v0, v1] = this.view;
    const sx = pw / (v1 - v0);
    panel.plot = { left, top, width: pw, height: ph, sx };
    return { w, height, ctx, left, top, pw, ph, v0, v1, sx };
  }

  panelRange(panel, layout) {
    const [i0, i1] = this.indexRange();
    let lo = Infinity;
    let hi = -Infinity;
    const lines = [];
    for (const s of panel.series) {
      if (!s.visible) continue;
      const points = decimate(this.tx, s.data, i0, i1, layout.v0, layout.sx, layout.pw, panel.log);
      lines.push({ s, points });
      if (points.min < lo) lo = points.min;
      if (points.max > hi) hi = points.max;
    }
    if (!(hi >= lo)) {
      lo = 0;
      hi = 1;
    }
    const span = hi - lo;
    const scale = Math.max(Math.abs(lo), Math.abs(hi));
    if (span <= 1e-9 * scale) {
      const mid = (lo + hi) / 2;
      const pad = panel.log ? 0.5 : Math.abs(mid) * 0.05 || 1;
      lo = mid - pad;
      hi = mid + pad;
    } else {
      const pad = span * 0.06;
      lo -= pad;
      hi += pad;
    }
    const sy = layout.ph / (hi - lo);
    panel.y0 = lo;
    panel.sy = sy;
    return { lines, lo, hi, sy };
  }

  drawAxes(ctx, layout, x, y, lo, hi, panelLog, X, Y) {
    const { w, left, top, pw, ph } = layout;
    const colors = this.colors;
    ctx.font = FONT;
    ctx.lineWidth = 1;
    ctx.strokeStyle = colors.grid;
    ctx.beginPath();
    for (const t of y.ticks) {
      const py = Math.round(Y(t)) + 0.5;
      ctx.moveTo(left, py);
      ctx.lineTo(left + pw, py);
    }
    for (const t of x.ticks) {
      const px = Math.round(X(t)) + 0.5;
      ctx.moveTo(px, top);
      ctx.lineTo(px, top + ph);
    }
    ctx.stroke();

    // The frame: the axes, and the zero line when it is in view.
    ctx.strokeStyle = colors.axis;
    ctx.beginPath();
    ctx.moveTo(left + 0.5, top);
    ctx.lineTo(left + 0.5, top + ph + 0.5);
    ctx.lineTo(left + pw, top + ph + 0.5);
    if (!panelLog && lo < 0 && hi > 0) {
      const py = Math.round(Y(0)) + 0.5;
      ctx.moveTo(left, py);
      ctx.lineTo(left + pw, py);
    }
    ctx.stroke();

    ctx.fillStyle = colors.muted;
    ctx.textBaseline = "middle";
    ctx.textAlign = "right";
    y.ticks.forEach((t, k) => ctx.fillText(y.labels[k], left - 6, Y(t)));
    ctx.textBaseline = "top";
    ctx.textAlign = "center";
    let free = -Infinity; // labels of the horizontal axis must not overlap
    x.ticks.forEach((t, k) => {
      const half = ctx.measureText(x.labels[k]).width / 2;
      const px = Math.max(half, Math.min(w - half, X(t)));
      if (px - half < free) return;
      ctx.fillText(x.labels[k], px, top + ph + 6);
      free = px + half + 8;
    });
    ctx.textAlign = "left";
    if (y.unit) ctx.fillText(y.unit, left, 4);
    ctx.textAlign = "right";
    ctx.fillText(x.caption, left + pw, 4);
  }

  drawMarkers(ctx, layout, X) {
    const { top, ph, v0, v1 } = layout;
    const inView = [];
    for (const m of this.markers) {
      if (m >= v0 && m <= v1) inView.push(m);
      if (inView.length > 40) break;
    }
    if (inView.length === 0 || inView.length > 40) return;
    ctx.strokeStyle = this.colors.muted;
    ctx.setLineDash([2, 4]);
    ctx.beginPath();
    for (const m of inView) {
      const px = Math.round(X(m)) + 0.5;
      ctx.moveTo(px, top);
      ctx.lineTo(px, top + ph);
    }
    ctx.stroke();
    ctx.setLineDash([]);
  }

  drawSeries(ctx, layout, Y, lines) {
    const { left, pw } = layout;
    const colors = this.colors;
    ctx.lineWidth = 1.5;
    ctx.lineJoin = "round";
    for (const { s, points } of lines) {
      const { xs, ys, count } = points;
      ctx.strokeStyle = colors.series[s.slot];
      ctx.setLineDash(DASHES[s.dash]);
      ctx.beginPath();
      let pen = false;
      for (let k = 0; k < count; k += 1) {
        const v = ys[k];
        if (v !== v) {
          pen = false;
          continue;
        }
        const px = left + xs[k];
        const py = Y(v);
        if (pen) ctx.lineTo(px, py);
        else ctx.moveTo(px, py);
        pen = true;
      }
      ctx.stroke();
      // Zoomed in far enough to tell the rows apart: mark them.
      if (points.every && count <= pw / 8) {
        ctx.setLineDash([]);
        ctx.fillStyle = colors.series[s.slot];
        ctx.beginPath();
        for (let k = 0; k < count; k += 1) {
          if (ys[k] !== ys[k]) continue;
          const px = left + xs[k];
          const py = Y(ys[k]);
          ctx.moveTo(px + 2.5, py);
          ctx.arc(px, py, 2.5, 0, 2 * Math.PI);
        }
        ctx.fill();
      }
    }
    ctx.setLineDash([]);
  }

  drawPanel(panel) {
    const layout = this.prepareCanvas(panel);
    if (!layout) return;
    const { ctx, left, top, pw, ph, v0, sx } = layout;
    const { lines, lo, hi, sy } = this.panelRange(panel, layout);
    const X = (t) => left + (t - v0) * sx;
    const Y = (v) => top + ph - (v - lo) * sy;
    const y = linearOrLogTicks(panel.log, lo, hi, Math.max(2, Math.floor(ph / 42)), panel.unit);
    const x = this.xTicks(pw);

    this.drawAxes(ctx, layout, x, y, lo, hi, panel.log, X, Y);

    ctx.save();
    ctx.beginPath();
    ctx.rect(left, top, pw, ph);
    ctx.clip();
    this.drawMarkers(ctx, layout, X);
    this.drawSeries(ctx, layout, Y, lines);
    ctx.restore();

    if (lines.length === 0) {
      ctx.fillStyle = this.colors.muted;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText("No column selected", left + pw / 2, top + ph / 2);
    }
  }

  /// The cursor and the selection: on their own canvas, so that moving the
  /// pointer does not draw the columns again.
  drawOverlays() {
    const colors = this.colors;
    if (!colors) return;
    const dpr = window.devicePixelRatio || 1;
    const [v0, v1] = this.view;
    for (const panel of this.panels) {
      const plot = panel.plot;
      if (!plot) continue;
      const ctx = panel.octx;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, panel.overlay.width, panel.overlay.height);
      if (this.drag?.mode === "zoom") {
        const a = Math.max(0, Math.min(plot.width, Math.min(this.drag.from, this.drag.to)));
        const b = Math.max(0, Math.min(plot.width, Math.max(this.drag.from, this.drag.to)));
        ctx.globalAlpha = 0.18;
        ctx.fillStyle = colors.accent;
        ctx.fillRect(plot.left + a, plot.top, b - a, plot.height);
        ctx.globalAlpha = 1;
      }
      if (this.cursor < 0) continue;
      const t = this.tx[this.cursor];
      if (!(t >= v0 && t <= v1)) continue;
      const px = plot.left + (t - v0) * plot.sx;
      ctx.strokeStyle = colors.muted;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(px) + 0.5, plot.top);
      ctx.lineTo(Math.round(px) + 0.5, plot.top + plot.height);
      ctx.stroke();
      for (const s of panel.series) {
        if (!s.visible) continue;
        let v = s.data[this.cursor];
        if (panel.log) v = toLog10(v);
        const py = plot.top + plot.height - (v - panel.y0) * panel.sy;
        if (!(py >= plot.top - 1 && py <= plot.top + plot.height + 1)) continue;
        ctx.beginPath();
        ctx.arc(px, py, 4, 0, 2 * Math.PI);
        ctx.fillStyle = colors.series[s.slot];
        ctx.fill();
        ctx.lineWidth = 2;
        ctx.strokeStyle = colors.surface;
        ctx.stroke();
      }
    }
  }

  updateReadout() {
    const i = this.cursor;
    if (i < 0) {
      this.readout.textContent = "";
    } else {
      const unit = this.xUnit ? ` ${this.xUnit}` : "";
      this.readout.textContent = `${this.xLabel} = ${formatValue(this.x[i])}${unit}`;
    }
    for (const panel of this.panels) {
      for (const s of panel.series) s.valueElement.textContent = i < 0 ? "" : formatValue(s.data[i]);
    }
  }

  setCursor(index, tell = true) {
    if (index === this.cursor) return;
    this.cursor = index;
    this.updateReadout();
    this.drawOverlays();
    if (tell) this.onCursor?.(index);
  }

  // ---- pointer and keyboard ----

  listen(panel) {
    const wrap = panel.wrap;
    /// Pixels from the left of the plot.
    const position = (event) => event.clientX - wrap.getBoundingClientRect().left - MARGIN.left;
    const valueAt = (px) => this.view[0] + px / (panel.plot?.sx ?? 1);

    wrap.addEventListener("pointerdown", (event) => {
      const button = event.button ?? 0;
      if (!panel.plot || (button !== 0 && button !== 1)) return;
      const px = position(event);
      const mode = button === 1 || event.shiftKey ? "pan" : "zoom";
      this.drag = { mode, from: px, to: px, view: [...this.view], pointer: event.pointerId };
      try {
        wrap.setPointerCapture?.(event.pointerId);
      } catch {
        // Ignore invalid/synthetic pointer IDs in Safari or tests.
      }
      if (button === 1) event.preventDefault();
    });
    wrap.addEventListener("pointermove", (event) => {
      if (!panel.plot) return;
      const px = position(event);
      const drag = this.drag;
      if (!drag) {
        this.setCursor(this.nearest(valueAt(px)));
        return;
      }
      if (drag.pointer !== undefined && event.pointerId !== undefined && event.pointerId !== drag.pointer) return;
      drag.to = px;
      if (drag.mode === "pan") {
        const d = (drag.from - px) / panel.plot.sx;
        this.setView(drag.view[0] + d, drag.view[1] + d);
      } else {
        this.drawOverlays();
      }
    });
    const finish = (event, cancelled) => {
      const drag = this.drag;
      if (!drag) return;
      if (drag.pointer !== undefined && event.pointerId !== undefined && event.pointerId !== drag.pointer) return;
      this.drag = null;
      try {
        if (wrap.hasPointerCapture?.(drag.pointer)) wrap.releasePointerCapture?.(drag.pointer);
      } catch {
        // Ignore missing pointer capture.
      }
      if (!cancelled && drag.mode === "zoom" && Math.abs(drag.to - drag.from) >= 5) {
        const a = valueAt(Math.max(0, Math.min(drag.from, drag.to)));
        const b = valueAt(Math.min(panel.plot.width, Math.max(drag.from, drag.to)));
        this.setView(a, b);
      } else {
        if (!cancelled) this.setCursor(this.nearest(valueAt(position(event))));
        this.drawOverlays();
      }
    };
    wrap.addEventListener("pointerup", (event) => finish(event, false));
    wrap.addEventListener("pointercancel", (event) => finish(event, true));
    wrap.addEventListener("dblclick", () => this.reset());
    wrap.addEventListener(
      "wheel",
      (event) => {
        // A plain wheel scrolls the page; with Ctrl (and for a pinch on a
        // touchpad, which arrives as Ctrl+wheel) it zooms.
        if (!panel.plot || !(event.ctrlKey || event.metaKey)) return;
        event.preventDefault();
        this.zoomBy(Math.exp(event.deltaY * 0.01), valueAt(position(event)));
      },
      { passive: false },
    );
    wrap.addEventListener("keydown", (event) => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const key = event.key;
      if (key === "ArrowLeft" || key === "ArrowRight") {
        const sign = key === "ArrowLeft" ? -1 : 1;
        if (event.shiftKey) this.panBy(0.1 * sign);
        else this.stepCursor(sign, panel);
      } else if (key === "+" || key === "=") {
        this.zoomBy(0.5);
      } else if (key === "-" || key === "_") {
        this.zoomBy(2);
      } else if (key === "0" || key === "Home") {
        this.reset();
      } else if (key === "Escape" && this.drag) {
        finish(event, true);
      } else {
        return;
      }
      event.preventDefault();
    });
  }

  /// Moves the cursor by what one pixel holds, and the view with it.
  stepCursor(sign, panel) {
    const n = this.tx.length;
    if (n === 0) return;
    const [a, b] = this.view;
    const [i0, i1] = this.indexRange();
    const perPixel = Math.max(1, Math.round((i1 - i0) / (panel.plot?.width ?? 600)));
    let index = this.cursor;
    if (index < 0 || !(this.tx[index] >= a && this.tx[index] <= b)) index = this.nearest((a + b) / 2);
    else index = Math.max(0, Math.min(n - 1, index + sign * perPixel));
    const t = this.tx[index];
    if (t < a) this.setView(t, t + (b - a));
    else if (t > b) this.setView(t - (b - a), t);
    this.setCursor(index);
  }
}
