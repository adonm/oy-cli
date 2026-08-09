# Publishing the OpenCode npm package

The OpenCode plugin is the public scoped package [`@oy-cli/opencode`](https://www.npmjs.com/package/@oy-cli/opencode) from `packages/opencode`. CI builds, tests, packs, installs, and uploads its tarball on every pull request and `main` push. Tagged releases publish it with npm trusted publishing.

## Trusted publisher

The npm package is connected to GitHub Actions with these values:

| Field | Value |
|---|---|
| Organization or user | `adonm` |
| Repository | `oy-cli` |
| Workflow filename | `release.yml` |
| Environment | `npm` |

The release workflow uses a GitHub-hosted runner, Node 26, an OIDC-capable npm version, the `npm` GitHub environment, and `id-token: write`. No long-lived `NPM_TOKEN` is stored.

## Release behavior

Cargo and npm package versions must match before tagging. On a tagged release, `.github/workflows/release.yml`:

1. builds the platform binaries;
2. checks release-facing versions against the tag;
3. packages the crate and builds, tests, audits, packs, and installs the locked npm package;
4. publishes both packages, skipping an existing npm version only when its `gitHead` matches the tagged commit;
5. publishes the GitHub release after both package jobs succeed.

The crate and npm jobs run independently after the binaries, while the GitHub release waits for both. CI and the release jobs use `scripts/check_versions.py` for the same alignment check.

`oy setup` registers the npm package version matching the CLI, so never publish only one half of a release.
The package vendors its commit-pinned Cursor provider runtime so neither source
installs nor consumers need to permit Git dependencies.

## npm controls

Keep the npm package's trusted publisher restricted to `release.yml` and the `npm` environment. In npm package **Settings → Publishing access**, require two-factor authentication and disallow traditional tokens after trusted publishing has been verified. Restrict GitHub environment and tag administration to maintainers.

To inspect package state without authenticating:

```bash
npm view @oy-cli/opencode version dist.integrity
```

To test a release candidate locally without publishing:

```bash
cd packages/opencode
npm ci --ignore-scripts
npm run build
npm test
npm audit --omit=dev
npm pack --dry-run
```

To test the packed package and its production dependencies:

```bash
cd packages/opencode
tmp=$(mktemp -d ../../.tmp/opencode-package.XXXXXX)
npm pack --pack-destination "$tmp"
cd "$tmp"
npm init --yes >/dev/null
npm install --ignore-scripts ./*.tgz
node --input-type=module -e '
  import("@oy-cli/opencode").then(({ default: plugin }) => {
    if (plugin.id !== "oy") process.exit(1)
    console.log(`loaded ${plugin.id}`)
  })
'
```

## Local OpenCode smoke

Use the checkout plugin with an isolated OpenCode config:

```bash
just opencode-dev
just opencode-dev models
```

The recipe uses the checkout plugin, a private OpenCode server, and ignored
temporary state. If `opencode2` is missing from `node@latest`, it installs the
documented `@opencode-ai/cli@next` package there. This verifies the plugin and
Cursor integration without making a provider request. To test live model
discovery, connect a key with `/connect`, choose Cursor, and inspect the model
picker. Do not paste the key into a config file.
