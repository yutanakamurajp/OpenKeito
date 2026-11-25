# Security

## Reporting a vulnerability

Report a vulnerability privately with [GitHub's private vulnerability reporting](https://github.com/yutanakamurajp/OpenKeito/security/advisories/new):
on the page of the repository, "Security" → "Report a vulnerability". Do not open a public
issue for it. Say which version you used (`openkeito --version`), what an attacker can do
and how to reproduce it.

Only the latest release is supported: fixes are made on the main branch and released from
there.

## What to expect from the parts

OpenKeito reads models, schedules and network files and writes result files. The solver,
the command and the Python package open no network connections of their own; the MCP
server talks to its client over standard input and output.

- **The command `openkeito` and the Python API** act with the rights of the user who runs
  them and take paths as given. They write result files only into the output directory
  they are given and do not overwrite a result without `--force` (`force=True`). A batch
  file names the model files of its cases by path, and `openkeito batch` reads them.
- **The WebAssembly module and the browser page** have no access to files: a model goes in
  as text and the results come back as text, in the page's own tab. The page loads nothing
  from another site.
- **The MCP server** hands these functions to an AI agent, which is why it limits the
  paths. The next section says how far.

A model is data: there is no expression or script in it that would be executed. A very
large model, a very small time step or a long run can use much processor time, memory and
disk space; none of the parts limits that.

## The MCP server

`openkeito-mcp` runs with the rights of the user who starts it and limits the paths in the
arguments of its tools to the **root** folder (the working directory in which it starts,
`--root <folder>`, or the environment variable `OPENKEITO_MCP_ROOT`). The check is made on
the resolved path: a path that leads outside the root through `..`, an absolute path,
another drive or a UNC path, a symbolic link, a junction or a Windows device name is
refused. An output directory, and a network given as a folder, are also checked for links
in them that lead outside the root.

Pickle files of networks are not loaded. A pandapower JSON file is read only when it names
objects of the modules that pandapower itself writes.

The server does not protect against the following. Name as the root only a folder that an
agent may read and change as a whole.

- A hard link under the root to a file outside it: it cannot be told from a file.
- Another process that replaces a folder by a link between the check and the use. The
  tools themselves cannot create links.
- Files beside a network file that pypowsybl opens together with it (a network given as a
  folder is checked; a single file's neighbours are not, nor the content of an archive).
- Links deeper in an output directory than the tools write (one level for a run, two for
  a batch).
- The use of processor time, memory and disk space by large models and long runs.
- Everything, when the server is started with `--allow-any-path`: the tools then read and
  write any path that the user can. The server says so on standard error when it starts.

A message that refuses a path names the place it resolved to, so an agent can learn where
a link under the root leads.
