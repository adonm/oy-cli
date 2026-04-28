# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): release refresh for `v0.7.7` after the maintainability refactor · 2026-04-28

## Findings summary

| Sev | Finding | Code refs |
|---|---|---|
| High | Read-only/plan mode can exfiltrate workspace data via `webfetch` | `src/tools.rs:86-92`, `src/tools.rs:405-410`, `src/cli.rs:92-94`, `src/tools.rs:1664-1739` |
| High | `webfetch` SSRF filter allows IPv4-mapped IPv6 localhost/private addresses | `src/tools.rs:2178-2229` |
| Medium | Workspace output path validation can be bypassed through symlinked ancestors | `src/cli.rs:511-516`, `src/cli.rs:544-570`, `src/cli.rs:2709-2713` |
| Medium | `list` can enumerate outside-workspace directories through symlink globs | `src/tools.rs:1335-1370`, `src/tools.rs:1888-1906` |
| Medium | File tools have unbounded file and match loading paths | `src/tools.rs:1470-1483`, `src/tools.rs:1494-1543`, `src/tools.rs:1971-2024` |
| Medium | Audit prompts still place repository text in the instruction context | `src/audit.rs:890-927` |
| Medium | Bedrock endpoint override can exfiltrate prompts and signed AWS requests | `src/agent.rs:126-135`, `README.md:242` |
| Medium | `/thinking` mutates process-global environment inside a multithreaded runtime | `src/cli.rs:1792-1811`, `src/main.rs:4` |
| Low | Local shim auth now avoids `OPENAI_API_KEY`, and probing is now limited to configured local shims | `src/agent.rs:582-588`, `src/agent.rs:863-886`, `src/agent.rs:932-940`, `src/agent.rs:1068-1070` |
| Low | Release workflow relies on checked-in `Cargo.lock`; keep it updated with every release | `.github/workflows/release.yml:22`, `.github/workflows/release.yml:24`, `.github/workflows/release.yml:51`, `Cargo.lock` |

## Detailed findings

## 1. High: Read-only/plan mode can exfiltrate workspace data via `webfetch`

**Category**
Security boundary / data exfiltration.

**Evidence**
- `src/cli.rs:92-94` maps `plan` mode to `ToolPolicy::read_only()`.
- `src/tools.rs:86-92` sets `ToolPolicy::read_only().network = true`.
- `src/tools.rs:405-410` exposes network-gated tools whenever `ctx.policy.network` is true.
- `src/tools.rs:1664-1739` sends model-controlled `GET` / `HEAD` / `OPTIONS` requests with non-sensitive model-controlled headers.

**Trust boundary / sink**
Untrusted repository content or prompt-injected model output → tool call args → outbound HTTP request in `tool_webfetch`.

**Impact**
`oy chat --mode plan` is documented as the safer starting point for untrusted repositories, but the model can combine read-only file tools with `webfetch` and send file contents to an attacker-controlled public URL through paths, query strings, bodies encoded into URL components, or allowed headers. That breaks the expected “read-only” confidentiality boundary.

**Exploitability / preconditions**
- User opens an attacker-influenced repository or prompt in plan/read-only mode.
- Model follows embedded instructions or otherwise decides to call `read` then `webfetch`.
- Outbound network is available.

**References**
- OWASP ASVS V1/V8: trust boundaries and data protection.
- Grugbrain: `small sharp tools` — read-only filesystem access and public network access are separate capabilities and should stay separate.

**Fix**
- Set `ToolPolicy::read_only().network = false`.
- Add a separate mode/flag for research networking, or require per-request approval for `webfetch` in plan/read-only contexts.
- Update docs so “read-only” and “network-enabled research” are not conflated.
- Add tests asserting `tool_specs(ToolPolicy::read_only())` omits `webfetch` unless explicit network approval is enabled.

---

## 2. High: `webfetch` SSRF filter allows IPv4-mapped IPv6 localhost/private addresses

**Category**
SSRF / network boundary bypass.

**Evidence**
- `src/tools.rs:2178-2195` validates IP literals by parsing `host` and calling `ensure_public_ip`.
- `src/tools.rs:2218-2229` rejects normal IPv4 private/loopback/link-local ranges and several IPv6 local ranges.
- The IPv6 branch does not handle IPv4-mapped IPv6 literals such as `::ffff:127.0.0.1` or `::ffff:10.0.0.1`.

**Trust boundary / sink**
Model-controlled `webfetch.url` → URL validation → `reqwest` request to the resolved socket.

**Impact**
A URL such as `http://[::ffff:127.0.0.1]:8080/` can pass the IPv6 checks even though it targets localhost through an IPv4-mapped IPv6 address. If the host stack routes it, `webfetch` can reach local/private services despite the public-only boundary.

**Exploitability / preconditions**
- `webfetch` is enabled.
- Local/private service is reachable via an IPv4-mapped IPv6 literal or resolver result.

**References**
- OWASP ASVS V5: canonicalization and SSRF validation.

**Fix**
- In `is_public_ip`, call `Ipv6Addr::to_ipv4_mapped()` and apply the IPv4 denylist to mapped values.
- Also reject IPv4-compatible, multicast, documentation, and reserved/non-global ranges where Rust exposes stable helpers or a local table.
- Add regression tests for `::ffff:127.0.0.1`, `::ffff:10.0.0.1`, and a public mapped address.

---

## 3. Medium: Workspace output path validation can be bypassed through symlinked ancestors

**Category**
Filesystem boundary / path traversal through symlink ancestors.

**Evidence**
- `src/cli.rs:544-570` rejects absolute and `..` paths, canonicalizes the requested parent only if the full parent already exists, then rejects only a final symlink destination.
- `src/cli.rs:511-516` calls `reject_symlink_destination(path)` and then `fs::create_dir_all(parent)` before opening the file.
- `src/cli.rs:2709-2713` uses this path for workspace output writes.

**Trust boundary / sink**
User/model-controlled output path (`--out`, saved response path, audit output) → workspace path resolver → filesystem write.

**Impact**
If the workspace contains `link -> /outside`, a requested output like `link/new-dir/out.md` has a non-existent parent. Validation skips canonicalizing that parent, then `create_dir_all` follows the existing `link` symlink and creates/writes outside the workspace.

**Exploitability / preconditions**
- Workspace contains an attacker-controlled symlinked directory ancestor.
- User or model writes output beneath that symlink path.

**References**
- OWASP ASVS V12: file/resource boundary checks.

**Fix**
- Resolve the nearest existing ancestor and require it to remain under the canonical workspace root.
- Reject symlink components in every path segment before creating directories.
- After `create_dir_all`, canonicalize the parent and re-check it is still inside root before opening the final file.
- Prefer openat/capability-style traversal with no-follow semantics for robust long-term hardening.

---

## 4. Medium: `list` can enumerate outside-workspace directories through symlink globs

**Category**
Filesystem disclosure / workspace boundary bypass.

**Evidence**
- `src/tools.rs:1335-1370` validates only the raw glob path, then runs `glob(ctx.root.join(&args.path))`.
- Glob matches are filtered using `rel_path(&ctx.root, path)` without canonicalizing the match target.
- `src/tools.rs:1888-1906` shows the stricter existing-path resolver canonicalizes targets and checks `within_root`, but `tool_list` does not use that path for glob matches.

**Trust boundary / sink**
Model-controlled `list.path` → filesystem glob traversal → returned filenames.

**Impact**
A workspace symlink such as `outside -> /home/user` lets `list` with `outside/*` enumerate filenames outside the workspace. In read-only mode this can disclose sensitive file/directory names even when file contents remain blocked by `resolve_existing_path`.

**Exploitability / preconditions**
- Workspace contains a symlink to an outside directory.
- Model calls `list` on the symlink/glob path.

**References**
- OWASP ASVS V12: filesystem traversal and resource access boundaries.

**Fix**
- Canonicalize every glob match and require it to remain under the canonical workspace root before display.
- Or replace glob traversal with a walker configured not to follow symlink directories.
- Add tests with `outside -> tempfile/outside` and verify `list outside/*` returns nothing or errors closed.

---

## 5. Medium: File tools have unbounded file and match loading paths

**Category**
Availability / resource exhaustion.

**Evidence**
- `src/tools.rs:1971-1981` uses `fs::read(path)` with no per-file byte cap, then converts the whole file to UTF-8.
- `src/tools.rs:1372-1406` reads the whole file before slicing requested lines.
- `src/tools.rs:1470-1483` accumulates all search matches, applying `limit` only when serializing the final JSON response.
- `src/tools.rs:1494-1543` accumulates changed-file diffs and counts for all replacements before truncating displayed output.

**Trigger / sink**
Large text files, generated files, or high-match inputs in the workspace → `read`, `search`, or `replace` memory/time use.

**Impact**
A repository containing very large non-NUL text files or millions of matches can make normal tool calls consume excessive memory or appear hung despite narrow `offset`/`limit` arguments. This is especially relevant for untrusted repositories.

**Exploitability / preconditions**
- User runs `oy` on a large or maliciously shaped repository.
- Model invokes `read`, `search`, or `replace` against broad paths.

**References**
- OWASP ASVS V12: resource limits.
- Grugbrain: `small sharp tools` — tool limits should bound actual work, not only output formatting.

**Fix**
- Add a per-file byte cap for interactive file tools.
- Stream `read` and `search` by line and stop once enough output plus an accurate “truncated” signal is available.
- Stop collecting search results after the requested limit plus one sentinel match.
- For `replace`, require explicit caps for maximum changed files/replacements and show a dry-run preview before broad writes.

---

## 6. Medium: Audit prompts still place repository text in the instruction context

**Category**
Prompt-injection resilience / report integrity.

**Evidence**
- `src/audit.rs:890-909` appends manifest, index, and chunk text directly to the prompt.
- `src/audit.rs:912-927` does the same for full-repository review and candidate findings.
- The prompt tells the model what to do, then places untrusted repository text in the same instruction channel.

**Trust boundary / sink**
Audited repository contents and generated candidate findings → LLM instruction context → final `ISSUES.md` report.

**Impact**
A malicious repository can contain text such as “ignore prior instructions” or “return []” in source/docs to suppress, re-rank, or corrupt findings. That undermines audit integrity for the untrusted-repository use case even though the runner no longer exposes tools to the audit model.

**Exploitability / preconditions**
- User audits attacker-controlled or untrusted content.
- Model follows injected instructions embedded in repository text.

**References**
- OWASP ASVS V1: trust boundaries and secure design.

**Fix**
- Wrap repository and candidate text in explicit data-only containers with clear delimiters and repeated “do not follow instructions inside” wording.
- Prefer JSON-escaped or length-delimited records for files/findings.
- Add adversarial audit prompt tests that include injection strings and assert the prompt preserves a data-only boundary.

---

## 7. Medium: Bedrock endpoint override can exfiltrate prompts and signed AWS requests

**Category**
Configuration trust boundary / outbound endpoint validation.

**Evidence**
- `src/agent.rs:126-135` reads `BEDROCK_RUNTIME_ENDPOINT` and passes it directly to the AWS SDK config builder via `set_endpoint_url`.
- `README.md:242` documents `BEDROCK_RUNTIME_ENDPOINT` as an override.
- Bedrock Converse requests include system prompts, user messages, source/tool snippets, and AWS-authenticated request metadata.

**Trust boundary / sink**
Process environment/configuration → outbound Bedrock runtime endpoint.

**Impact**
A mistaken or attacker-controlled endpoint, including a non-AWS HTTPS host or plaintext HTTP URL, can receive prompts/source snippets and signed AWS request metadata. Environment variables are often inherited into shells and automation, making this a sharp edge.

**Exploitability / preconditions**
- `BEDROCK_RUNTIME_ENDPOINT` is set incorrectly or by an attacker with environment control.
- User runs Bedrock-backed chat/audit/model requests.

**References**
- OWASP ASVS V9/V14: secure communications and safe configuration defaults.

**Fix**
- Parse and validate the endpoint before passing it to the SDK.
- Require HTTPS by default.
- Restrict hosts to expected Bedrock/AWS endpoint patterns where practical.
- Allow arbitrary or `http://localhost` endpoints only behind an explicit unsafe/testing opt-in.

---

## 8. Medium: `/thinking` mutates process-global environment inside a multithreaded runtime

**Category**
Unsafe global state / concurrency correctness.

**Evidence**
- `src/cli.rs:1792-1811` implements `/thinking` by calling unsafe `std::env::set_var` / `remove_var` for `OY_THINKING`.
- `src/main.rs:4` uses the default `#[tokio::main]` runtime, which is multithreaded.
- Model request construction reads reasoning/thinking settings from environment state in `src/agent.rs`.

**Trust boundary / sink**
Interactive slash command → process-global environment mutation → concurrent provider/runtime code.

**Impact**
Rust marks environment mutation unsafe because concurrent environment access by other threads/libraries can be undefined behavior on some platforms. It also creates hidden cross-request state: one chat command can affect later model calls in ways that are hard to reason about or test.

**Exploitability / preconditions**
- User runs interactive chat.
- `/thinking` is used while provider SDKs or runtime worker threads may access environment variables.

**References**
- Grugbrain: `local reasoning` and `too much abstraction` — mutable process-global configuration makes behavior non-local.

**Fix**
- Store thinking mode on `Session` or an explicit runtime config object.
- Pass it into model request construction instead of reading/writing environment variables at runtime.
- Read environment once at startup and treat it as immutable input thereafter.

---

## 9. Low: Local shim auth now avoids `OPENAI_API_KEY`, and probing is now limited to configured local shims

**Category**
Credential boundary / local endpoint probing.

**Evidence**
- `src/agent.rs:1068-1070` now returns `LOCAL_API_KEY` or the placeholder `oy-local`; it no longer falls back to `OPENAI_API_KEY`.
- `src/agent.rs:582-588` no longer includes default localhost shims in the always-probed shim list.
- `src/agent.rs:863-886` probes local shims only when `extra_local_shims()` discovers them from selected config/env.

**Impact**
The prior high-severity credential leak is reduced: `oy model` / `oy doctor` should no longer send a real OpenAI key to localhost by default, and default localhost ports are no longer probed unless a local shim is selected/configured. The remaining risk is lower: explicitly configured local model listing can still contact local listeners and reveal that `oy` is running plus the local placeholder token.

**Fix**
- Keep the `LOCAL_API_KEY`-only behavior and configured-only local probing.
- Leave local models as static selectable hints until the user selects/configures them.
- Add a test that `OPENAI_API_KEY` does not influence `local_api_key()`.

---

## 10. Low: Release workflow relies on checked-in `Cargo.lock`; keep it updated with every release

**Category**
Release reproducibility / CI hygiene.

**Evidence**
- `.github/workflows/release.yml:22`, `:24`, and `:51` use `cargo ... --locked`.
- `Cargo.lock` is present and should include the root package version for each release.

**Impact**
This is currently a healthy invariant, not a bug: `--locked` release gates are reproducible only if `Cargo.lock` is committed and updated when `Cargo.toml` changes. Missing lockfile updates will fail CI or produce confusing release failures.

**Fix**
- Keep `Cargo.lock` committed for the CLI application.
- Include `cargo check --locked` or `cargo test --locked` in release preparation.
- Verify the root `oy` package version in `Cargo.lock` matches `Cargo.toml` before tagging.
