# Audit Findings

> **Last audit**: 2026-03-17 · commit `f2a023f` · OWASP ASVS 5.0.0 · [grugbrain.dev](https://grugbrain.dev)
>
> **Codebase**: `oy-cli` v0.3.3 — tiny AI coding assistant (2 source files, 4 Python total)
>
> | Metric | Value |
> |--------|-------|
> | Python LoC | 4,665 |
> | `oy_cli.py` | 2,167 lines (134 functions, 13 classes) |
> | `shim.py` | 2,588 lines (169 functions, 17 classes) |
> | Test LoC | ~486 lines across 2 files (45 tests) |
> | Provider shims | 6 (openai, codex, gemini, bedrock, bedrock-mantle, claude) |
> | Dependencies | 9 runtime (`boto3`, `defopt`, `httpx`, `markdownify`, `msgspec`, `openai`, `rich`, `tiktoken`, `tenacity`) |

---

## H1 · SSRF via `httpx` tool — no private-IP filtering

| | |
|---|---|
| **Location** | `oy_cli.py:1426-1433` (`tool_httpx`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V4.3.1 — SSRF prevention; V2.1.3 — server-side request validation |
| **Status** | Open |

The `httpx` tool validates that the URL scheme is `http` or `https` but does not
check whether the resolved IP is a private, loopback, or link-local address.
An LLM-generated tool call like `httpx http://169.254.169.254/latest/meta-data/`
could reach cloud metadata endpoints, Docker sockets, or internal services.

**Recommendation**: After parsing the URL, resolve the hostname and reject
addresses in `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
`169.254.0.0/16`, `::1`, and `fd00::/8` using `ipaddress.ip_address().is_private`.
Apply the check *after* DNS resolution to defeat DNS-rebinding via redirect
(the tool already caps redirects at 10). Consider an `OY_ALLOW_PRIVATE_HTTP=1`
escape hatch for local-dev use.

---

## H2 · Credential file written world-readable before `chmod`

| | |
|---|---|
| **Location** | `shim.py:389-392` (`save_json`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V14.3.3 — sensitive data at rest; V8.3.4 — file permissions |
| **Status** | Open |

`save_json` calls `write_text()` then `chmod(0o600)`. Between those two calls
the file is briefly world-readable (inheriting the process umask, typically
`0o644`). On a shared system an attacker could race the window and read
OAuth tokens.

**Recommendation**: Write to a temp file with `os.open(path, O_WRONLY|O_CREAT|O_TRUNC, 0o600)` +
`os.fdopen()` (or `tempfile.NamedTemporaryFile` + `os.rename`) to ensure the
file is *never* accessible to other users. Alternatively, set `os.umask(0o077)`
at startup (simpler but process-global).

---

## H3 · Debug log records full conversation (including workspace secrets)

| | |
|---|---|
| **Location** | `oy_cli.py:86-121, 1600-1604` (`_debug_log`, `run_turn`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V14.2.1 — prevent sensitive data leakage in logs |
| **Status** | Open |

When `OY_DEBUG=1`, every request event serializes the full `prepared` message
list — including all prior tool outputs (file contents, command results, grep
hits) — to `~/.config/oy/debug.jsonl`. If the workspace contains API keys,
`.env` files, or credentials they end up in a plaintext, append-only log with
no rotation or size cap.

**Recommendation**:
1. Truncate tool-result bodies in log entries (e.g. first 200 chars).
2. Add `RotatingFileHandler` with a sensible cap (e.g. 10 MB, 2 backups).
3. Set `0o600` on the log file at creation (currently inherits umask).
4. Document the risk in `README.md` next to the `OY_DEBUG` reference.

---

## H4 · Safety-limit env overrides have no bounds checking

| | |
|---|---|
| **Location** | `oy_cli.py:60-75` (`_env`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V2.2.1 — input validation for configuration |
| **Status** | Open |

`_env` coerces environment variables to the type of the default but applies
no min/max bounds. Setting `OY_DEFAULT_MAX_STEPS=0` silently disables the
agent loop; setting `OY_MAX_BASH_CMD_BYTES=999999999` removes the command-size
guard. A malicious `.env` loader or shell profile could weaken safety limits.

**Recommendation**: Add `clamp(value, lo, hi)` after coercion for
security-critical limits (`MAX_BASH_CMD_BYTES`, `DEFAULT_MAX_STEPS`,
`DEFAULT_MAX_TOOL_CALLS`). Document the env-override surface in
`CONTRIBUTING.md`.

---

## M1 · `shim.py` is a 2,588-line monolith

| | |
|---|---|
| **Location** | `shim.py` (entire file) |
| **Category** | Complexity |
| **Standard** | grugbrain.dev — "complexity is the enemy"; ASVS V1.1.1 — architectural simplicity |
| **Status** | Open |

`shim.py` contains 169 functions and 17 classes covering six distinct
provider backends (OpenAI, Codex, Gemini, Bedrock, Bedrock-Mantle, Claude),
SigV4 signing, OAuth token refresh, JSON/path utilities, retry logic, and
message codec registries. Finding a specific provider's auth flow requires
scrolling past unrelated code.

**Recommendation**: Split into a `shim/` package with one module per provider
(e.g. `shim/gemini.py`, `shim/bedrock.py`) plus `shim/common.py` for shared
types and retry logic. This aligns with the project's "easy to audit" goal
and reduces merge-conflict surface. The public API (`CompletionClient`,
`get_client`, `detect_available_shims`) stays in `shim/__init__.py`.

---

## M2 · Six provider codecs use similar-but-different patterns

| | |
|---|---|
| **Location** | `shim.py:250-360` (codec definitions), `shim.py:1585-2265` (client builders) |
| **Category** | Complexity |
| **Standard** | grugbrain.dev — "say thing once" / DRY |
| **Status** | Open |

Each provider has its own message-encoding lambdas, output-block extractors,
and request builders. The Gemini and Claude codecs are structurally identical
in several places (text encoding, tool-result wrapping) but differ just enough
to prevent direct reuse, making it easy for a fix in one to be missed in another.

**Recommendation**: Introduce a thin `ProviderCodec` protocol with
`encode_text`, `encode_tool_use`, `encode_tool_result`, and `decode_output`
methods, then implement per-provider. This replaces the six anonymous-lambda
registries with something greppable and testable.

---

## M3 · `oy_cli.py` tool schemas parsed from README at import time

| | |
|---|---|
| **Location** | `oy_cli.py:128-180` (`_load_readme`, `_parse_tool_descriptions`, `_parse_system_prompts`) |
| **Category** | Complexity |
| **Standard** | grugbrain.dev — "no magic"; ASVS V1.1.1 — predictability |
| **Status** | Open |

Tool descriptions and system prompts are regex-scraped from `README.md` at
import time. A small formatting change in the README (e.g. adding a column)
silently breaks tool registration with an opaque `RuntimeError`. The
indirection also makes it harder to find where a tool's description lives.

**Recommendation**: Keep the README as the human-readable reference but
define tool descriptions as constants in `oy_cli.py` (or a `tools.py`).
Generate the README table from those constants during `mise run docs` if
single-source-of-truth is desired.

---

## M4 · OAuth token-refresh race in `_persist_json_dict`

| | |
|---|---|
| **Location** | `shim.py:457-461` (`_persist_json_dict`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V14.3.3 — data integrity at rest |
| **Status** | Open |

`_persist_json_dict` reads a JSON file, mutates the dict in memory, and writes
it back without any locking. Two concurrent `oy` processes refreshing the same
OAuth token could clobber each other's writes, leaving a stale or corrupt
credential file.

**Recommendation**: Use `fcntl.flock` (or `msvcrt.locking` on Windows) around
the read-update-write cycle. Alternatively, write to a temp file and
`os.replace()` atomically (also addresses H2).

---

## L1 · `resolve_path` does not reject symlinks targeting outside workspace

| | |
|---|---|
| **Location** | `oy_cli.py:604-609` (`resolve_path`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V5.3.1 — file path traversal; V5.2.8 — symlink checks |
| **Status** | Open |

`resolve_path` uses `.resolve()` which follows symlinks, then checks the
resolved path is under the workspace root. This *is* safe for reads/writes
(the resolved path is verified). However, if an attacker can plant a symlink
inside the workspace pointing outside it, `resolve_path` will happily allow
operations on the target — the write lands on the external file, not the
symlink. The `tool_glob` filter (line 1330) already documents this risk.

**Recommendation**: Consider warning when the resolved path differs from the
un-resolved path (i.e. a symlink was followed). For high-security contexts,
add an `OY_DENY_SYMLINKS=1` mode that rejects any path where
`target.resolve() != (root / p)` (without resolve on the right side).

---

## L2 · No test coverage for provider auth/refresh flows

| | |
|---|---|
| **Location** | `tests/test_shim.py` (15 tests) |
| **Category** | Complexity |
| **Standard** | OWASP ASVS V1.1.6 — security controls are tested; grugbrain.dev — "test is good" |
| **Status** | Open |

The shim test file has 15 tests covering model-spec parsing, message encoding,
and JSON utilities. The OAuth refresh flows for Gemini, Codex, and Claude
(~200 lines of auth logic) have zero test coverage. A regression in token
refresh silently breaks authentication.

**Recommendation**: Add integration-style tests using `httpx`'s mock transport
or `respx` to exercise `refresh_gemini_token`, `_refresh_codex_session`,
and `_refresh_claude_token` with happy-path and error responses.

---

## L3 · `requires-python = ">=3.14"` limits adoption

| | |
|---|---|
| **Location** | `pyproject.toml:10` |
| **Category** | Complexity |
| **Standard** | grugbrain.dev — "don't be too clever" |
| **Status** | Open |

Python 3.14 was released in 2025 and is not yet widely available in CI images,
Docker base images, or enterprise environments. The codebase uses PEP 758
bare-except syntax (`except A, B:` without parentheses) which requires 3.14+,
but this is the *only* 3.14-specific feature used.

**Recommendation**: If broader adoption is desired, replace the 6 bare
`except A, B:` sites with `except (A, B):` and lower the requirement to
`>=3.12`. If 3.14 is intentional, document the rationale in `CONTRIBUTING.md`.

---

## L4 · History file permissions set after creation

| | |
|---|---|
| **Location** | `oy_cli.py:1793-1795` (`_setup_readline`) |
| **Category** | Security |
| **Standard** | OWASP ASVS V14.3.3 — sensitive data at rest |
| **Status** | Open |

`history_path.touch(mode=0o600, exist_ok=True)` only sets the mode on
*creation*. If the file already exists with laxer permissions (e.g. from an
older version), `touch` with `exist_ok=True` does *not* change the mode.
The subsequent `readline.write_history_file` inherits whatever permissions
the file already has.

**Recommendation**: Unconditionally call `history_path.chmod(0o600)` after
the `touch` call to enforce permissions on every run.

---

## L5 · Unbounded `OY_BEDROCK_MAX_OUTPUT_TOKENS` parsed via bare `int()`

| | |
|---|---|
| **Location** | `shim.py:2064-2066` |
| **Category** | Security |
| **Standard** | OWASP ASVS V2.2.1 — safe configuration parsing |
| **Status** | Open |

`int(os.environ.get("OY_BEDROCK_MAX_OUTPUT_TOKENS", "4096"))` has no
validation. A non-numeric value causes an unhandled `ValueError` crash;
an absurdly large value could trigger unexpected API costs.

**Recommendation**: Use the same `_env()` helper from `oy_cli.py` (or a
shared config parser) with bounds clamping.

---

## Resolved / Changed Since Previous Audit

| # | Previous Finding | Resolution |
|---|-----------------|-----------|
| — | (First full audit) | No prior findings to resolve. Previous `ISSUES.md` at commit `cd63495` was the initial audit; this revision at `f2a023f` re-validates all findings against the current source and updates metrics. |
