# OpenKeito in the browser

A static page that runs the solver in the browser, from the WebAssembly module
[`openkeito-wasm`](../crates/openkeito-wasm/README.md). It shows six things:

1. The circuit of the model as a diagram (single-line or expanded three-phase rails) and its
   control blocks as a block diagram, drawn from the model as it is typed; a press on a
   component selects it in the model, and positions can be adjusted by dragging and saved as
   an `openkeito-layout` 0.1 sidecar. After a run, the diagram shows the recorded voltages
   and currents at the time of the cursor of the waveforms, colors buses and components by
   magnitude, and shows which switches, diodes and faults conduct there.
2. Run a bundled example and look at the waveforms, the summary and the events.
3. Edit the model in `JSON` or structured `Form` mode and have it validated as you type:
   every diagnostic comes with its code, its place in the model, a message and a hint, and a
   corrected model can be run at once.
4. Examples with a synchronous machine and with a grid-following converter (a fault at the
   terminals and the swing after it, the current limit during a fault, ...).
5. A frequency scan (`frequency_scan` of the model): impedance over frequency.
6. Inspect `openkeito batch` (and Python batch) output folders in the `Batch` tab: filter and
   sort cases, highlight threshold violations, compare waveforms across cases, and open any
   case in the diagram and waveforms.

Plain HTML, CSS and JavaScript (ES modules): no framework, no bundler, no dependency, nothing
loaded from another site. The solver runs in a Web Worker on one thread, so the page stays
responsive, and "Stop" ends a long run by ending the worker and starting a new one.

## Files

| File | What it is |
|---|---|
| `index.html`, `style.css`, `404.html`, `favicon.svg` | The page |
| `config.js` | The one place for settings that depend on where the page is published: `repositoryUrl` (the link "Source"; empty hides the link) |
| `examples.json` | The list of examples: file in `examples/`, group, title, a description of one or two lines, the columns shown first (`columns`, and `scanColumns` for the scan), the example shown first (`default`) |
| `src/main.js` | The page: editor, mode toggle (`JSON`/`Form`), diagnostics, tabs, summary, events |
| `src/worker.js` | The worker that holds the module |
| `src/chart.js` | The charts (canvas, written for this page) |
| `src/diagram.js`, `src/layout.js`, `src/render.js`, `src/symbols.js` | The diagram: what the page does with it, placing it, writing it as SVG, and the symbols (see "The diagram") |
| `src/layout_sidecar.js` | `openkeito-layout` 0.1 sidecar parsing, validation, deterministic serialization, and override merge |
| `src/inspector.js`, `src/inspector_panel.js` | Catalog/schema-driven structured form editing, undo/redo, and diagnostic field mapping |
| `src/overlay.js` | The results of a run on the diagram: which column belongs where, the states from the events, voltage/current magnitude classification, what is shown at one row (see "Results on the diagram") |
| `src/batch.js`, `src/batch_view.js` | Lazy folder/file loading for `openkeito batch` outputs, threshold filtering, multi-case comparison, and opening a case |
| `src/csv.js`, `src/decimate.js`, `src/locate.js`, `src/format.js`, `src/dom.js` | Reading the CSV, reducing curves to pixel columns, finding the place of a diagnostic in the text, numbers for the axes, building elements |
| `build.mjs` | Assembles `web/dist` |
| `serve.mjs` | A static server for a local look |
| `tests/modules.test.mjs`, `tests/diagram.test.mjs`, `tests/overlay.test.mjs`, `tests/layout_sidecar.test.mjs`, `tests/inspector.test.mjs`, `tests/batch.test.mjs`, `tests/batch_view.test.mjs`, `tests/a11y.test.mjs`, `tests/diagram_edit.test.mjs`, `tests/diagram_power.test.mjs` | Tests of the parts that need no browser. Those of the diagram, overlay, inspector, edit panel, and real-example power views take the topology, runs, catalog, and schema from the module, which must be built first |
| `_headers`, `cloudflare-build.sh` | For Cloudflare Pages, see below |

The examples are not copied into `web/`: `build.mjs` takes the files that `examples.json`
names from [`examples/`](../examples). It fails when a model of `examples/` is in neither
`examples` nor `excluded` (a list of `{ "file", "reason" }`) of `examples.json`, so a new
example cannot be forgotten. An example that takes much longer than ten seconds in a browser
belongs into `excluded`, or gets a `note` (shown after its description).

## Build and look at it

Node 18 or later, Rust with the target `wasm32-unknown-unknown`, and the command
`wasm-bindgen` in exactly the version of the crate `wasm-bindgen` in `Cargo.lock` (now
0.2.130).

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked

# 1. the module
cargo build --release -p openkeito-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg \
    target/wasm32-unknown-unknown/release/openkeito_wasm.wasm

# 2. optional: a smaller module (binaryen; measured for 0.7.0: 3.2 MB -> 2.4 MB, compressed 0.78 -> 0.73 MB)
wasm-opt -O3 crates/openkeito-wasm/pkg/openkeito_wasm_bg.wasm \
    -o crates/openkeito-wasm/pkg/openkeito_wasm_bg.wasm

# 3. the page
node web/build.mjs

# 4. look at it
node web/serve.mjs            # http://127.0.0.1:8080/
```

`build.mjs` writes `web/dist` (not in Git; emptied first): the page, `pkg/` with the module
and its JavaScript glue, and `examples/`. It then runs every listed example from the module
in Node and fails when one does not complete or when a column named in `examples.json` is
not in its result (`--no-check` skips this; `--pkg` and `--out` choose other directories).

The page must be served over HTTP: opened as a file, a browser loads neither modules nor
workers. `serve.mjs` listens on 127.0.0.1 only and sends the headers of `_headers`, so the
content security policy is tested too. Any other static server works if it sends
`text/javascript` for `.js`; with `application/wasm` for `.wasm` the module is compiled while
it downloads (without it the glue falls back to a slower way and warns in the console).

```sh
node --test web/tests/modules.test.mjs      # after step 1: the tests of the diagram use the module
```

`build.mjs` also checks the diagram of every listed example (every component and block is
drawn at its nodes and signals, no wire runs through a symbol), that every type of the
catalog has a symbol, and that every column of the result of an example has its place in
the diagram.

Useful addresses: `#<example>` opens an example (`#synchronous_machine`); `?clock=off` runs
the solver without its clock (every time in the summary is then 0), to see what reading the
clock costs.

## What the page does

Checked in Chromium 150 and Firefox 155 on Windows 11.

- **Diagram.** The first tab beside the model: the circuit, drawn from the model as it is
  typed (300 ms after the last keystroke, with `validate`). Nothing about the picture is in
  the model, and nothing is random: the same model gives the same SVG.
  - *One diagram, two ways of drawing.* A bus is a bar and a three-phase component one line
    to it (a single-line diagram); every other node is a thin vertical line and a
    single-phase component its symbol between two of them, with right-angled wires. A
    single-phase component at a phase of a bus leaves the bar with the phase written at it
    (`a`); so does a fault (`a`, `bc`) and a terminal written as three nodes (`abc` at a star
    point). A connection to `gnd` is an earth symbol at the component. Toggling `3-phase`
    expands three-phase buses into individual phase rails (`a`, `b`, `c`) for models up to
    `MAX_THREE_PHASE_EXPAND = 400` components; larger models stay in single-line view.
  - *Controls.* The second face is the block diagram: a vertical line for every signal, a
    block to the left of its output, with its inputs coming in from the left (a feedback
    steps down below the block and runs back). A component that reads a signal (a gate, a
    controlled source, a machine) stands there as a dashed box, and all the blocks that a
    `grid_following_converter` brings with it as one. Underlined links lead across:
    `← pwm` below a switch to the block that gives `pwm`, `→ v_out` at a node or a component
    to the measurement block that reads it, `← V(out)` and `← circuit` back to the circuit.
  - *Names.* A name that is too long for its place (16 characters at a symbol, 18 above a
    net, 21 in a link) goes on over a second and a third line
    (broken after `_`, `.` and the like where it can be): the names of components, blocks,
    nodes, buses and signals and the links are whole in the picture. Only what is longer
    than three lines ends with `…`, and so does the field written at a port of a block when
    its wire is shorter than the word; the tooltip has all of it.
  - *The model and the diagram.* A press on a component or a block selects it in the text;
    the component or block the text cursor is in is framed in the diagram (and its face
    shown). What an error or a warning is about is drawn in its colour with a mark (a disc
    with `!` for an error, a triangle for a warning): the component, the block, the node or
    the bus (`diagnostics` of the topology document). While the text cannot be read, the tab
    says so in place of a diagram.
  - *Results.* After a run, the values of the row at the cursor of the waveforms are
    written into the diagram, and the switches, diodes and faults are drawn as that row has
    them; `Levels` (on by default) colors buses and components by voltage and current
    magnitude (`data-heat="low|mid|high|over"`). A press on a component switches its columns
    on in the waveforms. See "Results on the diagram" below.
  - *Layout adjustment.* Toggling `Move` lets you drag rails horizontally and items in both
    axes (beyond a 3 px pointer threshold) while rerouting connecting wires. `Export layout`
    and `Import layout` save and load `openkeito-layout` `"0.1"` sidecar JSON without
    modifying the model JSON; `Reset layout` restores the automatic placement. Sidecar
    overrides for bundled examples are persisted per example in `localStorage`.
  - *Keys.* Tab reaches `.diagram-view` and then one of its components; Enter, Space, or the
    arrow keys move into and across components, links, and (after a run) the nodes, buses,
    and signals that have columns (Home and End jump to the first and last stop, Enter or
    Space activates); `+`, `−`, `0` (fit the width) and `1` (full size) zoom, as do the
    buttons and Ctrl+wheel. After a run, `,` and `.` go one row back and on, `[` and `]` to
    the event before and to the next. The diagram scrolls in its frame.
  - *Large models.* The components lie in rows that are packed from the top, the nets side
    by side, so a network of 118 buses is a wide picture (353 components: 14 722 × 1 674 px,
    scrolled). While the picture is larger than its frame by more than a quarter, an
    overview above it shows the whole in little (a line for every net, a box for every
    symbol, in the colours of the diagnostics) with a frame around what is in view; a press
    or a drag in the overview moves there (falling back to `getBoundingClientRect` when
    `getScreenCTM` is unavailable or singular). Above 1 500 components only the buses are
    drawn, with one line for all the components between two of them; the tab says so.
- **Model.** Switch between `JSON` (a text area with line numbers) and `Form` mode (backed by
  the same JSON text as the single source of truth). `Form` mode builds structured parameter
  forms from the 63-type catalog and JSON Schema (with units, descriptions, sign
  conventions, and `<datalist>` candidates for nodes, buses, and signals), lets you add or
  remove components, controls, and outputs, keeps Undo/Redo history, and shows `validate`
  diagnostics inline beside the matching field. 300 ms after the last edit the text goes to
  `validate`; errors and warnings are listed below with code, path, message and hint. A path
  is a button that selects the place in the text (`components[1].resistance_ohm`, or
  `line 14 column 4` when the text cannot be read), and the lines are marked in the margin.
  Ctrl+Enter runs.
- **Running.** The run that follows the choice of an example leaves the open tab as it is;
  a run started with the button or Ctrl+Enter shows the waveforms.
- **Waveforms.** One panel per unit (voltage, current, signals; never two scales in one
  panel), sharing the time axis. Columns are switched on and off with check boxes; the choice
  is kept over runs of the same model. Drag to zoom, Shift+drag to move, Ctrl+wheel or pinch
  to zoom, double-click to reset; with the keyboard ← → move the cursor, Shift+← → move the
  view, + − zoom, 0 resets. The cursor shows the values of all columns at its row. Dotted
  lines mark events while no more than 40 are in view.
- **Frequency scan.** Magnitude (logarithmic) and angle over frequency (logarithmic when the
  scan covers at least a factor of 20).
- **Summary.** Status, warnings, the numbers of the simulation and the solver, the computing
  time and its parts, the range of every column, downloads (`summary.json`, `result.csv`,
  `events.csv`, `phasors.csv`), and the whole summary.
- **Events.** Counts per component and the first 300 events; a time is a button that shows
  that instant in the waveforms, a component one that shows it in the diagram at that
  instant.
- **Batch.** Lazily loads an `openkeito batch` (or Python batch) output folder via
  `webkitdirectory` or file multi-selection, reading only `batch-summary.json` and
  `batch-metrics.csv` at first. Cases can be filtered by status or search query, sorted by
  metric column, and checked against lower/upper thresholds (highlighting violations with
  `.breach`). Checking cases lazily reads their `result.csv` files to overlay aligned
  comparison waveforms, and `Open` loads a case into `Diagram`, `Waveforms`, `Summary`, and
  `Events`.

Large results: the worker parses the CSV into one `Float64Array` per column and transfers
them. Before drawing, a column is reduced to what the pixels can show (per pixel column the
first, lowest, highest and last value), so a redraw is one pass over the rows in view.
Measured in Chromium 150 on the machine of the table below: 0.7 ms for 100 000 rows × 4
columns, 3.4 ms for 1 000 000 rows.

Colors: eight categorical colors in a fixed order for the light and for the dark scheme
(a ninth column of a panel starts again with a dashed line). The scheme follows the system;
"Theme" in the header overrides it.

### The diagram

`src/layout.js` turns the document of `topology` into positions, `src/layout_sidecar.js`
validates and merges `openkeito-layout` 0.1 position overrides, `src/render.js` turns placed
faces into SVG text, `src/symbols.js` holds the symbols, and `src/diagram.js` is what the
page does with them. The first four need no browser.

How it is arranged: every net (a bus, a node, on the face of the controls a signal) is a
vertical line; they stand side by side in the order in which a breadth-first walk from the
first source reaches them (the signals: in an order in which every block comes after those
it reads, as far as a loop allows). A component lies in the space to the right of its
leftmost net and reaches its nets with horizontal wires; it has a band of its own over all
the spaces its wires pass, so no wire runs through another component, and the bands are
packed from the top (first fit over intervals sorted by their start, the greedy colouring
of an interval graph). A symbol is mirrored when its first terminal is on the right, so that
the anode of a diode and the `+` of a source are where the model has them. Layered drawings
of this kind go back to Sugiyama, Tagawa and Toda, "Methods for visual understanding of
hierarchical system structures", IEEE Trans. Systems, Man, and Cybernetics 11 (1981).

The symbols follow the graphical symbols of IEC 60617 where it has one for the component,
and are put together from its elements where it has none; the right column says which.
They were drawn from the published descriptions of the standard, not checked against its
database line by line.

| Type | Symbol | |
|---|---|---|
| `resistor` | rectangle | IEC |
| `inductor` | four half circles | IEC |
| `capacitor` | two plates | IEC |
| `voltage_source` | circle with the line of the connection through it; `+` at `nodes[0]` | IEC (ideal voltage source), `+` added |
| `current_source` | circle with a line across; an arrow from `nodes[0]` to `nodes[1]` | IEC (ideal current source), arrow added |
| `switch` | make contact | IEC |
| `diode` | triangle and bar, anode at `nodes[0]` | IEC |
| `controlled_switch` | make contact with a dashed link to an operating device | composed |
| `thyristor` | triangle and bar with a gate at the cathode side, anode at `nodes[0]` | IEC |
| `transformer` | two overlapping circles, the ends of each winding on its side | IEC |
| `three_phase_transformer` | two overlapping circles; a star point that is brought out leaves below its winding | IEC |
| `three_phase_ideal_transformer` | two overlapping circles with the arrow of adjustability | composed |
| `three_phase_source` | circle with a sine and `3` | composed |
| `three_phase_current_source` | the current source with `3` | composed |
| `controlled_voltage_source`, `controlled_current_source`, `three_phase_controlled_source`, `three_phase_controlled_current_source` | the source with a dashed link to an operating device | composed |
| `synchronous_machine` | circle with `GS` and `3~` | IEC (machine) |
| `grid_following_converter` | rectangle with a diagonal, `~` towards the bus and `=` behind | IEC (converter) |
| `vcvs`, `vccs`, `cccs`, `ccvs` | on two tracks: the control port (two open ends, `+` above) or the sense port (a bar joining its ends with the arrow of its current) on the left, a diamond on the right with `+` and `−` (a voltage) or the arrow of the output current, and a dashed link from the port to the diamond | textbook (the diamond of circuit textbooks; no IEC 60617 entry was found in public descriptions) |
| `coupled_rl` | rectangle with `Z` | IEC (impedance) |
| `pi_line` | rectangle with a capacitor to earth at each end | composed |
| `bergeron_line` | rectangle with a bar at each end and `τ` | composed |
| `three_phase_load` | filled arrow away from the bus | composed |
| `fault` | flash with an arrowhead | IEC (flashover) |
| `nonlinear_resistor`, `nonlinear_inductor` | resistor, or inductor with a core, crossed by the line of a non-linear characteristic | IEC |
| earth | a lead to three bars | IEC |
| control blocks (32) | rectangle with a mark: `Σ`, `∫`, `K`, `PI`, `1/(1+sT)`, `G(s)`, `×`, `÷`, `f(u)`, `PLL`, `abc/dq0`, a small curve for a limiter, a dead band, a step, a ramp, a sine, a triangle wave, a time series, a piecewise linear curve | |

Every type of `openkeito components` has an entry in `CIRCUIT` or `CONTROL` of
`src/symbols.js` (31 and 32). A type without one is drawn as a dashed box with its name;
the tests and `build.mjs` fail while there is one, so a new component gets its symbol with
it.

Time to the diagram, measured in Node 24.15 on the machine of the table below (the smallest
of 5; `topology` is the call of the module with reading its JSON, "diagram" is both faces
placed and written as SVG, with their overviews):

| Model | Components | Buses | `topology` (ms) | Document | Diagram (ms) | SVG |
|---|---|---|---|---|---|---|
| `rlc` | 4 | 0 | 0.1 | 2 kB | 0.1 | 4 kB |
| `three_phase_fault` | 6 | 3 | 0.3 | 6 kB | 0.1 | 6 kB |
| `transition_three_times` | 20 | 6 | 0.7 | 34 kB | 1.2 | 61 kB |
| case118 of pandapower, converted | 353 | 118 | 5.0 | 330 kB | 5.7 | 351 kB |
| a grid of 20 × 20 buses | 1 161 | 400 | 22 | 1.1 MB | 29 | 1.2 MB |
| a grid of 40 × 40 buses (buses only) | 4 721 | 1 600 | 195 | 4.6 MB | 239 | 2.1 MB |

The first four rows were measured again with the overview and the names on several lines;
the two grids are the measurement before them. With the values of a run the SVG of the
118-bus model is 372 kB (room for 118 values; 14 722 × 1 685 px) and takes 6.1 ms; its
overviews are 58 kB.

In Chromium 150, the 118-bus model pasted into the editor has its
diagram in the page 0.43 s later, of which 0.3 s are the wait after the last keystroke;
putting the SVG (6 992 elements) into the page and laying it out takes 13 ms and 15 ms
(measured before the overview was added).

### Results on the diagram

`src/overlay.js` binds the columns of `result.csv` to the diagram, by `kind` and `target` of
`result.columns` in the summary, and makes for one row a *frame*: the text of every value and
the state of every switch, diode and fault. `render.js` writes a frame into the SVG text
(the tests compare that text), and the page writes the same texts and classes into the SVG
that is already there, so that a move of the cursor touches nothing else. What is promised:

- **Which values.** Only what the model records under `outputs`: the diagram shows cells of
  `result.csv`, it computes nothing. `V(n)` is written below the name of the node `n`,
  `V(b.a)` below the bus `b`; `I(X)` and `U(X)` below the symbol of the component `X` (the
  current, then the branch voltage); `I(T1.0.a)` and `I(T1.1.a)` on the side of terminal 0
  and of terminal 1 of `T1`; `S(x)` below the name of the signal `x` on the face of the
  controls. Where nothing is recorded, nothing is written, and no room is kept for it. A
  column that has no slot (the ground current `I(F1.g)` of a fault) is in the line below the
  diagram when its component is chosen, with every other column of that component.
- **Units.** The unit of the column (`V`, `A`; none for a signal), four significant digits
  and an SI prefix: `−12.35 kA`, `314.2 nV`; a signal has five digits.
- **Phases.** Of a three-phase quantity one phase is shown, the same for the whole diagram:
  `a` at first, chosen with the list beside the time. A component that has no column for
  that phase (a fault in phase a, seen at phase b) shows no value there. The line below the
  diagram and the waveforms have all three.
- **Time.** The row is the cursor of the waveforms: moving the pointer over a chart, the
  arrow keys there, the slider above the diagram, the buttons to the event before and to
  the next one, and a press in the Events tab all set the same row. Until one of them is
  used, the diagram shows the first row. With "Show the waveforms below the diagram" (on at
  first; the choice is kept) the Diagram tab has the charts below it, so that one can be
  watched while the cursor moves in the other.
- **States.** A `switch` and a `controlled_switch` are drawn open or closed (the closed
  contact has its two ends marked), a conducting `diode` is filled, a `fault` that is on is
  drawn in the colour of an error; the word below the symbol says the same (`open`,
  `closed`, `off`, `on`, `no fault`, `fault ab`). The state at a row is the one that row was
  computed with: the state at the start (a diode and a controlled switch blocking, a fault
  off, a switch as `initially_closed` says) and every event before the time of the row. An
  event at the time of the row, or between it and the next row, has not acted in that row
  (docs/model.md, "Conventions"): the component is framed with a dotted line and says what
  comes (`open ▸ closed`), and the row after it has the new state. A press on an event goes
  to that row. The `step` of a source is an instant to go to, not a state.
- **From the diagram to the waveforms.** A press on a component (or Enter on it) selects it
  in the model, as before, and switches its columns on in the waveforms, as their check
  boxes do; nothing is switched off. A block does this for the signals it gives; a node, a
  bus or a signal that has columns can be pressed too (its name is then underlined under the
  pointer).
- **An edited model.** The values belong to the text that was run. Once the model is edited
  and its diagram drawn again, they are taken away until the model is run again (or the
  edit is undone).

Time of a move of the cursor, in Chromium 150 on the machine of
the table below: 300 moves of the slider to rows all over the run, each with the texts and
classes of the diagram and the cursor of the charts.

| Model | Rows | Values in the diagram | Median (ms) | Largest (ms) |
|---|---|---|---|---|
| `rlc` | 10 001 | 2 | < 0.1 | 0.3 |
| `rectifier` | 100 001 | 8 | 0.1 | 0.3 |
| `transition_three_times` | 7 251 | 15 | 0.1 | 0.5 |
| case118 of pandapower, converted | 401 | 118 | 0.7 | 8.2 |

The time does not depend on the number of rows: a frame reads one cell per value and finds
the state of a component by bisection in its events (in Node 3 µs for `rectifier`, 35 µs
for the 118-bus model), so nothing has to be thinned out as for the charts. Binding the
columns of a run takes 0.2 ms for the 118-bus model (354 columns).

With `Levels` magnitude coloring (`data-heat` updates in `showRow()`), a Stage 2 integrated
measurement on the `case118`-scale network (118 buses, 353 components, 401 rows) measured
median `0.6 ms` and max `5.2 ms` across 100 cursor moves in Chromium 150, and
`4.32 ms` total in the Node unit benchmark (`computeNominalScales`, `frameAt`, level lookup,
and `toSvg`).

### Computing time in a browser

Chromium 150, Windows 11, x86-64, one run of each example after a
first one; "run" is the call of the module in the worker including the result as text,
"solver" is `timing.run_s` of the summary.

| Example | Steps | Rows | Run (ms) | Solver (ms) | Run without clock (ms) |
|---|---|---|---|---|---|
| `rlc` | 10 000 | 10 001 | 7.3 | 7.0 | 5.0 |
| `rl_breaker` | 8 000 | 8 001 | 6.8 | 6.6 | 5.6 |
| `three_phase_fault` | 15 000 | 15 001 | 69 | 68 | 54 |
| `line_energization` | 2 000 | 2 001 | 4.9 | 4.6 | 5.2 |
| `line_areas` | 5 000 | 5 001 | 27 | 26 | 25 |
| `synchronous_machine` | 30 000 | 1 501 | 50 | 49 | 55 |
| `machine_avr_governor` | 80 000 | 1 601 | 109 | 108 | 122 |
| `converter` | 6 000 | 6 001 | 22 | 21 | 23 |
| `converter_voltage_support` | 10 000 | 10 001 | 35 | 34 | 31 |
| `held_injection` | 2 000 | 2 001 | 7.0 | 6.5 | 6.2 |
| `buck` | 20 000 | 20 001 | 25 | 24 | 24 |
| `buck_interpolated` | 2 000 | 2 001 | 6.4 | 5.4 | 4.9 |
| `buck_pi` | 100 000 | 10 001 | 42 | 39 | 37 |
| `rectifier` | 100 000 | 100 001 | 125 | 124 | 85 |
| `transformer_inrush` | 2 000 | 2 001 | 2.8 | 2.6 | 2.3 |
| `saturable_reactor` | 1 200 | 1 201 | 1.4 | 1.1 | 1.3 |
| `arrester` | 160 | 161 | < 1 | 0.2 | < 1 |
| `current_control` | 300 | 301 | < 1 | 0.3 | < 1 |
| `pll` | 4 000 | 4 001 | 12 | 11 | 11 |
| `control_blocks` | 3 000 | 3 001 | 8.4 | 7.9 | 7.2 |

The frequency scans take 12 ms (`three_phase_fault`, 250 frequencies) and 10 ms
(`line_energization`, 500). From the click to the charts, `rectifier` (100 000 rows, 9.5 MB of
CSV) takes 0.18 s. No example comes near ten seconds, so none is excluded. The timer of a
browser has a resolution of 0.1 ms, so the small numbers are rough.

`transition_three_times` (145 000 steps, 7 251 rows) was added after this measurement; it is
the longest example and takes 0.5 s when `build.mjs` runs it in Node on the same machine.
`thyristor_rectifier` (5 000 steps, 5 001 rows) was added after it too.
`dependent_sources` (2 000 steps, 2 001 rows) and `ideal_transformer_vcvs_cccs` (5 000 steps,
5 001 rows) too.
`expression_logic` (2 000 steps, 2 001 rows) too.

The clock: a run reads it twice per output row. Where a model has cheap steps and writes
every one of them, that is a visible part of a run (`rectifier`: 125 ms with the clock, 85 ms
without). The page keeps the clock, because the parts of the computing time are worth more
than 40 ms here; `?clock=off` shows the difference.

## Cloudflare Pages

Written from the Cloudflare documentation as of 2026-11-22 (Pages: direct upload, build
configuration, build image, headers, serving, limits; Wrangler: `pages`; compression). Nothing
of this has been run: deploying is done by the maintainer.

What the page needs from a host is little: static files, `text/javascript` for `.js`, ideally
`application/wasm` for `.wasm`. `web/dist` is 54 files and 4.64 MB (27 examples; limits of
Pages: 20 000 files, 25 MiB per file).

### Direct upload (recommended)

Assemble `web/dist` on your machine or in CI as above, then upload the directory:

```sh
npx wrangler login                                   # once, opens the browser
npx wrangler pages project create <project> --production-branch main     # once
npx wrangler pages deploy web/dist --project-name <project>
```

- The site is then at `https://<project>.pages.dev`. `--branch <name>` deploys a preview at
  `<name>.<project>.pages.dev` instead of production. In a Git checkout Wrangler takes the
  branch from Git: a deploy from another branch than the production branch is a preview.
- In CI, give Wrangler an API token instead of `wrangler login` (the environment variables
  `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`).
- A project created for direct upload cannot be switched to Git integration later (and the
  other way round); it would be a new project.

### Git integration

Cloudflare builds on every push. In the project's build settings:

| Setting | Value |
|---|---|
| Framework preset | None |
| Build command | `bash web/cloudflare-build.sh` |
| Build output directory | `web/dist` |
| Root directory | (empty: the repository root) |

The build image (v3) has Node.js 22 but no Rust, so
[`cloudflare-build.sh`](cloudflare-build.sh) installs the toolchain with rustup, installs
`wasm-bindgen-cli` in the version of `Cargo.lock`, builds the module and runs `build.mjs`. The
script has not been run on Cloudflare. Compiling the toolchain's command and the module takes
several minutes of the 20 a build may take (the free plan has 500 builds a month, one at a
time), on every push of every branch; this is why the direct upload from CI, where the
compiled files can be cached, is the recommendation.

### Media type, compression, `_headers`

- **`.wasm`.** Pages sets `Content-Type` from the file name; nothing has to be configured.
  After the first deploy, check it once:
  `curl -sI https://<project>.pages.dev/pkg/openkeito_wasm_bg.wasm` should show
  `content-type: application/wasm`. If it did not, the page would still work (the glue falls
  back and warns in the console), and a rule in `_headers` could set the type.
- **Compression.** Cloudflare compresses responses itself (Gzip, Brotli or Zstandard, by what
  the browser accepts), and `application/wasm`, JavaScript, JSON, CSS and HTML are among the
  types it compresses. Do not upload compressed files and do not set `Content-Encoding`. The
  module is 3.96 MB and 0.96 MB compressed.
- **Caching.** Leave it to Pages: it sends an `ETag` and revalidates, and a new deploy is
  visible at once. The file names carry no hash, so a long `max-age` would be wrong.
- **`_headers`** is not required. The one in `web/` (copied into `web/dist`) adds a content
  security policy that allows the page's own files and compiling WebAssembly
  (`'wasm-unsafe-eval'`) and nothing else, forbids framing, and sends no referrer. Pages adds
  `X-Content-Type-Options: nosniff` and `Access-Control-Allow-Origin: *` on its own. To drop
  the policy, delete the file.
- **`404.html`.** Without one, Pages treats the site as a single-page application and answers
  every unknown address with the start page; with it, a missing file is a 404.

### Before publishing

- Check `repositoryUrl` in `config.js` (`https://github.com/yutanakamurajp/OpenKeito`).
- The page shows the version the module reports (`version()`): build the module from the
  commit you publish.
