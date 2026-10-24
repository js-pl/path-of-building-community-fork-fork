#!/usr/bin/env python3
"""Runs the upstream oracle over the corpus. See oracle/README.md.

  run.py generate                regenerate corpus/generated/ from corpus/generate.lua
  run.py dump [--out DIR]        write oracle outputs (default: tests/oracle),
                                 including modparser.jsonl
  run.py check                   dump twice and compare with tests/oracle
  run.py coverage [--out DIR]    dump with luacov and write a coverage report
  run.py data-json [--out DIR]   write upstream's data tables as JSON, to debug
                                 differences in tests/oracle/data.jsonl

Work is split into one LuaJIT process per CPU (--jobs to change).
"""

import argparse
import filecmp
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "oracle"
UPSTREAM_SRC = ROOT / "upstream" / "PathOfBuilding" / "src"
PREFIX = Path(os.environ.get("ORACLE_PREFIX", ROOT / ".oracle"))
LUAJIT = PREFIX / "bin" / "luajit"
ROCKS = PREFIX / "rocks"
OUTPUTS = ROOT / "tests" / "oracle"
CORPUS = ROOT / "corpus"
GENERATED = CORPUS / "generated"

JOBS = os.cpu_count() or 1


def load_corpus():
    with open(CORPUS / "corpus.toml", "rb") as f:
        return tomllib.load(f)


def corpus():
    """All corpus builds: the listed ones plus the generated ones."""
    config = load_corpus()
    builds = list(config["build"])
    gen = config["generated"]
    for seed in range(gen["first_seed"], gen["first_seed"] + gen["count"]):
        builds.append({
            "id": f"gen-{seed:04d}",
            "path": f"corpus/generated/gen-{seed:04d}.xml",
            "source": f"corpus/generate.lua, seed {seed}",
            "license": "MIT, this project",
        })
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
            sys.exit(f"corpus build {b['id']}: {b['path']} does not exist "
                     "(submodule checked out? `run.py generate` run?)")
    return builds


def build_args(builds):
    return [f"{b['id']}={ROOT / b['path']}" for b in builds]


def lua_env(extra=None):
    env = dict(os.environ)
    env.update({
        # upstream's runtime/lua first, as in upstream's .busted
        "LUA_PATH": ";".join([
            "../runtime/lua/?.lua",
            "../runtime/lua/?/init.lua",
            f"{ROCKS}/share/lua/5.1/?.lua",
            f"{ROCKS}/share/lua/5.1/?/init.lua",
        ]) + ";;",
        "LUA_CPATH": f"{ROCKS}/lib/lua/5.1/?.so;;",
    })
    env.update(extra or {})
    return env


def luajit(script, args, lua_opts=(), env=None):
    if not LUAJIT.exists():
        sys.exit(f"{LUAJIT} not found, run oracle/setup.sh first")
    result = subprocess.run([str(LUAJIT), *lua_opts, str(script), *args],
                            cwd=UPSTREAM_SRC, env=lua_env(env), stdout=subprocess.DEVNULL,
                            stderr=subprocess.PIPE, text=True)
    if result.returncode != 0:
        sys.exit(f"{script.name} failed:\n{result.stderr}")


def shards(items, jobs):
    """Splits items round-robin into at most `jobs` non-empty lists."""
    return [s for s in (items[i::jobs] for i in range(jobs)) if s]


def parallel(func, items):
    with ThreadPoolExecutor(len(items)) as pool:
        return list(pool.map(func, items))


def dump(out, jobs, lua_opts=(), env_for_shard=None, modparser=False):
    # luajit runs in upstream/src, so paths must be absolute
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for f in out.glob("*.json"):
        f.unlink()
    builds = corpus()
    if modparser:
        # Runs next to the build dumps; it parses the corpus builds' lines too
        (out / "modparser.jsonl").unlink(missing_ok=True)
        (out / "data.jsonl").unlink(missing_ok=True)
        jobs = max(1, jobs - 2)

    def run(indexed):
        index, shard = indexed
        env = env_for_shard(index) if env_for_shard else None
        luajit(ORACLE / "dump.lua", [str(out), *build_args(shard)], lua_opts, env)

    tasks = list(enumerate(shards(builds, jobs)))
    if modparser:
        tasks.append((-1, "modparser"))
        tasks.append((-2, "data"))

    def run_task(task):
        if task[1] == "modparser":
            luajit(ORACLE / "modparser.lua", [str(out / "modparser.jsonl"), *build_args(builds)])
        elif task[1] == "data":
            luajit(ORACLE / "data.lua", [str(out / "data.jsonl")])
        else:
            run(task)

    parallel(run_task, tasks)
    written = sorted(f.stem for f in out.glob("*.json"))
    expected = sorted(b["id"] for b in builds)
    if written != expected:
        sys.exit(f"dumper wrote {written}, expected {expected}")


def compare(a, b, pattern="*.json*"):
    """Returns the files that differ between two output directories."""
    names = sorted({f.name for f in a.glob(pattern)} | {f.name for f in b.glob(pattern)})
    return [n for n in names
            if not (a / n).exists() or not (b / n).exists() or not filecmp.cmp(a / n, b / n, shallow=False)]


def cmd_generate(args):
    gen = load_corpus()["generated"]
    GENERATED.mkdir(parents=True, exist_ok=True)
    for f in GENERATED.glob("gen-*.xml"):
        f.unlink()
    first, count = gen["first_seed"], gen["count"]
    # generate.lua takes a contiguous range of seeds
    per_job = -(-count // args.jobs)
    ranges = [(seed, min(per_job, first + count - seed)) for seed in range(first, first + count, per_job)]
    parallel(lambda r: luajit(CORPUS / "generate.lua", [str(GENERATED), str(r[0]), str(r[1])]), ranges)
    print(f"generated {count} builds in corpus/generated")


def cmd_dump(args):
    dump(Path(args.out), args.jobs, modparser=True)


def cmd_check(args):
    # Separate processes get different string hash seeds and so different
    # table iteration orders, and each run splits the corpus differently;
    # both must reproduce the committed outputs.
    failed = False
    with tempfile.TemporaryDirectory() as tmp:
        for run, jobs in (("1", args.jobs), ("2", max(1, args.jobs - 1))):
            out = Path(tmp) / run
            dump(out, jobs, modparser=True)
            diff = compare(OUTPUTS, out)
            if diff:
                failed = True
                print(f"run {run}: outputs differ from tests/oracle: {', '.join(diff)}")
    if failed:
        sys.exit("oracle outputs are not reproducible; run `oracle/run.py dump` "
                 "only if the submodule, corpus or dumper changed")
    print("oracle outputs match tests/oracle")


def read_stats(path):
    """Reads a luacov stats file: {file: [hits per line]}."""
    stats = {}
    lines = path.read_text().splitlines()
    for header, counts in zip(lines[0::2], lines[1::2]):
        max_line, name = header.split(":", 1)
        hits = [int(c) for c in counts.split()]
        assert len(hits) == int(max_line), f"bad stats for {name} in {path}"
        stats[name] = hits
    return stats


def write_stats(path, stats):
    with open(path, "w") as f:
        for name in sorted(stats):
            f.write(f"{len(stats[name])}:{name}\n{' '.join(map(str, stats[name]))} \n")


def cmd_coverage(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    for f in out.glob("luacov.*"):
        f.unlink()

    def shard_env(index):
        return {"COVERAGE_DIR": str(out / f"shard-{index}"), "LUACOV_CONFIG": str(ORACLE / "luacov.lua")}

    shard_dirs = []
    for index in range(args.jobs):
        d = out / f"shard-{index}"
        d.mkdir(exist_ok=True)
        (d / "luacov.stats.out").unlink(missing_ok=True)
        shard_dirs.append(d)
    dump(out / "outputs", args.jobs, ["-lluacov"], shard_env)
    # Coverage hooks keep LuaJIT mostly interpreting; results must not change
    diff = compare(OUTPUTS, out / "outputs", "*.json")
    if diff:
        sys.exit(f"outputs under coverage differ from tests/oracle: {', '.join(diff)}")

    merged = {}
    for d in shard_dirs:
        if not (d / "luacov.stats.out").exists():
            continue
        for name, hits in read_stats(d / "luacov.stats.out").items():
            total = merged.setdefault(name, [0] * len(hits))
            if len(total) < len(hits):
                total.extend([0] * (len(hits) - len(total)))
            for i, h in enumerate(hits):
                total[i] += h
    write_stats(out / "luacov.stats.out", merged)

    subprocess.run([str(LUAJIT), "-e", 'require("luacov.runner").run_report()'], cwd=UPSTREAM_SRC, check=True,
                   env={**os.environ, "COVERAGE_DIR": str(out), "LUACOV_CONFIG": str(ORACLE / "luacov.lua"),
                        "LUA_PATH": f"{ROCKS}/share/lua/5.1/?.lua;{ROCKS}/share/lua/5.1/?/init.lua;;",
                        "LUA_CPATH": f"{ROCKS}/lib/lua/5.1/?.so;;"})
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


def cmd_data_json(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    luajit(ORACLE / "data.lua", [str(out / "data.jsonl"), str(out)])
    print(f"wrote {out}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--jobs", type=int, default=JOBS)
    sub = parser.add_subparsers(required=True)
    p = sub.add_parser("generate")
    p.set_defaults(func=cmd_generate)
    p = sub.add_parser("dump")
    p.add_argument("--out", default=str(OUTPUTS))
    p.set_defaults(func=cmd_dump)
    p = sub.add_parser("check")
    p.set_defaults(func=cmd_check)
    p = sub.add_parser("coverage")
    p.add_argument("--out", default=str(ROOT / "target" / "oracle-coverage"))
    p.set_defaults(func=cmd_coverage)
    p = sub.add_parser("data-json")
    p.add_argument("--out", default=str(ROOT / "target" / "oracle-data"))
    p.set_defaults(func=cmd_data_json)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
