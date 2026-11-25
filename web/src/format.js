// Numbers for people: axis ticks with SI prefixes, and values with six digits.

const PREFIXES = new Map([
  [-15, "f"], [-12, "p"], [-9, "n"], [-6, "µ"], [-3, "m"], [0, ""],
  [3, "k"], [6, "M"], [9, "G"], [12, "T"],
]);

/// The power of 1000 that brings `maxAbs` into 1 .. 1000, and its prefix.
export function siScale(maxAbs) {
  if (!(maxAbs > 0) || !Number.isFinite(maxAbs)) return { exponent: 0, prefix: "" };
  let exponent = Math.floor(Math.log10(maxAbs) / 3 + 1e-9) * 3;
  exponent = Math.max(-15, Math.min(12, exponent));
  return { exponent, prefix: PREFIXES.get(exponent) };
}

/// Ticks at 1, 2 or 5 times a power of ten: about `target` of them in lo .. hi.
export function niceTicks(lo, hi, target) {
  const span = hi - lo;
  if (!(span > 0) || !Number.isFinite(span)) return { ticks: [lo], step: 1 };
  const raw = span / Math.max(1, target);
  const power = 10 ** Math.floor(Math.log10(raw));
  const unit = raw / power;
  const step = (unit < 1.5 ? 1 : unit < 3.5 ? 2 : unit < 7.5 ? 5 : 10) * power;
  if (lo + step === lo || hi + step === hi) return { ticks: [lo], step: 1 };
  const ticks = [];
  const first = Math.ceil(lo / step - 1e-9);
  const last = Math.floor(hi / step + 1e-9);
  for (let k = first; k <= last && ticks.length < 200; k += 1) ticks.push(k * step);
  return { ticks, step };
}

/// Ticks of a logarithmic axis whose ends are lo and hi in log10. Decades;
/// with 2 and 5 when there are few; null when the view is within one decade
/// (the caller then takes linear ticks of the values).
export function logTicks(lo, hi, target) {
  const decades = hi - lo;
  if (decades < 0.7) return null;
  const every = Math.max(1, Math.ceil(decades / Math.max(1, target)));
  const multiples = decades <= 1.5 ? [1, 2, 5] : decades <= 3 && target >= 6 ? [1, 3] : [1];
  const ticks = [];
  for (let d = Math.floor(lo); d <= Math.ceil(hi); d += every) {
    for (const m of every === 1 ? multiples : [1]) {
      const t = d + Math.log10(m);
      if (t >= lo - 1e-9 && t <= hi + 1e-9) ticks.push(t);
    }
  }
  return ticks;
}

/// Labels of linear ticks that share one scale: `scale.exponent` is taken out
/// (it goes into the unit of the axis), the decimals follow the step.
export function tickLabels(ticks, step, scale) {
  const factor = 10 ** scale.exponent;
  const decimals = Math.max(0, Math.min(12, -Math.floor(Math.log10(step / factor) + 1e-9)));
  return ticks.map((t) => {
    const text = (t / factor).toFixed(decimals);
    return Number(text) === 0 ? "0" : text;
  });
}

/// One value with its own SI prefix ("1.5 k"), for the ticks of a log axis.
export function withPrefix(value) {
  if (value === 0 || !Number.isFinite(value)) return String(value);
  const { exponent, prefix } = siScale(Math.abs(value));
  const text = String(Number((value / 10 ** exponent).toPrecision(3)));
  return prefix ? `${text} ${prefix}` : text;
}

/// A value with six significant digits, as a readout shows it.
export function formatValue(value) {
  if (typeof value !== "number" || Number.isNaN(value)) return "–";
  if (!Number.isFinite(value)) return value > 0 ? "∞" : "−∞";
  if (value === 0) return "0";
  // "0.000001", "1e-7", "1.23457e+21": six digits, no trailing zeros.
  return String(Number(value.toPrecision(6))).replace(/^-/, "−");
}

/// Seconds for people: "0.42 s", "38 ms".
export function formatDuration(seconds) {
  if (!Number.isFinite(seconds)) return "–";
  if (seconds >= 1) return `${seconds.toFixed(2)} s`;
  if (seconds >= 1e-3) return `${(seconds * 1e3).toFixed(seconds >= 0.1 ? 0 : 1)} ms`;
  if (seconds > 0) return `${(seconds * 1e6).toFixed(0)} µs`;
  return "0 s";
}

export function formatBytes(bytes) {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} kB`;
  return `${bytes} B`;
}
