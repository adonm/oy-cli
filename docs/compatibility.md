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

## OpenCode

oy 0.13.7 accepts:

| OpenCode host | Status |
|---|---|
| Current `0.0.0-next-*` channel | Installer default during the V2 beta |
| Tagged OpenCode 2.x | Accepted |
| Other prerelease channels | Rejected |
| OpenCode 1, major versions above 2, or unknown versions | Rejected |

The default executable is `opencode2`. `OY_OPENCODE` can select another executable, but it must report a supported version.

During the V2 beta, installation runs the upstream-documented `npm install -g @opencode-ai/cli@next` under mise's latest Node.js. The plugin SDK resolves from the same moving `next` channel. This keeps new installs current but means an upstream beta change can break compatibility between oy releases. The package lock records the build tested at release time. Restart OpenCode after either package changes.

Once OpenCode 2 is stable, oy will switch these references to the stable `latest` channel and remove the beta-specific compatibility path in a follow-up release.

## What `doctor --check` covers

```bash
oy doctor --check
```

This checks the effective service version, API, location, plugin, `oy` agent, three skills, three commands, models, and providers. It does not validate your permission choices or make a paid/provider-backed model request.

## Setup locations

- Global: `OPENCODE_CONFIG_DIR`, or the platform OpenCode config directory
- Workspace: `OY_ROOT/.opencode/`
- Preferred config file: existing `opencode.jsonc`, otherwise `opencode.json`

Setup preserves unrelated configuration and backs up changed oy-owned entries. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

## Optional tools

`tokei` and Universal Ctags are optional context helpers. Missing them does not block setup, audit, review, or remediation. Install them with:

```bash
oy doctor --install-missing
```

The helper installs prebuilt artifacts only: tokei 12.1.2 through mise's Aqua backend and Universal Ctags release archives from the official nightly-build repository.

## Reporting a compatibility problem

Include:

- `oy --version`;
- the selected OpenCode executable and its `--version` output;
- operating system and architecture;
- install method and setup scope;
- reviewed and redacted `oy doctor --json` output.

Use [OpenCode troubleshooting](https://v2.opencode.ai/troubleshooting) for service/provider issues. Do not include credentials, prompts, or sensitive source text.
