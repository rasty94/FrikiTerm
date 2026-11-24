#!/bin/sh
# Levanta o derriba el telnetd de pruebas de docker/telnetd-test (solo 127.0.0.1).
#   eval "$(scripts/test-telnetd.sh up)"   exporta FRIKI_TEST_TELNET_PORT
#   scripts/test-telnetd.sh down           para y borra el contenedor
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=friki/telnetd-test:local
NAME=friki-telnetd-test
case "${1:-}" in
up)
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker build -q -t "$IMAGE" "$ROOT/docker/telnetd-test" >&2
    docker run -d --rm --name "$NAME" -p 127.0.0.1::23 "$IMAGE" >&2
    port=$(docker port "$NAME" 23/tcp | head -n1 | sed 's/.*://')
    sleep 1
    echo "export FRIKI_TEST_TELNET_PORT=$port"
    ;;
down)
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    ;;
*)
    echo "uso: $0 up|down" >&2
    exit 2
    ;;
esac
