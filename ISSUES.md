# Audit Findings

> **Last audit**: 2026-03-17 · commit `2a03f49` · OWASP ASVS 5.0 / ASVS 4.0 references checked · [grugbrain.dev](https://grugbrain.dev)
>
> **Codebase**: `oy-cli` v0.3.4alpha — tiny local coding CLI with a small tool surface
>
> | Metric | Value |
> |--------|-------|
> | Python files | 4 |
> | Python LoC | 4,674 non-empty lines |
> | `oy_cli.py` | 1,907 lines (122 functions, 13 classes) |
> | `shim.py` | 2,376 lines (163 functions, 17 classes) |
> | Tests | 391 non-empty lines across 2 files (basic unit coverage) |
> | Provider shims | 6 (`openai`, `codex`, `gemini`, `bedrock`, `bedrock-mantle`, `claude`) |
> | Runtime dependencies | 7 (`boto3`, `defopt`, `msgspec`, `openai`, `rich`, `tiktoken`, `tenacity`) |
>
> **Audit lens**: CLI agent that can read/write workspace files, execute shell commands, make outbound HTTP requests, and persist local auth/config state. Priority was given to workspace-boundary safety, secret handling, least surprise, and keeping a deliberately small codebase from accreting provider-specific complexity.

## H1 · Debug log may persist full prompts, tool outputs, and secrets without permission hardening or redaction

| | |
|---|---|
| **Location** | `oy_cli.py:86-121`, `oy_cli.py:1599-1650` |
| **Category** | Security |
| **Standard** | OWASP ASVS V8.3 / V14.3; grugbrain: avoid clever hidden footguns |
| **Status** | Open |

When `OY_DEBUG` is enabled, the process eagerly opens `~/.config/oy/debug.jsonl` and logs full request/response payloads, including serialized messages and tool results. That can include repository contents, prompts, command output, HTTP bodies, tokens pasted by users, and other sensitive material. Unlike `save_json()` and the shell history path, this file is created via `logging.FileHandler` with no explicit `chmod(0o600)`, so permissions depend on the process umask and any pre-existing file state.

**Recommendation**: Treat debug logging as sensitive data at rest. Enforce `0o600` on creation and on reuse, document the risk in README/help output, and redact obviously sensitive fields before logging. Ideally add a narrower `OY_DEBUG=meta` mode that logs timing and tool names without full content.

---

## H2 · `httpx` tool allows unrestricted outbound network access, including localhost and cloud metadata targets

| | |
|---|---|
| **Location** | `oy_cli.py:1362-1461` |
| **Category** | Security |
| **Standard** | OWASP ASVS V1.14, V5.3 |
| **Status** | Open |

The built-in HTTP fetch tool accepts arbitrary URLs and follows redirects. In an agentic workflow this is effectively an SSRF primitive from the local machine: it can reach `localhost`, RFC1918 addresses, container bridges, and cloud metadata endpoints such as `169.254.169.254`, then return the response to the model. This may be acceptable for a power-user CLI, but it is a meaningful security boundary decision and currently appears unrestricted.

**Recommendation**: Add an opt-in network policy. At minimum support deny-by-default or configurable blocking for loopback, link-local, private IP ranges, and metadata hostnames; re-check redirect targets before following them. If preserving current behavior, document it plainly as a trusted-local-user feature.

---

## H3 · `bash` tool executes arbitrary shell with inherited credentials and broad ambient authority

| | |
|---|---|
| **Location** | `oy_cli.py:1241-1263`, `shim.py:552-565`, `shim.py:571-618` |
| **Category** | Security |
| **Standard** | OWASP ASVS V1.14, V4.3 |
| **Status** | Accepted risk / Open |

`tool_bash()` runs `bash -c` inside the workspace with a fairly complete environment assembled by `command_env()`. That is a core product feature, so the issue is not "shell injection" in the usual sense; the user is explicitly delegating shell execution to the agent. The concern is that the current design gives the model access to everything the invoking user can reach: git credentials, cloud credentials, SSH agents, package managers, local services, and destructive filesystem commands.

**Recommendation**: Keep the feature, but make the risk more explicit and easier to reduce. Provide a documented `--safe` or env-based mode that strips high-risk environment variables, disables network tools, or requires confirmation for destructive commands outside common dev workflows. This is more about safe operation than code correctness.

---

## H4 · Two monolithic modules concentrate most behavior and provider complexity, making review and change-risk high

| | |
|---|---|
| **Location** | `oy_cli.py` (1,907 lines), `shim.py` (2,376 lines) |
| **Category** | Complexity |
| **Standard** | grugbrain: complex thing bad; OWASP ASVS V1.1 (architecture clarity) |
| **Status** | Open |

The project is intentionally small in file count, but most logic lives in two very dense modules containing CLI flow, transcript management, tool dispatch, IO helpers, OAuth helpers, HTTP clients, provider adapters, and Bedrock/OpenAI/Codex/Gemini/Claude specifics. This keeps packaging simple, yet increases reviewer cognitive load and makes subtle regressions more likely because unrelated concerns share the same files and import surface.

**Recommendation**: Split by responsibility, not by framework fashion. For example: `tools_fs.py`, `tools_net.py`, `transcript.py`, and `providers/`. Keep the public surface tiny, but separate high-risk code paths so tests and audits can target them more directly.

---

## H5 · Provider-specific OAuth and model plumbing is drifting toward a mini platform inside `shim.py`

| | |
|---|---|
| **Location** | `shim.py` broadly, especially provider auth/cache sections and client builders |
| **Category** | Complexity |
| **Standard** | grugbrain: one day simple system become complicated mess |
| **Status** | Open |

`shim.py` now owns provider discovery, OAuth device/refresh flows, credential persistence, model caching, retry logic, Bedrock signing, and request adaptation for six providers. None of these pieces are individually unreasonable, but together they make the module harder to understand than the rest of the product concept suggests.

**Recommendation**: Define a narrow provider interface and move each provider’s auth/model quirks behind isolated adapters. The goal is not lots of files; it is to stop every provider from expanding shared branching logic.

---

## M1 · Existing debug logger setup may duplicate handlers on repeated imports in long-lived processes or tests

| | |
|---|---|
| **Location** | `oy_cli.py:86-100` |
| **Category** | Complexity |
| **Standard** | grugbrain: avoid surprising behavior |
| **Status** | Open |

`_init_debug_log()` attaches a `FileHandler` to the named logger each import when `OY_DEBUG` is enabled. In the common CLI path that likely happens once, but in embedded use, reload scenarios, or certain tests this can duplicate log lines and retain file handles longer than expected.

**Recommendation**: Guard against duplicate handlers by checking `logger.handlers` or using a module-local one-shot initializer.

---

## M2 · HTTP clients follow redirects broadly, which expands trust boundaries and complicates reasoning

| | |
|---|---|
| **Location** | `oy_cli.py:1432-1442`, `shim.py:611-618` |
| **Category** | Security |
| **Standard** | OWASP ASVS V5.1, V5.3 |
| **Status** | Open |

Both the user-facing HTTP tool and shared provider HTTP clients enable `follow_redirects=True`. For provider APIs this is usually harmless, but for a general-purpose fetch tool it means an apparently benign URL can bounce into an internal address space or a different host entirely.

**Recommendation**: For the general HTTP tool, prefer manual redirect handling with validation of each hop against the same allow/deny policy. For provider clients, keep redirects only where the endpoint behavior truly requires it.

---

## M3 · Arbitrary file writes do not enforce restrictive mode when creating new files that may contain sensitive content

| | |
|---|---|
| **Location** | `oy_cli.py:1180-1201` |
| **Category** | Security |
| **Standard** | OWASP ASVS V8.3, V14.3 |
| **Status** | Open |

`apply write` and `replace` correctly stay inside the workspace, but newly created files inherit default umask-driven permissions. In normal source repos that is fine; however, this CLI can also be asked to create `.env`, credentials, deploy keys, or other sensitive artifacts during agent sessions.

**Recommendation**: Consider an optional secure-write mode or automatic `0o600` for obviously sensitive filenames (`.env`, `*.pem`, auth/config files). If that feels too opinionated, at least document that generated secret files inherit normal filesystem defaults.

---

## M4 · Response buffering in `httpx` tool is bounded, but still scales linearly with `max_tokens` and holds all chunks in memory

| | |
|---|---|
| **Location** | `oy_cli.py:1430-1459` |
| **Category** | Performance |
| **Standard** | OWASP ASVS V1.2; grugbrain: simple resource limits good |
| **Status** | Open |

The HTTP tool streams responses, but appends chunks to a list and then joins them into a bytes object before rendering. Because `max_bytes = max_tokens * 16`, users can raise memory use by selecting a large `max_tokens` value, and the tool pays for both the list of chunks and the final concatenated bytes.

**Recommendation**: Cap `max_tokens` more aggressively for this tool and use a pre-sized `bytearray` or incremental decoder so only one bounded buffer is retained.

---

## M5 · Tests cover several recent safety fixes, but there is still little direct coverage for the riskiest agent features

| | |
|---|---|
| **Location** | `tests/test_oy_cli.py`, `tests/test_shim.py` |
| **Category** | Complexity |
| **Standard** | OWASP ASVS V1.1.5 |
| **Status** | Open |

Current tests cover path traversal, JSON path depth, immutable command env, and some transcript behavior. That is good. But there is no visible direct test coverage here for debug-log redaction/permissions, HTTP tool redirect/network policy, or the most sensitive persistence paths.

**Recommendation**: Add small focused tests around high-risk invariants: debug log mode/permissions, blocklist checks for unsafe URLs, and permission enforcement for persisted files. These tests will do more for auditability than broad integration tests.

---

## Resolved or improved since the previous audit

- Workspace path resolution now rejects traversal outside the root via `resolve_path()`.
- `glob` results are re-resolved and filtered back to the workspace, reducing symlink/pattern escape risk.
- Sensitive HTTP headers are redacted in rendered output.
- JSON path traversal has a depth cap with test coverage.
- Credential persistence in `shim.py` uses `save_json()` with `chmod(0o600)`.

## Short audit log

- Refreshed audit header for commit `2a03f49` and current code metrics.
- Rechecked repository intent against `README.md` and `CONTRIBUTING.md`.
- Cross-checked findings against OWASP ASVS material and grugbrain simplicity guidance.
- Previous findings not re-added when they appear resolved or materially improved in current code.
