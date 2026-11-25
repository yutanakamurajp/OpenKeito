# Origin of the code (clean-room policy)

OpenKeito is an independent implementation. These rules keep it that way.

## Allowed sources

- Published papers and textbooks on EMT simulation, numerical integration and circuit
  analysis (listed in [numerical-method.md](numerical-method.md)).
- Public standards and the documentation of open-source projects whose licenses permit it.
- Closed-form solutions and hand calculations.

## Not allowed

- Decompiling, disassembling or otherwise inspecting the binaries of any proprietary
  simulator to learn how it is implemented.
- Copying code, component definitions, file formats or documentation text from a
  proprietary simulator. OpenKeito's model format is its own.
- Committing output files of a proprietary simulator, or input files in its formats.
  Comparisons with such tools are run locally as black-box checks of results; publishing
  them requires that the tool's license permits it.

## Contributions

- Contributors confirm that their contribution follows these rules.
- Do not commit names of client companies or persons, or material received from them.
- Reference data in the repository is either analytic or produced by OpenKeito itself.
