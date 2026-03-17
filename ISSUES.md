# Audit Findings

> Last audit: 2025-07-15 (OWASP ASVS 5.0.0 / MASVS 2.1.0)

## Summary

Total issues found: 13

| Severity | Open | Resolved |
|----------|------|----------|
| High     | 1    | 2        |
| Medium   | 2    | 3        |
| Low      | 0    | 5        |

---

## Open Issues

### H2. `httpx` tool has no SSRF protection -- no internal-network filtering

- **Location**: `oy_cli.py` -- `tool_httpx`
- **Category**: security
- **Reference**: OWASP ASVS V14.5.1 (SSRF Prevention), CWE-918

The `httpx` tool validates the URL scheme (`http`/`https`) but does **not** block requests to localhost (`127.0.0.1`, `::1`), link-local addresses (`169.254.x.x`), private RFC-1918 ranges, or cloud metadata endpoints (`169.254.169.254`). The LLM can be prompted (or tricked by a malicious document in the workspace) to fetch internal services, cloud instance metadata, or other private resources.

Additionally, `follow_redirects=True` means an attacker-controlled server could redirect to an internal address after passing the scheme check.

Since `oy` is a local CLI tool (not a server), this is partially mitigated by the threat model: the user has already granted shell access. However, it still represents an escalation path when the LLM processes untrusted input.

**Recommendation**:
1. Add an optional `OY_HTTPX_ALLOW_PRIVATE` flag (default `false`) that blocks requests resolved to private/loopback/link-local IPs.
2. Consider logging a warning when redirected to a different host.

---

### M2. Missing CI/CD pipeline for automated security checks

- **Location**: `.github/workflows/` (only `release.yml` exists)
- **Category**: security
- **Reference**: OWASP ASVS V1.1 (Secure Software Development Lifecycle)

The project has a release workflow but lacks PR/CI workflows for automated test execution, linting/formatting verification, dependency vulnerability scanning, and build verification before merge.

**Recommendation**:
1. Add a `ci.yml` workflow that runs `ruff check`, `ruff format --check`, and `pytest`.
2. Enable Dependabot or `pip-audit` for dependency scanning.
3. Add branch protection rules requiring passing checks.

---

### M3. Missing pre-commit hooks

- **Location**: Project root (no `.pre-commit-config.yaml`)
- **Category**: security
- **Reference**: OWASP ASVS V1.1 (Secure Development Practices)

No pre-commit hooks are configured to catch syntax errors, formatting violations, linting issues, or accidental secret/key commits.

**Recommendation**:
1. Add `.pre-commit-config.yaml` with hooks for ruff, check-yaml, check-json, and secret detection.
2. Document setup in `CONTRIBUTING.md`.

---

## Resolved Issues

| ID | Issue | Resolution |
|----|-------|------------|
| H1 | `glob` results escaped workspace via symlinks/`..` | Glob results filtered through workspace-root containment check; out-of-workspace paths silently excluded |
| H3 | Hardcoded OAuth client secrets | All credentials have `os.environ.get()` overrides; source comment documents them as public "installed app" creds per RFC 8252 S8.5 |
| M1 | `max_tokens: 8096` instead of `8192` in Claude client | Corrected to `8192` |
| M4 | `_rel()` leaked absolute paths for out-of-workspace files | Returns `"<outside workspace>"` placeholder instead |
| M5 | Credential files written world-readable | `save_json()` calls `p.chmod(0o600)` after writing |
| L1 | `lru_cache` on `command_env` undocumented | Comment added explaining caching rationale and stale-cache caveat |
| L2 | Hardcoded default model without validation | Removed; `oy` now prompts users to pick and save a model on first run |
| L3 | Hardcoded timeouts and limits | All operational parameters configurable via `OY_*` env vars |
| L4 | `__import__` used for version retrieval | Replaced with `from importlib.metadata import version` |
| L5 | No input length bounds on `tool_bash` command | `MAX_BASH_CMD_BYTES` check (default 64 KB, configurable via `OY_MAX_BASH_CMD_BYTES`) |

---

## Security Strengths

1. **Path traversal protection**: `resolve_path()` with explicit `ValueError` on traversal attempts.
2. **Header redaction**: `_redact_header()` redacts Authorization, Cookie, and token/secret/api-key patterns in httpx output.
3. **No dangerous eval/exec**: No `eval()`, `exec()`, `pickle`, `marshal`, or dynamic code execution.
4. **Subprocess safety**: `subprocess.run()` with explicit argument lists; bash tool uses `["bash", "-c", command]` by design.
5. **Workspace confinement**: All file operations go through `resolve_path()` before touching the filesystem.
6. **Structured error responses**: `ToolResult(ok=False, ...)` with error type and message, not raw stack traces.
7. **Token-based context management**: Transcript truncation prevents unbounded memory growth.
8. **Small attack surface**: ~4300 lines across two modules.
