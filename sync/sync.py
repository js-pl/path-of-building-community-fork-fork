#!/usr/bin/env python3
"""Helpers for the sync skill (.claude/skills/sync). Each prints only what
the next step needs; long output goes to files under target/sync/.

  sync/sync.py triage [REF]     upstream's commits since the manifest's
                                commit, each with its files' status and port;
                                data-only commits on one line (REF: origin/dev)
  sync/sync.py show COMMIT      a commit's diff without the files that need
                                no port (data, skip, Export)
  sync/sync.py regen            bump-time regeneration in order (generated
                                corpus, oracle dump, synthetic lines, portgen);
                                prints a line per step and a diff summary
  sync/sync.py record [--except FILE ...]
                                set the manifest's upstream and every
                                ported/partial/data entry's sync to the
                                submodule's HEAD, except the listed files
"""
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SUB = ROOT / "upstream" / "PathOfBuilding"
MANIFEST = ROOT / "sync" / "manifest.toml"
LOGS = ROOT / "target" / "sync"
# upstream directories whose changes are data the importer reads, or export
# tooling: nothing to port by hand
DATA_DIRS = ("src/Data/", "src/TreeData/", "src/Export/")
NO_PORT = {"data", "skip"}


def git(*args, cwd=SUB, check=True):
    r = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and r.returncode != 0:
        sys.exit(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


def ok(*args, cwd=SUB):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True).returncode == 0


def manifest():
    with open(MANIFEST, "rb") as f:
        return tomllib.load(f)


def classify(path, files):
    """(status, port) of an upstream path such as src/Classes/Item.lua"""
    rel = path.removeprefix("src/")
    entry = files.get(rel)
    if entry:
        return entry["status"], entry.get("port") or entry.get("reason", "")
    if path.startswith(DATA_DIRS):
        return "data", ""
    if path.endswith(".lua") and path.startswith("src/"):
        return "new", "not in the manifest: sync/check.py --add, then give it its status"
    return "other", ""


def short(port):
    """A port list cut to its first paths: the manifest has the rest"""
    paths = port.split()
    if len(paths) <= 3 or status_text(port):
        return port
    return " ".join(paths[:3]) + f" (+{len(paths) - 3}, see the manifest)"


def status_text(s):
    return " " in s and not all("/" in w for w in s.split())


def fetch(ref, old):
    remote, _, branch = ref.partition("/")
    if not branch:
        return ref
    depth = 200
    while True:
        git("fetch", "-q", f"--depth={depth}", remote, branch)
        if ok("merge-base", "--is-ancestor", old, "FETCH_HEAD"):
            return git("rev-parse", "FETCH_HEAD").strip()
        if depth > 5000:
            sys.exit(f"{old} is not an ancestor of {ref}")
        depth *= 4


def triage(ref):
    m = manifest()
    old, files = m["upstream"], m["files"]
    new = fetch(ref, old)
    print(f"upstream {old[:9]}..{new[:9]} ({ref})")
    commits = git("log", "--reverse", "--format=%h %s", f"{old}..{new}", "--", "src").splitlines()
    todo, ui, port, added = set(), set(), 0, set()
    for line in commits:
        sha = line.split()[0]
        paths = git("show", "--name-only", "--format=", sha).split()
        rows = [(p, *classify(p, files)) for p in paths if p.startswith("src/")]
        relevant = [r for r in rows if r[1] not in NO_PORT and r[1] != "other"]
        if not relevant:
            print(f"{line}  [data only]")
            continue
        if all(r[1] == "todo" for r in relevant):
            todo.update(r[0] for r in relevant)
            names = ", ".join(r[0].removeprefix("src/") for r in relevant)
            print(f"{line}  [todo only: {names}]")
            continue
        port += 1
        print(line)
        for p, status, where in relevant:
            print(f"    {status:8} {p.removeprefix('src/')}" + (f" -> {short(where)}" if where else ""))
            if status == "todo":
                todo.add(p)
            if status == "new":
                added.add(p)
            if status in ("partial", "ported") and "web/" in where:
                ui.add(p.removeprefix("src/"))
    print(f"\n{len(commits)} commits, {port} to look at (sync/sync.py show <sha> for the diff)")
    if added:
        print(f"new files: {len(added)}")
    if todo:
        print(f"todo files touched (nothing to port, list in the PR): {len(todo)}")
    if ui:
        print("ported in web/, check by hand for new options, labels or controls: " + ", ".join(sorted(ui)))
    print(f"to bump: git -C upstream/PathOfBuilding checkout {new[:9]}")


def show(sha):
    files = manifest()["files"]
    paths = git("show", "--name-only", "--format=", sha).split()
    keep = [p for p in paths if classify(p, files)[0] not in NO_PORT | {"other"}]
    skipped = len(paths) - len(keep)
    print(re.sub(r"\n{3,}", "\n\n", git("show", "--format=%h %s%n%n%b", "--stat=100", sha).strip()))
    if keep:
        print(git("show", "--format=", "-w", sha, "--", *keep))
    if skipped:
        print(f"({skipped} data or skipped files left out)")


def step(name, cmd, log):
    with open(log, "a") as f:
        f.write(f"\n$ {' '.join(cmd)}\n")
        f.flush()
        r = subprocess.run(cmd, cwd=ROOT, stdout=f, stderr=subprocess.STDOUT)
    print(f"{'ok  ' if r.returncode == 0 else 'FAIL'} {name}")
    if r.returncode != 0:
        tail = log.read_text().splitlines()[-20:]
        print("\n".join(tail))
        sys.exit(f"step failed; full log: {log.relative_to(ROOT)}")


def changed(path):
    return git("status", "--porcelain", "--", path, cwd=ROOT).strip() != ""


def regen():
    LOGS.mkdir(parents=True, exist_ok=True)
    log = LOGS / "regen.log"
    log.write_text("")
    old = manifest()["upstream"]
    if not ok("diff", "--quiet", old, "HEAD", "--", "Dockerfile"):
        step("oracle/setup.sh (upstream's Dockerfile changed)", ["oracle/setup.sh"], log)
    # dump into a scratch directory, so that a failing oracle never leaves
    # tests/oracle half written
    scratch = LOGS / "oracle"
    dump = ["oracle/run.py", "dump", "--out", str(scratch)]

    def dump_step(name):
        step(name, dump, log)
        shutil.rmtree(ROOT / "tests" / "oracle")
        shutil.copytree(scratch, ROOT / "tests" / "oracle")

    # generate reads no oracle output; synthlines reads the dump's
    # modparser.jsonl, and the oracle parses its lines in turn
    step("generated corpus", ["oracle/run.py", "generate"], log)
    dump_step("oracle dump")
    step("synthetic mod lines", ["cargo", "run", "-q", "--release", "-p", "importer", "--bin", "synthlines"], log)
    if changed("corpus/synthetic_mod_lines.txt"):
        dump_step("oracle dump (synthetic lines changed)")
    step("portgen", ["cargo", "run", "-q", "--release", "-p", "importer", "--bin", "portgen"], log)
    print("\nregenerated (never read these diffs; the tests compare them):")
    print(git("diff", "--shortstat", "--", "tests/oracle", "corpus", cwd=ROOT).strip() or "  no changes")
    print(git("diff", "--stat=100", "--", "crates/engine/src/mod_parser/generated.rs", cwd=ROOT).strip())
    print(f"log: {log.relative_to(ROOT)}")


def record(keep):
    head = git("rev-parse", "HEAD").strip()
    keep = {k.removeprefix("src/") for k in keep}
    files = manifest()["files"]
    unknown = keep - set(files)
    if unknown:
        sys.exit(f"not in the manifest: {', '.join(sorted(unknown))}")
    lines, n = MANIFEST.read_text().splitlines(keepends=True), 0
    for i, line in enumerate(lines):
        if line.startswith("upstream = "):
            lines[i] = f'upstream = "{head}"\n'
            continue
        name = re.match(r'"([^"]+)" = ', line)
        if name and name[1] not in keep and 'sync = "' in line:
            new = re.sub(r'sync = "[0-9a-f]+"', f'sync = "{head}"', line)
            n += new != line
            lines[i] = new
    MANIFEST.write_text("".join(lines))
    print(f"upstream = {head[:9]}; {n} entries' sync updated, {len(keep)} kept")


def main():
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print(__doc__)
    elif args[0] == "triage":
        triage(args[1] if len(args) > 1 else "origin/dev")
    elif args[0] == "show" and len(args) == 2:
        show(args[1])
    elif args[0] == "regen":
        regen()
    elif args[0] == "record":
        record(args[2:] if args[1:2] == ["--except"] else [])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
