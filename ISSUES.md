# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=openai_resp::gpt-5.5 oy audit` · 2026-05-05

## Findings summary

| # | Status | Severity | Finding | Code reference |
|---|---|---|---|---|
| 1 | Partially mitigated in amended 0.8.0 release commit | High | Audit input can disclose unskipped secret-like repository files to the model provider | `src/audit/input.rs::collect_files`, `src/audit/input.rs::should_skip_path`, `src/audit.rs::run` |
| 2 | Open | High | PATH-based `gh` / `aws` discovery can execute attacker-controlled binaries | `src/agent/endpoints.rs::gh_auth_token`, `src/agent/bedrock.rs::aws_cli_available` |
| 3 | Open | Medium | Terminal escape sequences from repo/model/tool output are printed unsanitized | `src/cli/ui.rs::out`, `src/cli/ui/render.rs::markdown`, `src/cli/ui/progress.rs::tool_result` |
| 4 | Fixed in amended 0.8.0 release commit | Medium | Network-disabled policy is not enforced at the `webfetch` sink | `src/tools/network.rs::tool_webfetch`, `src/tools/policy.rs::ToolPolicy` |
| 5 | Fixed in amended 0.8.0 release commit | Medium | `list` can enumerate outside-workspace directories through symlink ancestors | `src/tools/workspace.rs::tool_list` |
| 6 | Open | Medium | Workspace/private file writes have symlink TOCTOU windows | `src/cli/config/paths.rs::resolve_workspace_output_path`, `src/cli/config/paths.rs::write_workspace_file`, `src/cli/config/paths.rs::write_private_file` |
| 7 | Fixed in amended 0.8.0 release commit | Medium | LLM compaction prompt lets untrusted content inject structural pseudo-XML | `src/agent/compaction.rs::transcript_for_summary`, `src/agent/compaction.rs::compaction_prompt` |
| 8 | Fixed in amended 0.8.0 release commit | Medium | SARIF rendering aborts on one model-generated unsafe/malformed code reference | `src/audit/sarif.rs::render_sarif`, `src/audit/sarif.rs::sarif_location` |
| 9 | Fixed in amended 0.8.0 release commit | Medium | Large audit reduce step can silently drop middle findings | `src/audit/reduce.rs::bounded_reduce_findings`, `src/audit/reduce.rs::compact_owned_to_tokens` |
| 10 | Fixed in amended 0.8.0 release commit | Low | Doctor prints copy-pasteable Docker command with shell-interpreted workspace path | `src/cli/app/doctor_cmd.rs::safe_container_command` |

## Validation notes

Validated against this amended 0.8.0 release commit (`chore: release 0.8.0`) on 2026-05-05. The audit findings were accurate for the pre-fix code. The amended release commit remediates low-usability-risk findings #4, #5, #7, #8, #9, and #10, and partially mitigates #1 by broadening audit secret-like filename skips. Issues #2, #3, and #6 remain open because their fixes require more opinionated behavior changes or platform-specific hardening.

## Detailed findings

### 1. Audit input can disclose unskipped secret-like repository files to the model provider

- **Status:** Partially mitigated in amended 0.8.0 release commit.
- **Severity:** High
- **Category:** Data exposure / unsafe file collection, OWASP ASVS V8/V13
- **Trust boundary / sink:** Local repository files → model-provider prompt.
- **Validated evidence:** `src/audit/input.rs::collect_files` collects reviewable files and `src/audit.rs::run` sends chunked file contents to `session::run_prompt_once_no_tools(...)`. The amended 0.8.0 release commit broadens `src/audit/input.rs::should_skip_path` to skip `.env.*` plus filenames containing `credential`, `secret`, or `token`, with regression coverage for `credentials.json` and `secrets.yaml`.
- **Residual impact:** This is still not fail-closed. Secret-bearing files with unrecognized names can still be included if not ignored by gitignore and if their extension is reviewable.
- **Exploitability / preconditions:** User runs `oy audit` in a repository containing secret-bearing files not skipped by gitignore or the hardcoded denylist.
- **Fix:** Keep the current denylist expansion, then add an explicit sensitive-file policy: conservative default skips for common cloud/kube/service-account credential paths, clear audit transparency about skipped sensitive files, and an explicit `--include-sensitive`/`--include-hidden` opt-in with tests.

### 2. PATH-based `gh` / `aws` discovery can execute attacker-controlled binaries

- **Status:** Open.
- **Severity:** High
- **Category:** Process execution from untrusted PATH, OWASP ASVS V12/V14
- **Trust boundary / sink:** User environment / current workspace `PATH` → implicit `Command::new(...)` execution.
- **Validated evidence:** `src/agent/endpoints.rs::gh_auth_token` runs `Command::new("gh").arg("auth").arg("token").output()`. It is reached through GitHub/Copilot token discovery paths. `src/agent/bedrock.rs::aws_cli_available` runs `Command::new("aws").arg("--version")`.
- **Impact:** If `PATH` resolves `gh` or `aws` to an attacker-controlled binary, ordinary model/auth/status discovery can execute arbitrary code with the user’s permissions, outside the tool approval flow.
- **Exploitability / preconditions:** User’s `PATH` contains the workspace or another attacker-writable directory before the real CLI binary.
- **Fix:** Do not auto-run external CLIs during discovery. Require explicit approval, use configured absolute paths, or use SDK/env credential lookup. At minimum, resolve the binary path first and refuse relative or workspace-contained executables.

### 3. Terminal escape sequences from repo/model/tool output are printed unsanitized

- **Status:** Open.
- **Severity:** Medium
- **Category:** Output encoding / terminal injection, OWASP ASVS V7
- **Trust boundary / sink:** Repo file contents, shell/web output, or model text → terminal stdout/stderr.
- **Validated evidence:** `src/cli/ui.rs::out` / `err` print raw strings. `src/cli/ui/render.rs::markdown`, `diff`, and `numbered_block` render raw content lines. `src/cli/ui/progress.rs::tool_result` prints preview lines directly. `src/cli/ui/text.rs::truncate_width` accounts for ANSI width but does not strip or escape control sequences.
- **Impact:** Malicious content containing OSC/CSI/control sequences can clear or spoof the terminal, create deceptive hyperlinks, alter window title, or set clipboard contents when previewed or rendered.
- **Exploitability / preconditions:** User views model/tool/repo output from a malicious repository, fetched page, or command output in a terminal supporting these sequences.
- **Fix:** Sanitize untrusted text at the UI boundary. Allow only ANSI sequences generated by the renderer; escape or replace C0/C1 controls, CSI, OSC, and BEL/ST-terminated sequences in model/tool/repo-originated strings. Keep generated styling separate from untrusted content so sanitization does not strip `oy`'s own color output.

### 4. Network-disabled policy is not enforced at the `webfetch` sink

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Medium
- **Category:** Access control / policy bypass, OWASP ASVS V4/V13
- **Trust boundary / sink:** Model tool call → outbound HTTP request.
- **Validated evidence:** The finding was accurate: `src/tools/network.rs::tool_webfetch` previously ignored the context with `let _ = ctx;`. The amended 0.8.0 release commit now checks `ctx.policy.network != NetworkAccess::Enabled` and fails with `tool denied by policy: webfetch` before URL resolution or network I/O. Regression test: `tools::tests::webfetch_checks_network_policy_at_sink`.
- **Impact if regressed:** The policy boundary would depend only on tool registration/advertisement, allowing direct invocation paths or future refactors to make outbound requests when network is disabled.
- **Fix:** Keep sink-level enforcement and dispatcher gating.

### 5. `list` can enumerate outside-workspace directories through symlink ancestors

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Medium
- **Category:** Filesystem boundary bypass, OWASP ASVS V5/V12
- **Trust boundary / sink:** Model/user-supplied workspace glob → filesystem enumeration.
- **Validated evidence:** The finding was accurate: `src/tools/workspace.rs::tool_list` validated the raw glob path then returned glob matches without canonicalizing each match. The amended 0.8.0 release commit now routes glob results through `safe_list_item`, canonicalizes each match, and drops entries whose resolved path is outside `ctx.root`. Regression test: `tools::tests::list_does_not_follow_symlink_globs_outside_workspace`.
- **Impact if regressed:** A symlink such as `link -> /etc` could let `list path="link/*"` disclose outside-workspace filenames.
- **Fix:** Keep per-result canonicalization. A future cleanup could replace `glob` with an `ignore` walker configured with `follow_links(false)` for one path traversal implementation.

### 6. Workspace/private file writes have symlink TOCTOU windows

- **Status:** Open.
- **Severity:** Medium
- **Category:** Filesystem race / symlink write, OWASP ASVS V12
- **Trust boundary / sink:** Workspace/output path controlled by user/model/local repo state → file write with user permissions.
- **Validated evidence:** `src/cli/config/paths.rs::write_workspace_file` checks `reject_symlink_destination(path)`, creates parents, then opens with `OpenOptions::create(true).write(true).truncate(true)`. `resolve_workspace_output_path` validates existing ancestors but stops at the first missing component. `write_private_file` uses similar create/truncate behavior for private state.
- **Impact:** A concurrent local attacker controlling the workspace can replace the checked path or a newly-created ancestor with a symlink after validation and before open, redirecting writes outside the workspace.
- **Exploitability / preconditions:** Attacker can modify the workspace concurrently during output or replacement writes. For a local single-user CLI this is mainly a hardening issue, but it matters for shared workspaces and untrusted repositories.
- **Fix:** Use race-safe opens: `O_NOFOLLOW` for the final component on Unix, create/open ancestors relative to directory file descriptors with no-follow checks, and revalidate parent directories after creation. Apply equivalent symlink rejection to private config/session writes.

### 7. LLM compaction prompt lets untrusted content inject structural pseudo-XML

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Medium
- **Category:** Prompt-boundary integrity, OWASP ASVS V1/V5
- **Trust boundary / sink:** Untrusted transcript content from user/repo/tool output → compaction prompt → stored summary reused in later context.
- **Validated evidence:** The finding was accurate: `src/agent/compaction.rs::transcript_for_summary` embedded raw message text inside pseudo-XML `<message ...>` tags. The amended 0.8.0 release commit now serializes each transcript item as JSON with `index`, `role`, and escaped `body`, and `compaction_prompt` explicitly tells the model to treat every `body` as untrusted message data.
- **Impact if regressed:** Content such as `</message><message role="user">...` could be interpreted by the summarizing model as transcript structure instead of untrusted message data.
- **Fix:** Keep structured escaping and clear prompt-boundary instructions.

### 8. SARIF rendering aborts on one model-generated unsafe/malformed code reference

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Medium
- **Category:** Audit output integrity / availability, OWASP ASVS V5/V13
- **Trust boundary / sink:** Model-generated markdown finding references → SARIF renderer / CI output.
- **Validated evidence:** The finding was accurate: `src/audit/sarif.rs::render_sarif` previously propagated `sarif_location(&finding.code_ref)?`, so one unsafe path could abort all SARIF output. The amended 0.8.0 release commit now makes `sarif_location` return `Option<Value>`, normalizes safe `./src/...` paths, and emits results without physical locations for unsafe paths. Regression test: `audit::tests::sarif_renderer_omits_unsafe_locations_without_dropping_results`.
- **Impact if regressed:** One malformed or unsafe model-generated code reference could drop all findings from CI/code-scanning output.
- **Fix:** Keep result-level degradation. Consider adding a warning/count in audit transparency output for omitted SARIF locations.

### 9. Large audit reduce step can silently drop middle findings

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Medium
- **Category:** Audit integrity / operational reliability; Grugbrain: `local reasoning`, `small sharp tools`
- **Validated evidence:** The finding was accurate: `src/audit/reduce.rs::compact_owned_to_tokens` previously used `crate::ui::head_tail(text, max_chars)`, preserving only the beginning and end of the combined findings blob. The amended 0.8.0 release commit now splits candidate findings by headings, preserves each heading, trims details per section, and only falls back to head/tail on the already-structured compact form when the prompt is still too large. Regression test: `audit::tests::reduce_findings_compaction_preserves_middle_finding_headings`.
- **Impact if regressed:** Complete findings in the middle of a large audit could be removed before final reduce/ranking.
- **Fix:** Keep structural compaction. For very large repos, consider hierarchical reduce passes by chunk group to retain more details per finding.

### 10. Doctor prints copy-pasteable Docker command with shell-interpreted workspace path

- **Status:** Fixed in amended 0.8.0 release commit.
- **Severity:** Low
- **Category:** Command injection in diagnostic output, OWASP ASVS V5
- **Trust boundary / sink:** Current workspace path → shell command suggested to user.
- **Validated evidence:** The finding was accurate: `src/cli/app/doctor_cmd.rs::safe_container_command` formatted `root.display()` inside double quotes. The amended 0.8.0 release commit now builds the Docker mount string and passes it through `shell_quote`, using single-quote-safe quoting for shell-interpreted paths.
- **Impact if regressed:** A path containing shell substitutions or quotes, e.g. `/tmp/$(touch /tmp/oy-pwn)`, could execute if a user copied the suggested command into a shell.
- **Fix:** Keep shell quoting, or switch the UI to an argv-style command plus separate path field if more shell portability is needed.
