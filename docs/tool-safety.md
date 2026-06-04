# Tool safety

`oy` tools run on the user's machine, inside the configured workspace unless noted. Tools are not a sandbox; they use the current user permissions and may expose snippets or command output to the model transcript.

Native OpenAI-compatible tool loops fail closed for repeated identical failed tool calls and long tool-only churn. Tool failures sent back to the model use `TOOL_ERROR` and `RECOVERY` markers, and large model-visible tool outputs are truncated with head/tail preservation before the next provider request. Transient provider retries use jittered backoff and stop after any write, shell, or persistent todo side-effect attempt so a whole prompt is not replayed after local mutation risk.

## Capability matrix

| Tool | Capability | Mutation | Main gate | Notes |
|---|---|---:|---|---|
| `list` | Lists workspace paths by glob | No | Always available | Use first for discovery |
| `read` | Reads one UTF-8 workspace file | No | Always available | Prefer narrow `offset`/`limit` slices |
| `read_multiple_files` | Reads up to 20 UTF-8 workspace files in one call | No | Always available | Per-file `offset`/`limit`/`tail_lines` |
| `search` | Searches workspace text | No | Always available | Prefer literal mode for exact strings |
| `sloc` | Counts source lines with tokei | No | Always available | Useful for sizing and planning |
| `todo` | Manages in-memory todos | No by default | Always available | `persist=true` writes `TODO.md` and uses write approval |
| `ask` | Asks the user in interactive runs | No | Interactive only | Use only for genuine ambiguity |
| `think` | Structured reasoning with numbered thoughts | No | Always available | Survives compaction |
| `outline` | Structural file outline (tree-sitter) | No | Always available | Survey before reading |
| `webfetch` | Fetches web pages with Spider and returns the Spider MCP scrape shape | No local mutation | Network policy | Minimal HTTP-only Spider setup |
| `repo_clone` | Clones/refreshes a git repository into the oy repos cache | Yes (outside workspace) | Network policy | Parses scp-style URLs and `#fragment`; `git` invocations are wrapped in `tokio::time::timeout` (300 s for clone/fetch, 30 s for `rev-parse`); registered as an external side effect |
| `replace` | Replaces text in workspace files | Yes | File-write approval | Inspect/search before changing |
| `patch` | Applies diffs to existing workspace files | Yes | File-write approval | No create/delete; inspect/read before patching |
| `bash` | Runs a shell command in workspace | Process side effects | Shell approval | Filters credential-like env vars; still uses user permissions |

## Approval modes

| Mode | File writes | Shell | Intended use |
|---|---:|---:|---|
| `default` / `ask` | Ask | Ask | Normal trusted work |
| `plan` / `read` | Deny | Deny | First look or untrusted repository |
| `accept-edits` / `edit` | Auto | Ask | Trusted mechanical edits |
| `auto-approve` / `auto` | Auto | Auto | Trusted unattended work only |

Network availability is controlled separately from file-write and shell approval. Treat any mode with both workspace reads and network fetches as able to disclose workspace content if the model is confused by untrusted instructions.

## Filesystem boundary

Workspace tools should only operate within `OY_ROOT` or the current directory. When editing this boundary:

- reject absolute paths and parent traversal where appropriate,
- canonicalize existing paths,
- check symlink ancestors and final destinations,
- keep final writes inside the workspace,
- add tests for traversal, symlinks, and missing parents.

`read` intentionally requires an exact existing file path. Missing-path errors may include fuzzy path suggestions, but the suggested file is not read until the model/user sends a follow-up `read` call with that exact path.

## Network boundary

`webfetch` is for public documentation and public API research. It uses Spider's default HTTP crawler setup and the HTTP-compatible part of the `spider_mcp` `spider_scrape` argument/output shape (`url`, optional `return_format`, `user_agent`, and `cookie`) while this build remains without Chrome/wait/proxy support. When changing it:

- keep the model-visible schema limited to fields this build actually applies,
- keep Spider setup simple and HTTP-only,
- add regression tests for webfetch defaults and output preview shape.

The public-target boundary is single-sourced: `tools::network::validate_public_url_target` runs the scheme allowlist, host validation, and per-socket `validate_public_ip` for both the initial request resolve and the redirect policy, so a public-at-first host cannot drift to a private-IP target through a redirect. DNS pinning is the async path (`resolve_public_addrs`); the redirect closure is sync because `reqwest::redirect::Policy::custom` cannot be async, so a single redirect does a brief blocking DNS lookup. Tighten the allowlist in the helper and both paths follow.

## Shell boundary

`bash` is the highest-risk tool. It can read credential files, modify files, contact networks, start processes, and affect the host outside the repo. `oy` removes credential-like environment variables from child processes by default, but shell is still not sandboxed. Terminal/control sequences in stdout/stderr are passed through raw for bat/terminal formatting. Keep shell use explicit:

- ask by default,
- deny in read-only modes,
- include the command in the approval preview,
- prefer file tools for inspection and small edits,
- avoid destructive commands unless explicitly requested.

## Retry boundary

Provider retries happen outside the tool loop and can otherwise replay a whole prompt. `tools::invoke_inner` records external side-effect attempts before dispatching `bash`, `replace`, `patch`, `repo_clone`, or `todo` with `persist=true`. Once recorded, transient provider failures are returned to the user instead of retrying the prompt. Keep this list aligned with any new tool that can mutate files, start processes, persist local state, or affect systems outside the transcript.

## Audit disclosure boundary

`oy audit` has no model tools, but it sends collected file text to the model provider. Audit collection should skip build outputs, dependencies, lockfiles, hidden or likely-secret files by default, and should explain any option that includes more data.

Audit review input is not compacted or truncated to fit model context. If a file/chunk is too large for the derived budget, audit fails closed and asks for a narrower scope or larger-context model.
