# Security Policy

## Threat model

`oy` is a local coding assistant, not a sandbox. It can read workspace files, fetch public web pages, ask model providers for help, and—when approved—edit files or run shell commands with your user permissions.

Sensitive data can appear in prompts, source snippets, tool output, command output, saved sessions, and chat history. Treat `~/.config/oy-rust/` as sensitive local data.

Shell commands run with your user permissions. `oy` removes credential-like environment variables (for example names containing `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY`, `ACCESS_KEY`, or `AUTH`) before launching `bash`, but this is not a sandbox: commands can still read credential files, sockets, agent state, and other local resources available to your user.

## Safer use for untrusted repositories

Prefer a disposable container or VM. Start read-only, then opt into writes only when you trust the workspace and proposed changes.

```bash
# Deterministic no-tools audit; writes ISSUES.md inside the mounted workspace
docker run --rm -it \
  -v "$PWD:/workspace:rw" \
  -w /workspace \
  oy-image oy audit

# Exploratory read-only agent mode
docker run --rm -it \
  -v "$PWD:/workspace:ro" \
  -w /workspace \
  oy-image oy chat --mode plan

# Writable but contained workspace
docker run --rm -it \
  -v "$PWD:/workspace:rw" \
  -w /workspace \
  -e OPENAI_API_KEY \
  oy-image oy chat
```

Avoid mounting the host Docker socket into AI-assisted development containers. Docker socket access is usually host-root-equivalent.

`oy audit` does not give tools to the model, but it still sends collected repository text to the configured model provider. Use `oy chat --mode plan` for exploratory read-only work, avoid `auto-approve`/`OY_YOLO` for untrusted work, and prefer throwaway provider credentials where practical.

## Reporting a Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
