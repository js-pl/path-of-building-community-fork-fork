#!/usr/bin/env bash
# Phase 0 baseline, see bench/README.md.
#
# usage: bench/run.sh [--workers N] [--tasks LIST] [--out DIR] [BUILD...]
#
# BUILD is a PoB XML file or a file holding a share code. Defaults to the
# builds in upstream's spec/TestBuilds.
set -euo pipefail

bench_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$bench_dir")"
upstream="$root/upstream/PathOfBuilding"

workers="$(nproc)"
tasks="node:combined,node:FullDPS,anoint"
out="$root/target/bench"
builds=()
while [[ $# -gt 0 ]]; do
	case "$1" in
		--workers) workers="$2"; shift 2 ;;
		--tasks) tasks="$2"; shift 2 ;;
		--out) out="$2"; shift 2 ;;
		*) builds+=("$(realpath "$1")"); shift ;;
	esac
done
if [[ ${#builds[@]} -eq 0 ]]; then
	while IFS= read -r -d '' f; do builds+=("$f"); done \
		< <(find "$upstream/spec/TestBuilds" -name '*.xml' -print0 | sort -z)
fi
mkdir -p "$out"
out="$(realpath "$out")"

if [[ ! -f "$upstream/src/HeadlessWrapper.lua" ]]; then
	echo "upstream submodule missing: git submodule update --init" >&2
	exit 1
fi
if ! luajit -e 'require("lua-utf8")' 2>/dev/null; then
	echo "needs luajit with the lua-utf8 module (luarocks --lua-version=5.1 install luautf8)" >&2
	exit 1
fi

cd "$upstream/src"
export LUA_PATH="../runtime/lua/?.lua;../runtime/lua/?/init.lua;;"
luajit "$bench_dir/lua/run.lua" --workers "$workers" --tasks "$tasks" --out "$out" "${builds[@]}" \
	| tee "$out/report.md"
