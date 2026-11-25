# MCP server

`openkeito-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on
stdio that gives AI agents the tools of OpenKeito: the schema and the component catalog,
validation, model topology, runs and frequency scans, queries of results, the conversion of pandapower and
PowSyBl networks, schedules, time slices and transitions. It comes with the
[Python package](python.md):

```sh
python -m pip install ".[mcp,pandapower,powsybl]"    # from the repository
openkeito-mcp --root C:/work/emt                     # or: python -m openkeito.mcp_server
```

Register the command with a client, for example `claude mcp add openkeito -- openkeito-mcp`,
or in the client's configuration:

```json
{"mcpServers": {"openkeito": {"command": "openkeito-mcp", "args": ["--root", "C:/work/emt"]}}}
```

## What the tools do

Every tool returns a JSON document. A mistake in the use of a tool (a missing argument, a
file that cannot be read, an unknown network) is a tool error whose message says what to
change; a mistake in a model comes back as its diagnostics (`errors` and `warnings`, each
with code, path, message and hint). No tool overwrites a file or a result without `force`,
and no answer is larger than the limits in the table: what is left out is named in the
answer, with where to read it.

| Tool | Does |
|---|---|
| `get_schema`, `list_components` | the JSON Schema of models, batches or schedules; the component catalog |
| `validate_model` | the diagnostics of a model (code, path, message, hint) |
| `get_topology` | the connections of a model (components and terminals, nodes, buses, control blocks, signals and diagnostics; same document as `openkeito topology`) |
| `run_model`, `scan_frequency` | run a model or its frequency scan into a new directory; return the summary. Above 50 columns the summary lists the column names instead of the statistics and phasors of every column (`omitted`, `summary_path`) |
| `query_results` | chosen columns of a run or scan, in a time or frequency range, thinned, with statistics: at most `max_rows` rows, 20 000 values and 500 columns per answer; `truncated` and `note` say when the answer was cut and how to get more |
| `convert_network` | a pandapower or PowSyBl network into a model and its mapping. The network is a built-in example, a `.json` file written by `pandapower.to_json`, or a file that pypowsybl imports; pickle files are not loaded, and a pandapower file that names objects of other modules (controllers of your own) is refused |
| `make_schedule` | generator set points from pandapower's AC or DC OPF for every time of an input schedule |
| `run_time_slices` | a schedule × disturbances on a network, in parallel, with the steady-state check. Cases that did not complete carry their diagnostics; above 200 cases the list is cut (`cases_total`, `cases_truncated`) |
| `run_transitions` | a schedule followed from each of its times to the next (or over given intervals) with machines, converters, loads and breakers moving to the next set points, in parallel; one row per interval with the start and the end against the two power flows, the extremes on the way, and what could not be followed. With `chain` (or `times`), one run through all the times instead, with a row of differences per time (`settled` flags whether each stage settled). `estimate_only=true` returns the step count and calibrated wall-time estimate without running the full transition; `max_wall_s` refuses a run whose estimate exceeds the limit |

`convert_network`, `run_time_slices` and `run_transitions` read the short-circuit attributes
of a CGMES file as `from_cgmes` does (see
[Short-circuit data](python.md#short-circuit-data)); `short_circuit_profile=false` turns
that off. The `options` of `run_transitions` are those of
[`run_transitions` in Python](python.md#transitions): the machines get a power system
stabilizer unless `pss` is false.

## The root folder

The tools read and write only under one folder, the **root**:

1. `openkeito-mcp --root <folder>`, or else
2. the environment variable `OPENKEITO_MCP_ROOT`, or else
3. the working directory in which the server starts.

A root that is not an existing folder stops the server with exit code 2. A relative path
given to a tool is taken from the root (also with `--allow-any-path`), and the server tells
the agent its root in its instructions. A path is resolved before it is used (`..`, symbolic
links, junctions) and refused when it then lies outside the root, with a message that says
where it led and what to do:

```
out '../outside/x' is outside the root folder of this server: it resolves to C:\work\outside\x, which is not
under C:\work\root. Put it under C:\work\root and give that path (a relative path is taken from this root), or
start openkeito-mcp with --root <folder> (or OPENKEITO_MCP_ROOT) naming a folder that holds it, or with
--allow-any-path to lift the limit
```

`openkeito-mcp --allow-any-path` lifts the limit: the agent can then read and write every
file that the user who starts the server can. Use it only where that is acceptable. The
server says so in one line on standard error when it starts, and the limit cannot be lifted
by an environment variable.

The limit belongs to the MCP server alone; the command `openkeito` and the Python API take
paths as given. What the limit covers and what it does not is in
[SECURITY.md](../SECURITY.md).

From Python, `openkeito.mcp_server.server(root=None, allow_any_path=False)` returns the
server with every tool registered.
