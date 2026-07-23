# Security

## Trust model

`oy` is not a sandbox. It adds a coding agent and repository review workflows to OpenCode or Cursor.

The agent host and the user control models, provider credentials, permissions, edits, shell commands, web access, and sessions. Oy's integrations do not add permission overrides.

The oy CLI can:

- read eligible files inside the selected workspace;
- run read-only Git commands for target-diff reviews;
- write `.oy/runs/` evidence, reports, and private workflow metadata;
- update OpenCode configuration or Cursor integration files during explicit setup;
- launch OpenCode and optional mise-managed installers.

Prepared source text may be sent to the model provider configured in the selected host. Treat prompts, reports, host logs/sessions, and setup backups as potentially sensitive.

## Safer use

- Review [`docs/install.sh`](docs/install.sh) before piping it to a shell.
- The `--cursor` installer target downloads and executes Cursor's official `https://cursor.com/install` script; review that upstream installer when required by your trust policy.
- Run `oy setup --dry-run` or `oy setup --cursor --dry-run` before changing an existing integration.
- Configure agent-host permissions for the repository you are reviewing.
- Use a disposable container or VM for untrusted repositories.
- Do not mount the host Docker socket into an AI-assisted container.
- Do not keep secrets under the workspace root solely because secret-like filenames are excluded from collection.
- Inspect generated findings before publishing or uploading them.

Setup backs up changed oy-owned configuration and files before replacing them. A machine crash can still interrupt the operation, so keep the reported backup until the new setup is verified.

## Report a security problem

Open an issue in the public GitHub repository:

https://github.com/adonm/oy-cli/issues/new

Include the affected oy and agent-host versions, operating system, reproduction steps, impact, and relevant logs. Redact credentials, authorization headers, prompts, private source code, session contents, and local paths that should not be public.
