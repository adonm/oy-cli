# Contributing

Thanks for contributing to `oy-cli`.

## Development Setup

Use `uv` for Python environment management, installs, linting, testing, and builds.
Do not use bare `pytest`, `ruff`, `pip`, or ad-hoc virtualenv commands for normal repo workflows.
Prefer `uv sync` and `uv run ...` consistently.

If you use Dev Containers, this repo includes `.devcontainer/devcontainer.json` based on [`wagov-dtt/devcontainer-base`](https://github.com/wagov-dtt/devcontainer-base).

```bash
uv sync
```

## Common Commands

Always run checks through `uv`:

```bash
uv sync
uv run ruff format .
uv run ruff check .
uv run python -m pytest tests/ -v
uv run oy --help
uv build
```

## Project Notes

- PyPI package: `oy-cli`
- installed command: `oy`
- intended end-user install path: `uv tool install oy-cli`
- current design goal: keep the implementation small and easy to audit
- prefer env-first run configuration so common usage stays close to `oy "prompt"`
- run env vars: `OY_MODEL`, `OY_SHIM`, `OY_NON_INTERACTIVE`, `OY_SYSTEM_FILE`, `OY_ROOT`, `OY_CONFIG`
- tuning env vars: `OY_MAX_CONTEXT_TOKENS`, `OY_UNATTENDED_LIMIT`, `OY_MAX_BASH_CMD_BYTES`, `OY_BEDROCK_READ_TIMEOUT`, `OY_BEDROCK_MAX_OUTPUT_TOKENS`
- prefer simple, direct changes over abstraction-heavy rewrites
- `except A, B:` syntax is valid Python 3.14+ (PEP 758) -- ruff formats it this way; parenthesised form also works
- keep system prompts and tool descriptions in `oy_cli/session_text.toml`
- complexity guidance should favor grugbrain.dev style simplicity
- security guidance should explicitly align with OWASP thinking
- performance guidance should reflect performance-aware programming: measure first, avoid obvious waste

## Code Style and Naming

### Naming

- optimize for readability at the call site
- prefer the shortest name that is still obvious in local context
- avoid bloated names when surrounding scope already gives the context; `model`, `shim`, `path`, `root`, `state`, `items`, `result`, and `registry` are usually better than names that repeat the whole story
- keep the same concept named the same way across nearby modules; if something is a registry, call it `registry`, not `specs` in one place and `tools` in another
- prefer nouns for data, verbs for functions, and predicate names for booleans (`is_...`, `has_...`, `can_...`, `should_...`)
- use leading underscores sparingly; in this repo, “internal” mainly means “not exported from top-level modules or `__all__`”, not “hard to access”
- do not add underscore prefixes just to simulate privacy if they make the code harder to read
- internal helpers should still be resilient enough to call directly; do not rely on “private” status for correctness

### Modern Python Style

- target modern Python directly; use `Path`, union syntax (`A | B`), `match`/`case`, context managers, f-strings, comprehensions, plain dict/list structures, and the Python 3.12+ `type Name = ...` statement for shaped data
- prefer typed structured data (`type` aliases, `TypedDict`, small dict/list payloads) plus procedural functions over classes, dataclasses, and method-heavy wrappers
- only keep a class when it is clearly the simplest fit for a real protocol boundary or language requirement (for example, an exception type or required third-party subclass); otherwise collapse behavior into functions over typed data
- when replacing classes, also remove compatibility shims and legacy wrappers instead of layering new procedural code on top of old object APIs
- prefer early returns and flat control flow over deep nesting
- keep functions small enough to scan in one pass, but do not split out one-line helpers with vague names just to make a function shorter
- type hints should help the reader; annotate public and non-trivial internal functions, and prefer `type` aliases for reusable shaped data
- prefer small shaped data structures over passing parallel tuples, loose dicts, or positional blobs around
- make side effects explicit in names and signatures
- avoid clever tricks, abstraction-heavy rewrites, hidden mutation, and framework-style indirection
- prefer simple data flow, explicit error handling, and code that is easy to debug in a terminal

## Release Process

1. **Pre-flight** — all checks must pass:

   ```bash
   uv run ruff check .
   uv run python -m pytest tests/ -v
   uv run oy model
   uv build              # builds wheel + sdist into dist/
   ```

2. **Bump version** in `pyproject.toml`:
   - stable: `"0.5.0"`
   - pre-release: `"0.5.0b1"` (PEP 440 beta)

3. **Commit, tag, push, and release**:

   ```bash
   git add -A && git commit -m "Release v0.5.0"
   git tag v0.5.0
   git push origin main --tags

   # stable
   gh release create v0.5.0 --generate-notes

   # pre-release
   gh release create v0.5.0b1 --prerelease --generate-notes
   ```

   The `release.yml` workflow triggers on the GitHub Release event,
   builds the wheel/sdist, and publishes to PyPI via trusted publishing.

4. **Install and verify**:

   ```bash
   uv tool install --force oy-cli          # stable
   uv tool install --force oy-cli==0.5.0b1 # pre-release
   oy --help
   ```

### Hygiene

- keep `README.md` user-focused
- keep contributor workflow here in `CONTRIBUTING.md`
- make sure checks pass before shipping — use the `uv` commands above before release
