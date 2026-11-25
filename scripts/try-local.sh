#!/usr/bin/env bash
# Entorno de prueba manual: levanta el sshd y el telnetd de pruebas (Docker) y prepara un
# FRIKI_HOME aislado con sesiones ya guardadas. No toca tu configuración real.
#
#   eval "$(scripts/try-local.sh up)"    # exporta FRIKI_HOME y FRIKI_TEST_*
#   friki                                # selector de sesiones
#   scripts/try-local.sh down
set -euo pipefail
cd "$(dirname "$0")/.."
HOME_DIR="${TMPDIR:-/tmp}/frikiterm-prueba"

case "${1:-}" in
up)
    eval "$(scripts/test-sshd.sh up)"
    eval "$(scripts/test-telnetd.sh up)"
    rm -rf "$HOME_DIR"
    mkdir -p "$HOME_DIR/sessions"
    chmod 700 "$HOME_DIR"
    cp "$FRIKI_TEST_KNOWN_HOSTS" "$HOME_DIR/known_hosts"
    key=$FRIKI_TEST_KEY
    s() { # nombre, cuerpo TOML
        printf '%s\n' "$2" >"$HOME_DIR/sessions/$1.toml"
    }
    s ssh-moderno "host = \"$FRIKI_TEST_HOST\"
port = $FRIKI_TEST_PORT
user = \"$FRIKI_TEST_USER\"
identity_files = [\"$key\"]"
    s ssh-sin-strict-kex "host = \"$FRIKI_TEST_HOST\"
port = $FRIKI_TEST_NOSTRICT_PORT
user = \"$FRIKI_TEST_USER\"
identity_files = [\"$key\"]"
    s telnet "protocol = \"telnet\"
host = \"127.0.0.1\"
port = $FRIKI_TEST_TELNET_PORT"
    s shell-local 'protocol = "local-shell"'
    echo "export FRIKI_HOME='$HOME_DIR'"
    env | grep '^FRIKI_TEST_' | sed "s/^/export /; s/=\(.*\)/='\1'/"
    ;;
down)
    scripts/test-telnetd.sh down || true
    scripts/test-sshd.sh down
    rm -rf "$HOME_DIR"
    ;;
*)
    echo "uso: $0 up|down" >&2
    exit 250
    ;;
esac
