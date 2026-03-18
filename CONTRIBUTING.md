# Contributing

Thanks for contributing to `oy-cli`.

## Development Setup

`mise` manages local tooling and `uv` handles Python environments and packaging.

If you use Dev Containers, this repo includes `.devcontainer/devcontainer.json` based on [`wagov-dtt/devcontainer-base`](https://github.com/wagov-dtt/devcontainer-base).

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
- tuning env vars: `OY_MAX_CONTEXT_TOKENS`, `OY_UNATTENDED_TIMEOUT_SECONDS`, `OY_MAX_BASH_CMD_BYTES`, `OY_BEDROCK_READ_TIMEOUT`, `OY_BEDROCK_MAX_OUTPUT_TOKENS`
- prefer simple, direct changes over abstraction-heavy rewrites
- `except A, B:` syntax is valid Python 3.14+ (PEP 758) -- ruff formats it this way; parenthesised form also works
- keep system prompts tight; avoid duplicating tool docs inside prompts when tool definitions already provide them
- complexity guidance should favor grugbrain.dev style simplicity
- security guidance should explicitly align with OWASP thinking
- performance guidance should reflect performance-aware programming: measure first, avoid obvious waste

## Release Process

1. **Pre-flight** — all checks must pass:

   ```bash
   mise run check        # ruff + pytest + oy model
   mise run build        # builds wheel + sdist into dist/
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
- make sure checks pass before shipping — don't skip `mise run check`
