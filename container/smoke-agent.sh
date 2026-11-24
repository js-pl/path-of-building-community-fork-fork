#!/bin/sh
# `pob agent` end to end with Podman: it starts a host, mints a token for
# one build, builds the images and runs a check in the container instead
# of the harness (`--exec`). From inside, the MCP endpoint answers through
# the proxy's route with the token and refuses another; the host's files
# and control socket aren't there. After the run the token is revoked.
#
#   container/smoke-agent.sh target/release/pob
set -eu

pob=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
root=$(cd "$(dirname "$0")/.." && pwd)
data=$(mktemp -d)
export XDG_DATA_HOME="$data" POB_SETTINGS="$data/settings.json"
mkdir -p "$data/path-of-building-fork-fork/builds/RF"
cp "$root/corpus/generated/gen-0003.xml" "$data/path-of-building-fork-fork/builds/RF/build.xml"
cleanup() {
    pkill -f "$pob host --headless" 2>/dev/null || true
    rm -rf "$data"
}
trap cleanup EXIT

check='
fail=0
ok() { echo "ok    $1"; }
no() { echo "FAIL  $1"; fail=1; }
init='"'"'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}'"'"'
post() {
    curl -sS --max-time 20 -X POST "http://pob-proxy:8731/mcp" \
        -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" "$@"
}
headers=$(mktemp)
if post -D "$headers" -H "Authorization: Bearer $POB_TOKEN" -d "$init" | grep -q serverInfo; then
    ok "MCP initialize through the route, with the token"
else no "MCP initialize through the route, with the token"; fi
sid=$(grep -i "^mcp-session-id:" "$headers" | cut -d" " -f2 | tr -d "\r")
post -H "Authorization: Bearer $POB_TOKEN" -H "Mcp-Session-Id: $sid" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}" >/dev/null
if post -H "Authorization: Bearer $POB_TOKEN" -H "Mcp-Session-Id: $sid" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"summary\",\"arguments\":{}}}" \
    | grep -q "Life"; then ok "a tool call on the granted build"; else no "a tool call on the granted build"; fi
code=$(post -o /dev/null -w "%{http_code}" -H "Authorization: Bearer pob_wrong" -d "$init" || true)
[ "$code" = 401 ] && ok "another token is refused" || no "another token is refused ($code)"
if find / \( -path /proc -o -path /sys \) -prune -o \( -name control.sock -o -name build.xml -o -name tokens.json \) -print 2>/dev/null | grep -q .
then no "no host files or control socket"; else ok "no host files or control socket"; fi
! curl -sS -o /dev/null --max-time 15 https://example.com/ && ok "no HTTPS to another host" \
    || no "no HTTPS to another host"
exit $fail
'
"$pob" agent claude --build RF --exec -- sh -c "$check"

left=$("$pob" agents tokens --json)
echo "$left"
case "$left" in
*'"tokens":[]'*) echo "ok    the run's token is revoked" ;;
*) echo "FAIL  the run's token is revoked"; exit 1 ;;
esac
