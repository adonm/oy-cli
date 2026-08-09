# Security

## Trust model

`oy` is not a sandbox. It adds a coding agent and repository review workflows to OpenCode.

OpenCode and the user control models, provider credentials, permissions, edits, shell commands, web access, and sessions. Oy's integrations do not add permission overrides.

The default OpenCode plugin includes a V2 adapter for oy's provider-only [`opencode-cursor` fork](https://github.com/adonm/opencode-cursor). OpenCode connects to it through an authenticated loopback bridge bound to `127.0.0.1`; the random bridge token and Cursor API key are held in memory and are not written to workspace files. When a `cursor/*` model is selected, Cursor runs its own local agent loop and tools, including shell, write, edit, and delete, directly on the host. Those calls are not gated by OpenCode permissions, and Cursor's sandbox is off by default. This is an explicit exception to the normal OpenCode permission boundary.

An explicit absolute Cursor request `cwd` wins; otherwise the bridge follows
the current absolute OpenCode working directory. Either is honored outside the
initial oy workspace. This is intentional: cross-repository work should not be
silently redirected or rejected. Treat `cursor/*` as host-capable and use an
external container or VM when filesystem isolation is required.

The oy CLI can:

- read eligible files inside the selected workspace;
- run read-only Git commands for target-diff reviews;
- write `.oy/runs/` evidence, reports, and private workflow metadata;
- update OpenCode configuration during explicit setup;
- launch OpenCode and optional mise-managed installers.

Prepared source text may be sent to the model provider configured in OpenCode. Treat prompts, reports, OpenCode logs/sessions, and setup backups as potentially sensitive.

## Safer use

- Review [`docs/install.sh`](docs/install.sh) before piping it to a shell.
- Run `oy setup --dry-run` before changing an existing integration.
- Configure OpenCode permissions for the repository you are reviewing.
- Treat `cursor/*` as an unsandboxed Cursor Agent session even though it appears in OpenCode's model picker. OpenCode permission rules do not constrain Cursor's internal tools.
- The loopback bridge token authenticates OpenCode's local request path; it is not an approval boundary. Do not expose the OpenCode API or bridge port to other users.
- If wanted, set `sandbox: true` under `providers.cursor.request.body`; oy does not force it because that can restrict normal coding workflows. Treat it as convenience hardening, not a substitute for host isolation.
- Use a disposable external container or VM for untrusted repositories or whenever host filesystem isolation is required.
- Do not mount the host Docker socket into an AI-assisted container.
- Do not keep secrets under the workspace root solely because secret-like filenames are excluded from collection.
- Inspect generated findings before publishing or uploading them.

Setup backs up changed oy-owned configuration and files before replacing them. A machine crash can still interrupt the operation, so keep the reported backup until the new setup is verified.

## Report a security problem

Open an issue in the public GitHub repository:

https://github.com/adonm/oy-cli/issues/new

Include the affected oy and OpenCode versions, operating system, reproduction steps, impact, and relevant logs. Redact credentials, authorization headers, prompts, private source code, session contents, and local paths that should not be public.
