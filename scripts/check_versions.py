#!/usr/bin/env python3
"""Fail when release-facing version pins disagree with Cargo.toml."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def main() -> int:
    version = tomllib.loads(read("Cargo.toml"))["package"]["version"]
    package = json.loads(read("packages/opencode/package.json"))
    package_lock = json.loads(read("packages/opencode/package-lock.json"))
    installer = re.search(r'^oy_version="([^"]+)"$', read("docs/install.sh"), re.MULTILINE)

    checks = {
        "packages/opencode/package.json": package["version"],
        "packages/opencode/package-lock.json": package_lock["version"],
        "packages/opencode/package-lock.json root package": package_lock["packages"][""]["version"],
        "docs/install.sh": installer.group(1) if installer else "<missing oy_version>",
    }
    errors = [
        f"{name}: expected {version}, found {actual}"
        for name, actual in checks.items()
        if actual != version
    ]

    required_text = {
        "CHANGELOG.md": f"## [{version}]",
        "docs/getting-started.md": f"github:adonm/oy-cli@{version}",
        "docs/examples.md": f"github:adonm/oy-cli@{version}",
        "packages/opencode/README.md": f"@oy-cli/opencode@{version}",
    }
    errors.extend(
        f"{path}: missing {needle}"
        for path, needle in required_text.items()
        if needle not in read(path)
    )

    if errors:
        print("release version alignment failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"release version alignment passed: {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
