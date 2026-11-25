# openkeito-model

The model format of OpenKeito, an open electromagnetic transient (EMT) simulator for power
systems: the structures of a model, of a batch file and of a schedule file, reading them
from JSON, validation, the component catalog and the JSON Schemas.

- A model is one JSON document. Units are part of the field names (`resistance_ohm`,
  `time_step_s`), unknown fields are errors, and a number without its exponent letter
  (`1.1141-6`) is an error whose hint gives the number as it should be written.
- `load_str` reads and validates a model and returns it with its warnings, or the list of
  diagnostics. Every diagnostic has a code, the path of the place in the model, a message
  and a hint that says what to change.
- `json_schema`, `batch_json_schema` and `schedule_json_schema` give the schemas that the
  command `openkeito schema` prints; `component_catalog` gives every component and control
  block with its parameters, units, sign convention and an example.

This crate does not solve anything: `openkeito-core` is the solver, and `openkeito-cli` the
command `openkeito` and the library that runs a model and writes its results. The format,
its conventions and the diagnostics are described in the documentation of the repository
(`docs/model.md`).

Licensed under either of Apache License 2.0 or MIT at your option.
