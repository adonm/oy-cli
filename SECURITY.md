# Security

## Trust model

`oy` is not a sandbox. It adds repository audit and review workflows to the coding agent you already use.

The agent and the user control models, provider credentials, permissions, edits, shell commands, web access, and sessions. The oy skills do not add permission overrides; they run under the agent's own permission model.

The oy CLI can:

- read eligible files inside the selected workspace;
- run read-only Git commands for target-diff reviews;
- write `.oy/runs/` evidence, reports, and private workflow metadata;
- write the skills and migrate legacy OpenCode config during explicit setup;
- delete the obsolete OpenCode plugin package cache under the platform cache directory;
- launch mise-managed installers during `oy doctor --install-missing`.

Prepared source text may be sent to the model provider configured in your agent. Treat prompts, reports, agent logs/sessions, and setup backups as potentially sensitive.

## Safer use

- Review [`docs/install.sh`](docs/install.sh) before piping it to a shell.
- Run `oy setup --dry-run` before changing an existing installation.
- Configure your agent's permissions for the repository you are reviewing.
- Use a disposable external container or VM for untrusted repositories or whenever host filesystem isolation is required.
- Do not mount the host Docker socket into an AI-assisted container.
- Do not keep secrets under the workspace root solely because secret-like filenames are excluded from collection.
- Inspect generated findings before publishing or uploading them.

Setup backs up changed oy-owned files and legacy config before replacing them. A machine crash can still interrupt the operation, so keep the reported backup until the new setup is verified.

## Report a security problem

Open an issue in the public GitHub repository:

https://github.com/adonm/oy-cli/issues/new

Include the affected oy version, your agent and its version, operating system, reproduction steps, impact, and relevant logs. Redact credentials, authorization headers, prompts, private source code, session contents, and local paths that should not be public.
