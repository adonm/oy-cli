# Audit Findings

> **Last audit**: 2026-03-17 · commit `cd63495` · OWASP ASVS 5.0.0 · [grugbrain.dev](https://grugbrain.dev)
>
> **Codebase**: `oy-cli` v0.3.3 — tiny AI coding assistant (2 source files, 4 Python total)
>
> | Metric | Value |
> |--------|-------|
> | Python LoC | 4,610 |
> | `oy_cli.py` | 2,138 lines (121 functions, 13 classes) |
> | `shim.py` | 2,586 lines (152 functions, 17 classes) |
> | Test LoC | ~440 lines across 2 files |
> | Provider shims | 6 (openai, codex, gemini, bedrock, bedrock-mantle, claude) |
> | Dependencies | 8 runtime (`boto3`, `defopt`, `httpx`, `markdownify`, `openai`, `rich`, `tiktoken`, `tenacity`, `msgspec`) |

## Summary

Total issues: 13 (3 from previous audit confirmed resolved, 10 new/carried)

| Severity | Open | Resolved |
|----------|------|----------|
| High     | 1    | 2        |
| Medium   | 4    | 3        |
| Low      | 5    | 5        |

---

## Open Issues

### H1. `httpx` tool has no SSRF protection — no internal-network filtering

- **Location**: `oy_cli.py` — `tool_httpx` (line ~1418)
- **Category**: security
- **Reference**: OWASP ASVS V14.5.1 (SSRF Prevention), CWE-918
- **Status**: **Open** (carried from previous audit as H2)

The `httpx` tool validates the URL scheme (`http`/`https`) but does **not** block
requests to localhost (`127.0.0.1`, `::1`), link-local, or RFC 1918 private ranges.
An LLM-directed tool call like `httpx http://169.254.169.254/latest/meta-data/` could
leak cloud instance metadata (AWS IMDSv1, GCP, Azure).

**Recommendation**: Resolve the hostname before connecting and reject addresses in
`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`,
`::1`, and `fe80::/10`. Use Python's `ipaddress.ip_address(...).is_private` as a
starting point.

---

### M1. `shim.py` is a 2,586-line monolith with 6 provider backends

- **Location**: `shim.py` (entire file)
- **Category**: complexity
- **Reference**: grugbrain.dev — "complexity is the single biggest enemy"; "split code into smaller files when it gets too big"
- **Status**: **Open** (new)

`shim.py` contains 152 functions and 17 classes covering shared abstractions *and*
six distinct provider implementations (OpenAI, Codex, Gemini, Bedrock, Mantle, Claude).
Each provider has 8–12 functions handling auth, token refresh, request encoding, retry
logic, and model listing. This makes it hard to audit a single provider in isolation
and increases the risk of cross-contamination when changing one backend.

**Recommendation**: Extract each provider into its own module under a `shims/` package
(e.g. `shims/openai.py`, `shims/gemini.py`). Keep the shared types, `CompletionClient`,
`HttpChatEndpoint`, retry logic, and codec machinery in `shims/base.py`. The `ShimSpec`
registry at the bottom already provides clean boundaries — use them as the split points.

---

### M2. Readline history file written without restrictive permissions

- **Location**: `oy_cli.py` — `_setup_readline` (line ~1758)
- **Category**: security
- **Reference**: OWASP ASVS V8.3.4 (Sensitive Data in Storage), CWE-532
- **Status**: **Open** (new)

The chat history file (`~/.config/oy/history`) is created by `readline.write_history_file`
which inherits the process umask (often `0o022`, giving world-readable `0o644`).
Chat history may contain sensitive prompts, code snippets, API keys pasted by the user,
or internal project details. On the test system the file happened to be `0o600` but
this is not guaranteed.

**Recommendation**: After the first `readline.write_history_file` call (or via the
`atexit` handler), explicitly `os.chmod(history_path, 0o600)` — matching the pattern
already used by `save_json` for credential files.

---

### M3. Unbounded `_json_path` traversal can consume CPU on deeply nested payloads

- **Location**: `oy_cli.py` — `_json_path` (line ~315)
- **Category**: performance
- **Reference**: CWE-400 (Uncontrolled Resource Consumption)
- **Status**: **Open** (new)

`_json_path` recursively descends into arbitrary JSON structures returned by HTTP
fetches. A malicious or very deeply nested JSON response combined with a dot-separated
path could cause excessive recursion or processing. Additionally, list index access
`v[int(part)]` has no bounds check beyond Python's own `IndexError`.

**Recommendation**: Cap the depth of `_json_path` traversal (e.g. 20 levels) and
catch `IndexError` to return a clear error message. The function is already small —
this is a one-line guard.

---

### M4. `oy_cli.py` unconditionally imports `readline` at module level

- **Location**: `oy_cli.py` — line 7
- **Category**: complexity
- **Reference**: grugbrain.dev — "don't abstract too early"
- **Status**: **Open** (new)

`import readline` at the top of `oy_cli.py` means the module is loaded even for
non-interactive commands (`oy "prompt"`, `oy model`, `oy audit`). On minimal Python
builds (Alpine musl, some containers, WASM), `readline` may be absent, causing an
immediate crash even for non-interactive usage.

**Recommendation**: Move `import readline` into `_setup_readline()` where it is
actually needed, with a try/except `ImportError` fallback.

---

### L1. Debug log file may persist sensitive conversation data in `/tmp`

- **Location**: `oy_cli.py` — `_init_debug_log` (line ~92)
- **Category**: security
- **Reference**: OWASP ASVS V8.3.1 (Sensitive Data Logging), CWE-532
- **Status**: **Open** (new)

When `OY_DEBUG=1`, a JSONL file is created in `/tmp` via `tempfile.mkstemp` with a
predictable prefix (`oy-debug-`). The file receives the full conversation transcript
including all tool call arguments (file contents, bash commands, HTTP responses).
While `mkstemp` creates the file with `0o600`, the `/tmp` location and predictable
naming make it easy to forget about. No automatic cleanup or size limit exists.

**Recommendation**: Write the debug log to `~/.config/oy/debug.jsonl` (alongside other
oy data) or at least document the location clearly. Consider adding a rotation/size cap.

---

### L2. No response size limit on `httpx` tool fetches

- **Location**: `oy_cli.py` — `tool_httpx` (line ~1422)
- **Category**: performance
- **Reference**: CWE-400 (Uncontrolled Resource Consumption)
- **Status**: **Open** (new)

The `httpx` tool fetches the entire HTTP response body into memory before truncating
it via `clip_tokens`. A malicious or accidental URL pointing to a multi-GB file
(e.g. a binary download) would consume unbounded memory.

**Recommendation**: Use `httpx`'s streaming API or set `max_content_length` to a
reasonable cap (e.g. 2MB) before reading the body.

---

### L3. `lru_cache` on `command_env` caches mutable `os.environ.copy()`

- **Location**: `shim.py` — `command_env` (line ~568)
- **Category**: complexity
- **Reference**: grugbrain.dev — "caching is a classic complexity devil"
- **Status**: **Open** (new)

`command_env` is decorated with `@lru_cache(maxsize=8)` and returns a dict built from
`os.environ.copy()`. The comment acknowledges the cache may be stale if env vars change
mid-process. Because the returned dict is mutable, a caller modifying the returned dict
would corrupt the cached value for all subsequent callers.

**Recommendation**: Either return a frozen mapping (`types.MappingProxyType`) so callers
cannot mutate the cache, or drop the cache and accept the negligible re-scan cost
(the function runs once per tool call, not in a hot loop). Given the project's
simplicity goals, dropping the cache is simpler.

---

### L4. Dual tool-call argument aliases add surface area

- **Location**: `shim.py` — lines 1643–1644
- **Category**: complexity
- **Reference**: grugbrain.dev — "avoid the temptation to have two ways of doing things"
- **Status**: **Open** (new)

```python
parse_tool_call_arguments = _decode_tool_call_arguments
_responses_output_to_message = _decode_responses_output
```

These module-level aliases create two names for the same function. `parse_tool_call_arguments`
is the public name but `_decode_tool_call_arguments` is already used directly elsewhere.
This adds cognitive overhead when auditing call sites.

**Recommendation**: Pick one canonical name per function and use it everywhere.

---

### L5. Test coverage is thin relative to security surface

- **Location**: `tests/` (~440 lines total)
- **Category**: security / complexity
- **Reference**: OWASP ASVS V1.1.7 (Threat modelling and security testing)
- **Status**: **Open** (new)

Tests cover tool dispatch, transcript management, config handling, and shim wiring,
but there are no tests for:
- `resolve_path` boundary enforcement (symlink traversal, `..` escapes)
- `tool_httpx` URL validation edge cases
- `tool_apply` write/overwrite/move/delete with adversarial paths
- `_json_path` with deeply nested or malformed inputs
- `clip_tokens` truncation correctness at boundary values

**Recommendation**: Add targeted security-boundary tests for `resolve_path`,
`tool_httpx`, and `tool_apply`. These are the main trust boundaries between
LLM-generated tool calls and the filesystem/network.

---

## Resolved Issues (from previous audit 2025-07-15)

### ✅ H1. Path traversal in file tools (resolved in v0.3.1)

`resolve_path` now correctly blocks `..` traversal and symlink escapes.
Glob results are filtered post-resolve to stay within the workspace root.
Verified: symlinks pointing outside the workspace are rejected.

### ✅ H3. Default model was hardcoded (resolved in v0.3.0)

No default model is hardcoded. Users must configure one via `oy model` or `OY_MODEL`.

### ✅ M1. OAuth client IDs/secrets in source (resolved — not a finding)

The embedded OAuth client IDs and secrets are public "installed app" credentials per
RFC 8252 §8.5 and Google's native-app OAuth documentation. Clear source comments
explain this. Not a security issue.

### ✅ M2. `save_json` now sets `0o600` on credential files (resolved in v0.3.1)

All credential and config files written by `save_json` get `chmod(0o600)`.

### ✅ M3. Config directory created with explicit permissions (resolved in v0.3.1)

`save_json` creates parent directories and restricts the file itself to `0o600`.

### ✅ L1–L5 from previous audit

Previous low-severity items (error message leakage, broad exception catches,
missing input length limits on tool args, env var documentation, CI workflow
permission scoping) have all been addressed or accepted as negligible risk.

---

## Audit Methodology

1. **Structure review**: Read README.md, CONTRIBUTING.md, pyproject.toml, and all
   source files (`oy_cli.py`, `shim.py`, tests, CI workflows).
2. **Standards consulted**:
   - OWASP ASVS 5.0.0 (May 2025) — chapters on input validation (V5), authentication
     (V2), session management (V3), stored cryptography (V6), error handling (V7),
     data protection (V8), HTTP security (V14).
   - [grugbrain.dev](https://grugbrain.dev) — complexity budgets, abstraction limits,
     "say no to complexity", caching as a complexity source, file splitting heuristics.
3. **Tool-assisted checks**: `scc` for size metrics, `grep`/`rg` for pattern scanning,
   `pytest` for test pass verification, manual symlink/permission verification.
4. **Scope**: Security (SSRF, path traversal, credential handling, file permissions),
   complexity (file size, abstraction layers, naming), and performance (unbounded
   resource consumption, caching correctness).
