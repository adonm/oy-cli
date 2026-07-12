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

The release workflow uses a GitHub-hosted runner, Node 24, an OIDC-capable npm version, the `npm` GitHub environment, and `id-token: write`. No long-lived `NPM_TOKEN` is stored.

## Release behavior

Cargo and npm package versions must match the `v*` tag. On a tagged release, `.github/workflows/release.yml`:

1. builds the platform binaries;
2. installs locked npm dependencies and runs the plugin tests;
3. publishes `@oy-cli/opencode` through npm OIDC, or skips an already-published version only when its `gitHead` matches the tagged commit;
4. publishes the GitHub release only after npm succeeds;
5. publishes the matching crate through crates.io trusted publishing.

The curl installer and `oy setup` pin the npm plugin version matching the binary, so never publish only one half of a release.

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
npm pack --dry-run
```
