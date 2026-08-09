# Compatibility

## Platforms

| Environment | Support |
|---|---|
| Linux x86_64 with glibc | Full CI and release archive |
| Linux aarch64 with glibc | Release archive; full suite not run on target |
| macOS Apple Silicon | Release archive; full suite not run on target |
| Other Linux/macOS targets | Source build; not release-tested |
| Windows | Use WSL2; native Windows is unsupported |
| Other operating systems | Unsupported at build time |

The installer requires a POSIX shell plus `curl` or `wget`. Its prebuilt oy release supports the three release-archive targets above; other Linux/macOS targets require a source build. Building from source requires Rust 1.96+. The npm plugin declares Linux and macOS support.

## OpenCode and Cursor models

This release accepts:

| OpenCode host | Status |
|---|---|
| Current `0.0.0-next-*` channel | Installer default during the V2 beta |
| Tagged OpenCode 2.x | Accepted |
| Other prerelease channels | Rejected |
| OpenCode 1, major versions above 2, or unknown versions | Rejected |

The default executable is `opencode2`. `OY_OPENCODE` can select another OpenCode executable, but it must report a supported version.

During the V2 beta, installation runs the upstream-documented `npm install -g @opencode-ai/cli@next` under mise's latest Node.js. The version-matched `@oy-cli/opencode` package uses the documented V2 plugin context and includes pinned Cursor provider/SDK dependencies. New installs follow the moving OpenCode beta, so an upstream V2 contract change can still require a compatible oy release. Restart OpenCode after either package changes.

The bundled Cursor provider adapter uses oy's provider-only `@oy-cli/opencode-cursor` fork and pins `@cursor/sdk` 1.0.27. It registers API-key and `CURSOR_API_KEY` connections, exposes Cursor through an authenticated loopback OpenAI Responses route accepted by OpenCode V2, and retries bounded model-catalog refresh failures after first authenticated use. The npm packages require Node.js 24.15 or newer; CI covers current Node 24 and 26 releases. `cursor/*` uses host-capable Cursor tools outside OpenCode permissions; see the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md).

Once OpenCode 2 is stable, oy will switch these references to the stable `latest` channel and remove the beta-specific compatibility path in a follow-up release.

## What `doctor --check` covers

```bash
oy doctor --check
```

This checks the effective service version, API, location, plugin, `oy` agent, three skills, three commands, models, providers, the Cursor provider entry, and its authenticated loopback bridge configuration. It does not validate your permission choices or make a paid/provider-backed model request.

## Setup locations

- OpenCode global: `OPENCODE_CONFIG_DIR`, or the platform OpenCode config directory
- OpenCode workspace: `OY_ROOT/.opencode/`
- OpenCode preferred config file: existing `opencode.jsonc`, otherwise `opencode.json`

Setup preserves unrelated configuration and backs up changed oy-owned entries. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

## Optional tools

`tokei` and Universal Ctags are optional context helpers. Missing them does not block setup, audit, review, or remediation. Install them with:

```bash
oy doctor --install-missing
```

The helper installs prebuilt artifacts only: tokei 12.1.2 through mise's Aqua backend and Universal Ctags release archives from the official nightly-build repository.

The optional helpers are independent of the selected Cursor model provider.

## Reporting a compatibility problem

Include:

- `oy --version`;
- the selected OpenCode executable and its `--version` output;
- operating system and architecture;
- install method and setup scope;
- reviewed and redacted `oy doctor --json` output.

Use [OpenCode troubleshooting](https://v2.opencode.ai/troubleshooting) for service/provider issues. Do not include credentials, prompts, or sensitive source text.
