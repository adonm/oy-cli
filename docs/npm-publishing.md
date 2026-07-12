# Publishing the OpenCode npm package

The OpenCode plugin is the public scoped package `@oy-cli/opencode` from `packages/opencode`. CI builds, tests, packs, installs, and uploads its tarball on every pull request and `main` push. Tagged releases can publish it with npm trusted publishing after the one-time bootstrap below.

## 1. Own the npm scope

Sign in at [npmjs.com](https://www.npmjs.com/) and create or join the `oy-cli` organization. The package name uses the `@oy-cli` scope, so an npm account named only `adonm` is not sufficient unless it owns or belongs to that organization.

Authenticate locally and confirm the account:

```bash
npm login
npm whoami
```

## 2. Publish the package once

npm needs an existing package before its settings page can configure a trusted publisher. For the first release only:

```bash
cd packages/opencode
npm ci --ignore-scripts
npm run build
npm test
npm publish --access public
```

The package version must not already exist. This repository keeps the Cargo and npm package versions aligned with the release tag.

## 3. Connect npm to GitHub Actions

Open the package on npmjs.com, then select **Settings → Trusted publishing → GitHub Actions** and enter:

| Field | Value |
|---|---|
| Organization or user | `adonm` |
| Repository | `oy-cli` |
| Workflow filename | `release.yml` |
| Environment | `npm` |
| Allowed action | `npm publish` |

The repository workflow already uses a GitHub-hosted runner, Node 24, npm's registry URL, and `id-token: write`. No long-lived `NPM_TOKEN` is needed.

Enable the guarded release job after the trusted publisher is saved:

```bash
gh variable set NPM_PUBLISH_ENABLED --body true --repo adonm/oy-cli
```

Future `v*` tags will then run the `publish-npm` job in `.github/workflows/release.yml`. The job requires the npm version to exactly match the Git tag, runs the locked build/tests, and publishes with automatic npm provenance.

## 4. Lock publishing down

After one trusted publish succeeds, use npm package **Settings → Publishing access** to require two-factor authentication and disallow traditional tokens. Keep the GitHub `npm` environment and tag protection restricted to maintainers.

To disable npm publishing without changing the workflow:

```bash
gh variable delete NPM_PUBLISH_ENABLED --repo adonm/oy-cli
```

The release job is intentionally skipped while that variable is absent or not `true`, so tags remain safe before npm bootstrap is complete.
