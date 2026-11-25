// Reduces a large column to what the pixels of a plot can show.

export function toLog10(v) {
  return Number.isFinite(v) && v > 0 ? Math.log10(v) : NaN;
}

/// Reduces rows i0 ..= i1 of a column to points for a plot `width` pixels
/// wide. Returns pixel positions `xs` (from the left of the plot), values
/// `ys` (log10 of them with `log`; NaN where there is none) and the range.
export function decimate(tx, data, i0, i1, v0, sx, width, log) {
  const n = i1 - i0 + 1;
  if (n <= 0) {
    return { xs: new Float64Array(0), ys: new Float64Array(0), count: 0, min: Infinity, max: -Infinity, every: true };
  }
  let min = Infinity;
  let max = -Infinity;
  if (n <= 2 * width) {
    const xs = new Float64Array(n);
    const ys = new Float64Array(n);
    for (let k = 0; k < n; k += 1) {
      const x = tx[i0 + k];
      let v = Number.isFinite(x) ? data[i0 + k] : NaN;
      if (log) v = toLog10(v);
      else if (!Number.isFinite(v)) v = NaN;
      xs[k] = (x - v0) * sx;
      ys[k] = v;
      if (v < min) min = v;
      if (v > max) max = v;
    }
    return { xs, ys, count: n, min, max, every: true };
  }
  const xs = new Float64Array(4 * (width + 8));
  const ys = new Float64Array(4 * (width + 8));
  let count = 0;
  let column = NaN;
  let any = false;
  let first = 0;
  let last = 0;
  let lo = 0;
  let hi = 0;
  let loAt = 0;
  let hiAt = 0;
  const flush = () => {
    if (!any || count + 4 > xs.length) return;
    const px = column + 0.5;
    xs[count] = px;
    ys[count++] = first;
    if (lo < hi) {
      xs[count] = px;
      ys[count++] = loAt < hiAt ? lo : hi;
      xs[count] = px;
      ys[count++] = loAt < hiAt ? hi : lo;
    }
    xs[count] = px;
    ys[count++] = last;
  };
  for (let i = i0; i <= i1; i += 1) {
    let c = Math.floor((tx[i] - v0) * sx);
    if (c !== c) continue;
    c = c < -1 ? -1 : c > width ? width : c;
    if (c !== column) {
      flush();
      column = c;
      any = false;
    }
    let v = data[i];
    if (log) v = toLog10(v);
    else if (!Number.isFinite(v)) v = NaN;
    if (v - v !== 0) continue;
    if (!any) {
      any = true;
      first = last = lo = hi = v;
      loAt = hiAt = i;
    } else {
      last = v;
      if (v < lo) {
        lo = v;
        loAt = i;
      }
      if (v > hi) {
        hi = v;
        hiAt = i;
      }
    }
    if (v < min) min = v;
    if (v > max) max = v;
  }
  flush();
  return { xs, ys, count, min, max, every: false };
}
