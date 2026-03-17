# Contributing

Thanks for contributing to `oy-cli`.

## Development Setup

`mise` manages local tooling and `uv` handles Python environments and packaging.

```bash
mise install
uv sync
```

## Common Commands

```bash
mise run fmt
mise run lint
mise run check
uv run python -m pytest tests/ -v
uv run oy --help
mise run build
```

## Project Notes

- PyPI package: `oy-cli`
- installed command: `oy`
- intended end-user install path: `uv tool install oy-cli`
- current design goal: keep the implementation small and easy to audit
- prefer env-first run configuration so common usage stays close to `oy "prompt"`
- run env vars: `OY_MODEL`, `OY_SHIM`, `OY_NON_INTERACTIVE`, `OY_SYSTEM_FILE`, `OY_ROOT`, `OY_CONFIG`
- tuning env vars: `OY_MAX_TOOL_OUTPUT_TOKENS`, `OY_MAX_TOOL_TAIL_TOKENS`, `OY_MAX_BASH_CMD_BYTES`, `OY_MAX_CONTEXT_TOKENS`, `OY_MAX_MESSAGE_TOKENS`, `OY_DEFAULT_MAX_STEPS`, `OY_DEFAULT_MAX_TOOL_CALLS`, `OY_DEFAULT_LINE_LIMIT`, `OY_BEDROCK_READ_TIMEOUT`, `OY_BEDROCK_MAX_OUTPUT_TOKENS`
- prefer simple, direct changes over abstraction-heavy rewrites
- `except A, B:` syntax is valid Python 3.14+ (PEP 758) -- ruff formats it this way; parenthesised form also works
- keep system prompts tight; avoid duplicating tool docs inside prompts when tool definitions already provide them
- complexity guidance should favor grugbrain.dev style simplicity
- security guidance should explicitly align with OWASP thinking
- performance guidance should reflect performance-aware programming: measure first, avoid obvious waste

## Release Hygiene

- keep `README.md` user-focused
- keep contributor workflow here in `CONTRIBUTING.md`
- make sure `mise run fmt`, `mise run lint`, `mise run check`, and `mise run build` pass before shipping
