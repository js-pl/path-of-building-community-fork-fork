#!/usr/bin/env python3
"""Checks sync/manifest.toml against the upstream submodule: every Lua file
under upstream's src/ is listed, no listed file is missing, statuses are
known and ported files point at existing paths. See docs/design.md.

  sync/check.py            check
  sync/check.py --add      append missing files as status "todo"
"""

import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "upstream" / "PathOfBuilding" / "src"
MANIFEST = ROOT / "sync" / "manifest.toml"
STATUSES = {"ported", "partial", "data", "todo", "skip"}


def upstream_files():
    return sorted(str(p.relative_to(SRC)) for p in SRC.rglob("*.lua"))


def main():
    with open(MANIFEST, "rb") as f:
        manifest = tomllib.load(f)
    files = manifest["files"]
    errors = []
    actual = set(upstream_files())
    missing = sorted(actual - set(files))
    if "--add" in sys.argv and missing:
        with open(MANIFEST, "a") as f:
            for path in missing:
                f.write(f'"{path}" = {{ status = "todo" }}\n')
        print(f"added {len(missing)} files as todo")
        return
    for path in missing:
        errors.append(f"{path}: not in the manifest (run sync/check.py --add)")
    for path, entry in files.items():
        if path not in actual:
            errors.append(f"{path}: listed but not in upstream")
        status = entry.get("status")
        if status not in STATUSES:
            errors.append(f"{path}: unknown status {status!r}")
        if status in ("ported", "partial", "data"):
            if "sync" not in entry:
                errors.append(f"{path}: {status} without a sync commit")
            for port in entry.get("port", "").split():
                if not (ROOT / port).exists():
                    errors.append(f"{path}: port {port} does not exist")
            if not entry.get("port"):
                errors.append(f"{path}: {status} without a port location")
        if status == "skip" and not entry.get("reason"):
            errors.append(f"{path}: skipped without a reason")
    head = subprocess.run(["git", "-C", str(SRC), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if manifest.get("upstream") != head:
        errors.append(f"manifest is for upstream {manifest.get('upstream')}, submodule is at {head}")
    if errors:
        sys.exit("\n".join(errors))
    counts = {}
    for entry in files.values():
        counts[entry["status"]] = counts.get(entry["status"], 0) + 1
    print(f"{len(files)} upstream files: " + ", ".join(f"{n} {s}" for s, n in sorted(counts.items())))


if __name__ == "__main__":
    main()
