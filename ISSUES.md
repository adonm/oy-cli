# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `oy audit` · 2026-05-09
>
> Historical review-fix details are in `CHANGELOG.md`. Current open items are in `REVIEW.md`.

## Triage

| Status | Finding |
|---|---|
| Fixed | `webfetch` public-IP classification now delegates to `ip_rfc::global` with explicit denials for multicast and deprecated IPv6 site-local ranges |
| Fixed | Shell child processes remove credential-like environment variables before launch |
| Context | Terminal/output escape safety is sink-specific; direct metadata sinks escape ESC, raw content previews go through bat/terminal rendering intentionally |
| Open | See `REVIEW.md` (2026-06-04) for five current maintainability findings |

## Audit findings

### Fixed: `webfetch` public-IP classification uses maintained global-address semantics

`src/tools/network.rs::is_public_ip` delegates to `ip_rfc::global` plus boundary-specific denials. Regression tests cover private, shared, loopback, link-local, documentation, benchmarking, protocol-assignment, multicast, reserved, deprecated site-local, and representative public ranges.

### Context: Terminal/output escape safety is sink-specific

Direct progress/error metadata escapes ESC before writing to the terminal. Content previews (markdown, diffs, code blocks, verbose bash output) are rendered through bat-backed paths where ANSI/terminal bytes are preserved for formatting. The rule: escape untrusted metadata before direct terminal sinks; do not pre-escape content handed to bat/terminal rendering.

### Fixed: Shell child processes remove credential-like environment variables

`bash` child processes remove credential-like environment variables before launch. Regression coverage confirms secret-like env vars are removed while non-secret env vars remain visible.

## Current work

See `REVIEW.md` for five open maintainability findings from the 2026-06-04 review and `ROADMAP.md` for the three-month plan to address them.
