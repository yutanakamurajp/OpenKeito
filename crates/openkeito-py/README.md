# OpenKeito

OpenKeito (Keito, 系統 = "power system" in Japanese) is an open electromagnetic transient (EMT)
simulator for power systems, meant to be driven by people and by AI agents alike. This is its
Python package: it runs the solver (written in Rust) in process and connects it to pandapower
and PowSyBl.

```python
import openkeito

result = openkeito.run("model.json")        # a dict, JSON text or a path
result.status                               # "completed"
result.time, result["V(n3)"]                # numpy arrays
result.summary["result"]["statistics"]      # smallest, largest and last value of every column
```

- **A model is one JSON document** with a published schema (`openkeito.schema()`), units in
  the field names (`resistance_ohm`, `time_step_s`) and a catalog of every component with an
  example (`openkeito.components()`). An invalid model or a failed run raises
  `openkeito.OpenKeitoError`, whose diagnostics each have a code, the place in the model, a
  message and a hint that says what to change.
- **From a power flow.** `openkeito.pandapower.from_pandapower` and
  `openkeito.powsybl.from_powsybl` / `from_cgmes` convert a network into a model whose
  steady state is the power flow, and the run starts from it. Generators can be sources or
  synchronous machines, inverters current injections or averaged converters.
- **Schedules.** Values of network elements over time (from an optimal power flow, say) run
  as time slices with disturbances, in parallel (`run_time_slices`), or as transitions from
  one time to the next with machines and converters under control (`run_transitions`).
- **MCP server.** `openkeito-mcp` offers validation, runs, queries of results, conversion
  and schedules to AI agents; its tools read and write only under one folder.

Python 3.10 or later. The extras `pandapower`, `powsybl`, `pandas` and `mcp` install what
the converters and the server need. Building from the source tree (`pip install .`) needs a
Rust toolchain.

The model format, the conventions, the conversion rules and the numerical methods with
their verification are documented in the repository (see the project links).

Licensed under either of Apache License 2.0 or MIT at your option.
