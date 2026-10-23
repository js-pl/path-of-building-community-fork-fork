#!/usr/bin/env python3
"""Runs the upstream oracle over the corpus. See oracle/README.md.

  run.py dump [--out DIR]        write oracle outputs (default: tests/oracle)
  run.py check                   dump twice and compare with tests/oracle
  run.py coverage [--out DIR]    dump with luacov and write a coverage report
  run.py bench [--out FILE] [--repeats N]
"""

import argparse
import filecmp
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "oracle"
UPSTREAM_SRC = ROOT / "upstream" / "PathOfBuilding" / "src"
PREFIX = Path(os.environ.get("ORACLE_PREFIX", ROOT / ".oracle"))
LUAJIT = PREFIX / "bin" / "luajit"
OUTPUTS = ROOT / "tests" / "oracle"


def corpus():
    with open(ROOT / "corpus" / "corpus.toml", "rb") as f:
        builds = tomllib.load(f)["build"]
    ids = set()
    for b in builds:
        for key in ("id", "path", "source", "license"):
            if not b.get(key):
                sys.exit(f"corpus build {b} has no {key}")
        if not re.fullmatch(r"[a-z0-9.-]+", b["id"]):
            sys.exit(f"corpus id {b['id']!r} must be lowercase letters, digits, '.' and '-'")
        if b["id"] in ids:
            sys.exit(f"duplicate corpus id {b['id']}")
        ids.add(b["id"])
        if not (ROOT / b["path"]).is_file():
            sys.exit(f"corpus build {b['id']}: {b['path']} does not exist (submodule checked out?)")
    return builds


def build_args(builds):
    return [f"{b['id']}={ROOT / b['path']}" for b in builds]


def luajit(script, args, lua_opts=(), env=None):
    if not LUAJIT.exists():
        sys.exit(f"{LUAJIT} not found, run oracle/setup.sh first")
    rocks = PREFIX / "rocks"
    full_env = dict(os.environ)
    full_env.update({
        # upstream's runtime/lua first, as in upstream's .busted
        "LUA_PATH": ";".join([
            "../runtime/lua/?.lua",
            "../runtime/lua/?/init.lua",
            f"{rocks}/share/lua/5.1/?.lua",
            f"{rocks}/share/lua/5.1/?/init.lua",
        ]) + ";;",
        "LUA_CPATH": f"{rocks}/lib/lua/5.1/?.so;;",
    })
    full_env.update(env or {})
    subprocess.run([str(LUAJIT), *lua_opts, str(ORACLE / script), *args],
                   cwd=UPSTREAM_SRC, env=full_env, check=True, stdout=subprocess.DEVNULL)


def dump(out, lua_opts=(), env=None):
    # luajit runs in upstream/src, so paths must be absolute
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for f in out.glob("*.json"):
        f.unlink()
    builds = corpus()
    luajit("dump.lua", [str(out), *build_args(builds)], lua_opts, env)
    written = sorted(f.stem for f in out.glob("*.json"))
    expected = sorted(b["id"] for b in builds)
    if written != expected:
        sys.exit(f"dumper wrote {written}, expected {expected}")


def compare(a, b):
    """Returns the files that differ between two output directories."""
    names = sorted({f.name for f in a.glob("*.json")} | {f.name for f in b.glob("*.json")})
    return [n for n in names
            if not (a / n).exists() or not (b / n).exists() or not filecmp.cmp(a / n, b / n, shallow=False)]


def cmd_dump(args):
    dump(Path(args.out))


def cmd_check(args):
    # Two separate processes get different string hash seeds and so different
    # table iteration orders; both must reproduce the committed outputs.
    failed = False
    with tempfile.TemporaryDirectory() as tmp:
        for run in ("1", "2"):
            out = Path(tmp) / run
            dump(out)
            diff = compare(OUTPUTS, out)
            if diff:
                failed = True
                print(f"run {run}: outputs differ from tests/oracle: {', '.join(diff)}")
    if failed:
        sys.exit("oracle outputs are not reproducible; run `oracle/run.py dump` "
                 "only if the submodule, corpus or dumper changed")
    print("oracle outputs match tests/oracle")


def cmd_coverage(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    for f in ("luacov.stats.out", "luacov.report.out"):
        (out / f).unlink(missing_ok=True)
    env = {"COVERAGE_DIR": str(out), "LUACOV_CONFIG": str(ORACLE / "luacov.lua")}
    dump(out / "outputs", ["-lluacov"], env)
    # Coverage hooks keep LuaJIT mostly interpreting; results must not change
    diff = compare(OUTPUTS, out / "outputs")
    if diff:
        sys.exit(f"outputs under coverage differ from tests/oracle: {', '.join(diff)}")
    rocks = PREFIX / "rocks"
    subprocess.run([str(LUAJIT), "-e", 'require("luacov.runner").run_report()'], cwd=UPSTREAM_SRC, check=True,
                   env={**os.environ, **env,
                        "LUA_PATH": f"{rocks}/share/lua/5.1/?.lua;{rocks}/share/lua/5.1/?/init.lua;;",
                        "LUA_CPATH": f"{rocks}/lib/lua/5.1/?.so;;"})
    report = (out / "luacov.report.out").read_text()
    summary = report[report.index("Summary"):]
    (out / "summary.txt").write_text(summary)
    (out / "uncovered.txt").write_text(uncovered(report))
    print(summary)


def uncovered(report):
    """Lists the lines luacov marks as never run, as `file:line: source`."""
    lines = []
    sections = re.split(r"^={10,}\n(.+)\n={10,}\n", report, flags=re.M)
    for name, body in zip(sections[1::2], sections[2::2]):
        if name == "Summary":
            continue
        for i, line in enumerate(body.splitlines(), 1):
            # luacov right-aligns hit counts in 10 columns and pads misses with *
            if re.match(r"\*+0 ", line):
                lines.append(f"{name}:{i}: {line[10:].strip()}")
    return "\n".join(lines) + "\n"


def cmd_bench(args):
    out = Path(args.out).resolve()
    out.parent.mkdir(parents=True, exist_ok=True)
    luajit("bench.lua", [str(out), str(args.repeats), *build_args(corpus())])
    print(out.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(required=True)
    p = sub.add_parser("dump")
    p.add_argument("--out", default=str(OUTPUTS))
    p.set_defaults(func=cmd_dump)
    p = sub.add_parser("check")
    p.set_defaults(func=cmd_check)
    p = sub.add_parser("coverage")
    p.add_argument("--out", default=str(ROOT / "target" / "oracle-coverage"))
    p.set_defaults(func=cmd_coverage)
    p = sub.add_parser("bench")
    p.add_argument("--out", default=str(ROOT / "target" / "oracle-bench.json"))
    p.add_argument("--repeats", type=int, default=5)
    p.set_defaults(func=cmd_bench)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
