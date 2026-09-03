#!/usr/bin/env python3
"""Bump the oy version across every release-facing file.

Usage: python3 scripts/bump_version.py <new-version> [date]

The current version is read from Cargo.toml. All version pins checked by
scripts/check_versions.py are rewritten, and a new CHANGELOG heading is
inserted under the [Unreleased] section. Run `python3 scripts/check_versions.py`
afterwards to verify.
"""

from __future__ import annotations

import re
import sys
import tomllib
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: bump_version.py <new-version> [date]", file=sys.stderr)
        return 2
    new = sys.argv[1]
    old = tomllib.loads(read("Cargo.toml"))["package"]["version"]
    if new == old:
        print(f"already at version {new}", file=sys.stderr)
        return 1
    day = sys.argv[2] if len(sys.argv) > 2 else date.today().isoformat()

    # Cargo package metadata and its lockfile entry.
    write(
        "Cargo.toml",
        re.sub(
            r'^version = "[^"]+"$',
            f'version = "{new}"',
            read("Cargo.toml"),
            count=1,
            flags=re.MULTILINE,
        ),
    )
    lock = read("Cargo.lock")
    lock = lock.replace(
        f'name = "oy-cli"\nversion = "{old}"',
        f'name = "oy-cli"\nversion = "{new}"',
    )
    write("Cargo.lock", lock)

    # Installer pin and versioned documentation examples.
    write("docs/install.sh", read("docs/install.sh").replace(f'oy_version="{old}"', f'oy_version="{new}"'))
    for path in ("docs/getting-started.md", "docs/examples.md"):
        write(path, read(path).replace(f"github:adonm/oy-cli@{old}", f"github:adonm/oy-cli@{new}"))

    # Changelog heading under the [Unreleased] section.
    heading = f"## [{new}] - {day}"
    changelog = read("CHANGELOG.md")
    if heading in changelog:
        print(f"CHANGELOG.md already contains {heading}", file=sys.stderr)
    else:
        changelog = changelog.replace("## [Unreleased]\n", f"## [Unreleased]\n\n{heading}\n", 1)
        write("CHANGELOG.md", changelog)

    print(f"bumped {old} -> {new}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
