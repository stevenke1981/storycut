"""Generate a dependency license inventory from the locked local workspace."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def cargo_packages() -> list[tuple[str, str, str, str]]:
    packages: dict[tuple[str, str], tuple[str, str, str, str]] = {}
    for workspace in (ROOT, ROOT / "desktop" / "src-tauri"):
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=workspace,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        metadata = json.loads(result.stdout)
        for item in metadata["packages"]:
            name, version = item["name"], item["version"]
            packages[(name, version)] = ("Rust", name, version, item.get("license") or "UNKNOWN")
    return list(packages.values())


def npm_packages() -> list[tuple[str, str, str, str]]:
    node_modules = ROOT / "desktop" / "node_modules"
    if not node_modules.is_dir():
        raise SystemExit("Run npm ci in desktop/ before generating the inventory")
    packages: dict[tuple[str, str], tuple[str, str, str, str]] = {}
    for directory, subdirs, files in os.walk(node_modules):
        subdirs[:] = [name for name in subdirs if name not in {".bin", "dist", "build"}]
        if "package.json" not in files:
            continue
        package_path = Path(directory) / "package.json"
        try:
            item = json.loads(package_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError):
            continue
        name, version = item.get("name"), item.get("version")
        if not isinstance(name, str) or not isinstance(version, str):
            continue
        license_value = item.get("license") or "UNKNOWN"
        license_name = license_value if isinstance(license_value, str) else "UNKNOWN"
        packages[(name, version)] = ("npm", name, version, license_name)
    return list(packages.values())


def main() -> None:
    entries = sorted(cargo_packages() + npm_packages(), key=lambda row: (row[0], row[1].lower(), row[2]))
    output = ROOT / "docs" / "LICENSE_INVENTORY.md"
    lines = [
        "# Dependency license inventory",
        "",
        "Generated from `cargo metadata --locked` in both Rust workspaces and the installed `desktop/node_modules` tree.",
        "This lists package-declared license expressions; release packaging must include the applicable license texts.",
        "Regenerate after dependency changes with `python -X utf8 scripts/license_inventory.py`.",
        "",
        f"Packages: {len(entries)}. UNKNOWN license declarations: {sum(row[3] == 'UNKNOWN' for row in entries)}.",
        "",
        "| Ecosystem | Package | Version | Declared license |",
        "|---|---|---|---|",
    ]
    for ecosystem, name, version, license_name in entries:
        escaped = [value.replace("|", "\\|") for value in (ecosystem, name, version, license_name)]
        lines.append("| " + " | ".join(escaped) + " |")
    output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Wrote {output} with {len(entries)} packages")


if __name__ == "__main__":
    main()
