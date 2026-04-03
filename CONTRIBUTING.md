# Contributing

Thanks for contributing to `oy-cli`.

## Development loop

The Go port is the primary target. Keep the Python baseline green until final retirement.

```bash
go test ./...
go build ./cmd/oy
./oy --help
uv run pytest -q
```

## Repo layout

- `cmd/oy/main.go` — Go CLI entrypoint
- `internal/oy/cli/` — command entrypoints and chat/session UX
- `internal/oy/agent/` — transcript handling and agent loop
- `internal/oy/runtime/` — prompt text, config, and runtime helpers
- `internal/oy/tools/` — model-exposed tools plus local file/search/web helpers
- `internal/oy/providers/` — provider shims, auth, HTTP helpers, and model discovery
- `internal/oy/aws/` — Bedrock SigV4 signing support
- `oy_cli/` — legacy Python baseline kept temporarily for parity checks
- `tests/` — pytest coverage for the Python baseline during migration
- `GO_PORT_TRACKER.md` — migration checklist and progress log

## Working rules

- package / command: `oy-cli` / `oy`
- prefer the Go implementation for new behavior
- keep the implementation small, direct, and easy to audit
- prefer env-first configuration so common usage stays close to `oy "prompt"`
- keep prompt text and tool descriptions in `internal/oy/runtime/session_text.toml`
- when docs, tests, and behavior disagree, fix them together
- prefer simple changes over abstraction-heavy rewrites
- keep security guidance OWASP-minded and performance guidance measurement-first
- update `GO_PORT_TRACKER.md` as milestones move
- commit after significant pieces of work

## Style

- optimize for readability at the call site
- prefer short obvious names in local context
- keep the same concept named the same way across nearby modules
- prefer nouns for data, verbs for functions, and predicate names for booleans
- prefer typed data plus procedural functions over wrappers unless a class is clearly simpler
- prefer early returns and flat control flow
- avoid clever tricks, hidden mutation, and framework-style indirection

## Change checklist

- keep `README.md` user-focused and task-oriented
- keep contributor workflow here in `CONTRIBUTING.md`
- keep `/ask` wording explicit: no-write, but public `webfetch` is still allowed
- add or extend focused Go tests next to changed behavior
- keep `uv run pytest -q` passing while Python remains in-tree
- run targeted tests before broader checks when iterating

## Release process

1. Run pre-flight checks:

   ```bash
   go test ./...
   go build ./cmd/oy
   uv run pytest -q
   ```

2. Update release metadata as needed.
3. Commit, tag, push, and create the GitHub release.
4. Verify the produced binary with `./oy --help`.

The `release.yml` workflow now builds and uploads a Go binary artifact.
