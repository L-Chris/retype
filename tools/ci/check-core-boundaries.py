#!/usr/bin/env python3
"""Reject first-party platform dependencies from any core package."""
import json
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[2]
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root
))
violations = []
for package in metadata["packages"]:
    manifest = Path(package["manifest_path"])
    if not manifest.is_relative_to(root / "core"):
        continue
    for dependency in package["dependencies"]:
        path = dependency.get("path")
        if path and Path(path).resolve().is_relative_to(root / "platforms"):
            violations.append(f"{package['name']} -> {dependency['name']} ({path})")
if violations:
    raise SystemExit("Core must depend on shared modules, not platform adapters:\n" + "\n".join(violations))
print("Core boundaries verified: no platform adapter dependencies")
