// The numeric CSV files of a run (result.csv, scan.csv) as columns.

/// Parses a CSV text whose cells are all numbers into one Float64Array per
/// column. A cell that is empty or not a number becomes NaN. Column names in
/// valid models contain no commas or quotes.
export function parseNumericCsv(text) {
  if (text === "") return { header: [], columns: [], rows: 0 };
  const headerEnd = text.indexOf("\n");
  const header = (headerEnd < 0 ? text : text.slice(0, headerEnd)).replace(/\r$/, "").split(",");
  const width = header.length;
  // Count the rows first, to allocate the columns once.
  let rows = 0;
  if (headerEnd >= 0) {
    for (let i = headerEnd + 1; i < text.length; ) {
      const next = text.indexOf("\n", i);
      const end = next < 0 ? text.length : next;
      if (end > i) rows += 1;
      i = end + 1;
    }
  }
  const columns = header.map(() => new Float64Array(rows));
  let row = 0;
  for (let i = headerEnd + 1; headerEnd >= 0 && i < text.length; ) {
    const next = text.indexOf("\n", i);
    const end = next < 0 ? text.length : next;
    if (end > i) {
      let start = i;
      for (let k = 0; k < width; k += 1) {
        let comma = k === width - 1 ? end : text.indexOf(",", start);
        if (comma < 0 || comma > end) comma = end;
        const cell = text.slice(start, comma).trim();
        columns[k][row] = cell === "" ? NaN : Number(cell);
        start = Math.min(end, comma + 1);
        if (comma === end) {
          for (let rest = k + 1; rest < width; rest += 1) columns[rest][row] = NaN;
          break;
        }
      }
      row += 1;
    }
    i = end + 1;
  }
  return { header, columns, rows };
}
