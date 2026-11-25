# openkeito-cli

The command `openkeito` of OpenKeito, an open electromagnetic transient (EMT) simulator for
power systems, and the library behind it.

```sh
openkeito validate model.json
openkeito run model.json --out results/run-001
```

| Command | Output |
|---|---|
| `openkeito validate <file>` | The diagnostics of a model, a schedule file or a batch file |
| `openkeito run <model> --out <dir> [--threads N] [--force]` | `result.csv`, `events.csv`, `phasors.csv` (with steady-state initialization) and `summary.json` |
| `openkeito scan <model> --out <dir> [--force]` | The impedances over frequency of the model's `frequency_scan`, in `scan.csv` |
| `openkeito batch <batch.json> --out <dir> [--jobs N] [--force]` | The cases of a batch file, in parallel, each into its own directory |
| `openkeito schema [--batch \| --schedule]` | JSON Schema of the model format (or of the batch or the schedule format) |
| `openkeito components` | Every component and control block with parameters, units, sign conventions and an example |

Exit codes: 0 success, 1 the simulation failed, 2 the model or the request is invalid. Every
command prints a JSON document; a run is good when its `status` is `"completed"`.

The library (`openkeito_cli`) is what the command, the Python package and the WebAssembly
module call: `run_loaded` and `scan_loaded` write into an output directory, `run_texts` and
`scan_texts` return the result files as text. The feature `native` (on by default) holds
the command itself and `batch` (argument parsing, the thread pool); a host without them,
such as a browser, builds the library with `default-features = false`.

The model format, the conventions and the diagnostics are described in the documentation
of the repository.

Licensed under either of Apache License 2.0 or MIT at your option.
