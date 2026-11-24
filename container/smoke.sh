#!/bin/sh
# Builds the default agent container's images with Podman and checks its
# defaults from inside (docs/agent.md, "Testing", the default container):
# non-root, no capabilities, a read-only root filesystem, the harnesses
# installed, HTTPS only to the allowlist, no route out but the proxy.
#
#   container/smoke.sh            # from a checkout, with Podman installed
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
tag=smoke-$$
net=pob-smoke-$$
ctx=$(mktemp -d)
cleanup() {
    podman rm -f "pob-proxy-$tag" >/dev/null 2>&1 || true
    podman network rm -f "$net" >/dev/null 2>&1 || true
    podman volume rm -f "pob-smoke-home-$$" "pob-smoke-work-$$" >/dev/null 2>&1 || true
    rm -rf "$ctx"
}
trap cleanup EXIT

# The build context, as `pob agent` writes it
cp "$root/container/Containerfile" "$ctx/"
cp -r "$root/container/allow" "$ctx/allow"
mkdir -p "$ctx/proxy"
cp "$root/crates/proxy/Cargo.toml" "$ctx/proxy/"
cp -r "$root/crates/proxy/src" "$ctx/proxy/src"
podman build -q --target proxy -t "pob-proxy:$tag" "$ctx"
podman build -q --target agent -t "pob-agent:$tag" "$ctx"

# No DNS on the internal network: its resolver has no upstream, and the
# agent needs none (the proxy resolves); the proxy is named with --add-host
podman network create --internal --disable-dns "$net" >/dev/null
podman run -d --name "pob-proxy-$tag" --network "$net" \
    --read-only --cap-drop all --security-opt no-new-privileges \
    --memory 256m --pids-limit 128 \
    "pob-proxy:$tag" --listen 0.0.0.0:3128 --allow /etc/pob-proxy/claude.txt >/dev/null
podman network connect podman "pob-proxy-$tag"
ip=$(podman inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" "pob-proxy-$tag")

fail=0
check() {
    # check <what> <shell test>: runs the test in the agent container
    if podman run --rm --network "$net" --add-host "pob-proxy:$ip" \
        --read-only --tmpfs /tmp:rw,nosuid,nodev,size=512m \
        -v "pob-smoke-home-$$:/home/node" -v "pob-smoke-work-$$:/work" \
        --cap-drop all --security-opt no-new-privileges \
        --memory 4g --cpus 2 --pids-limit 512 \
        "pob-agent:$tag" sh -c "$2" >/dev/null 2>&1; then
        echo "ok    $1"
    else
        echo "FAIL  $1"
        fail=1
    fi
}
check "not root" '[ "$(id -u)" != 0 ]'
check "no capabilities" 'grep -q "^CapEff:[[:space:]]*0000000000000000$" /proc/self/status'
check "the root filesystem is read-only" '! touch /usr/x && touch /tmp/x /work/x /home/node/x'
check "Claude Code is installed" 'claude --version'
check "Codex is installed" 'codex --version'
check "HTTPS to an allowed host" \
    'code=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 20 https://api.anthropic.com/) && [ "$code" != 000 ]'
check "no HTTPS to another host" '! curl -sS -o /dev/null --max-time 20 https://example.com/'
check "no plain HTTP" '! curl -fsS -o /dev/null --max-time 20 http://api.anthropic.com/'
check "no route out without the proxy" \
    '! curl -sS -o /dev/null --noproxy "*" --max-time 10 https://1.1.1.1/'
podman logs "pob-proxy-$tag" 2>&1 | sed 's/^/proxy: /'
exit $fail
