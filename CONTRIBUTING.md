# Contributing

Thanks for contributing to `oy-cli`.

## Development setup

Use `uv` for installs, formatting, linting, tests, and builds.

```bash
uv sync
uv run ruff check .
uv run pytest -q
uv run pytest tests/test_providers.py -q
uv run oy --help
uv build
```

## Repo layout

- `oy_cli/cli.py` — command entrypoints and chat UX
- `oy_cli/agent.py` — transcript handling and agent loop
- `oy_cli/runtime.py` — prompt text, rendering, config, and runtime helpers
- `oy_cli/tools.py` — model-exposed tools plus local file/search/web helpers
- `oy_cli/providers.py` — provider shims, auth, HTTP helpers, and model discovery
- `tests/` — pytest coverage plus shared helpers in `tests/conftest.py`

## Working rules

- package / command: `oy-cli` / `oy`
- install path: `uv tool install oy-cli`
- keep the implementation small, direct, and easy to audit
- prefer env-first configuration so common usage stays close to `oy "prompt"`
- keep prompt text and tool descriptions in `oy_cli/session_text.toml`
- prefer shared helpers in `tests/conftest.py` over repetitive setup
- when docs, tests, and behavior disagree, fix them together
- prefer simple changes over abstraction-heavy rewrites
- collapse repeated helper code when it makes nearby call sites shorter and clearer
- keep security guidance OWASP-minded and performance guidance measurement-first

`except A, B:` syntax is valid Python 3.14+ (PEP 758); ruff may format it that way.

## Style

- optimize for readability at the call site
- prefer short obvious names in local context
- keep the same concept named the same way across nearby modules
- prefer nouns for data, verbs for functions, and predicate names for booleans
- target modern Python directly: `Path`, union syntax, `match`, comprehensions, context managers, and f-strings
- prefer typed data plus procedural functions over wrappers unless a class is clearly simpler
- prefer early returns and flat control flow
- avoid clever tricks, hidden mutation, and framework-style indirection

## Release process

1. Run pre-flight checks:

   ```bash
   uv run ruff check .
   uv run python -m pytest tests/ -v
   uv run oy model
   uv build
   ```

2. Bump `version` in `pyproject.toml`.
3. Commit, tag, push, and create the GitHub release.
4. Install the published build and verify `oy --help`.

The `release.yml` workflow builds the wheel/sdist and publishes to PyPI via trusted publishing.

## Hygiene

- keep `README.md` user-focused and task-oriented
- keep contributor workflow here in `CONTRIBUTING.md`
- keep `/ask` wording explicit: no-write, but public `webfetch` is still allowed
- prefer adding or extending focused regression tests next to the behavior you changed
