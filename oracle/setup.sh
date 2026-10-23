#!/usr/bin/env bash
# Builds the oracle's Lua runtime into $ORACLE_PREFIX (default: .oracle/):
# LuaJIT at the commit upstream's test image uses, plus the rocks upstream needs.
# Needs git, make, a C compiler and luarocks.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
prefix="${ORACLE_PREFIX:-$root/.oracle}"
upstream="$root/upstream/PathOfBuilding"

# Same LuaJIT commit as upstream's Dockerfile, so the oracle runs what
# upstream's own CI runs
luajit_commit="$(sed -n 's/.*git checkout \([0-9a-f]\{40\}\).*/\1/p' "$upstream/Dockerfile")"
if [ -z "$luajit_commit" ]; then
	echo "could not find the LuaJIT commit in $upstream/Dockerfile" >&2
	exit 1
fi

if [ "$(cat "$prefix/luajit-commit" 2>/dev/null)" != "$luajit_commit" ]; then
	rm -rf "$prefix"
	mkdir -p "$prefix/src"
	git clone --quiet https://github.com/LuaJIT/LuaJIT "$prefix/src/LuaJIT"
	git -C "$prefix/src/LuaJIT" checkout --quiet "$luajit_commit"
	make -C "$prefix/src/LuaJIT" -j"$(nproc)" PREFIX="$prefix" >/dev/null
	make -C "$prefix/src/LuaJIT" install PREFIX="$prefix" >/dev/null
	ln -sf "$(ls "$prefix"/bin/luajit-* | head -n 1)" "$prefix/bin/luajit"
	echo "$luajit_commit" > "$prefix/luajit-commit"
fi

# luautf8 as in upstream's Dockerfile; luacov for coverage, plus
# luafilesystem for its includeuntestedfiles option. The pure Lua luacov hook
# is used: cluacov is built against PUC Lua internals and crashes under LuaJIT.
rock() {
	if ! luarocks --lua-version=5.1 --tree="$prefix/rocks" show "$1" 2>/dev/null | grep -q "^$1 $2"; then
		luarocks --lua-version=5.1 --tree="$prefix/rocks" install "$1" "$2" \
			LUA_INCDIR="$prefix/include/luajit-2.1" >/dev/null
	fi
}
rock luautf8 0.1.6-1
rock luacov 0.17.0-1
rock luafilesystem 1.9.0-1

"$prefix/bin/luajit" -v
