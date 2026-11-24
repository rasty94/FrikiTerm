#!/bin/sh
# Levanta o derriba el OpenSSH de pruebas de docker/sshd-test.
#   eval "$(scripts/test-sshd.sh up)"   exporta las FRIKI_TEST_* y deja el contenedor corriendo
#   scripts/test-sshd.sh down           para el contenedor y borra claves, contraseña y known_hosts
# Solo stdout lleva las lineas export; los mensajes van a stderr.
# Estado: la ruta del directorio de secretos (mktemp -d, en $XDG_RUNTIME_DIR o $TMPDIR) se guarda
# en $HOME/.cache/frikissh-test/current (directorio 0700). Antes de borrar se comprueba que sea un
# directorio propio (no enlace) llamado friki-sshd-test.* directamente bajo esa base.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=friki/sshd-test:local
NAME=friki-sshd-test
TMP_BASE=${TMPDIR:-/tmp}
TMP_BASE=${TMP_BASE%/}
CACHE="$HOME/.cache/frikissh-test"
POINTER="$CACHE/current"

log() { printf '%s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }

# Base donde mktemp crea los secretos: XDG_RUNTIME_DIR si existe, si no el temporal del sistema.
secrets_base() {
    if [ -n "${XDG_RUNTIME_DIR:-}" ] && [ -d "$XDG_RUNTIME_DIR" ]; then
        printf '%s\n' "${XDG_RUNTIME_DIR%/}"
    else
        printf '%s\n' "$TMP_BASE"
    fi
}

# Devuelve 0 solo si $1 es un directorio real (no enlace) llamado friki-sshd-test.* directamente
# bajo una de las bases posibles.
owned_dir() {
    d=$1
    [ -n "$d" ] && [ -d "$d" ] && [ ! -L "$d" ] || return 1
    for base in "${XDG_RUNTIME_DIR:-}" "$TMP_BASE"; do
        [ -n "$base" ] && [ "${d%/*}" = "$base" ] || continue
        case "${d##*/}" in friki-sshd-test.*) return 0 ;; esac
    done
    return 1
}

prepare_cache() {
    [ ! -L "$CACHE" ] || die "$CACHE es un enlace simbólico"
    mkdir -p "$CACHE"
    chmod 700 "$CACHE"
}

down() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    if [ -f "$POINTER" ] && [ ! -L "$POINTER" ]; then
        dir=$(cat "$POINTER")
        if owned_dir "$dir"; then
            rm -rf "$dir"
        else
            log "aviso: ruta de estado no válida, no se borra: $dir"
        fi
    fi
    rm -f "$POINTER"
}

up() {
    command -v docker >/dev/null 2>&1 || die "hace falta docker"
    prepare_cache
    down
    dir=$(mktemp -d "$(secrets_base)/friki-sshd-test.XXXXXX")
    printf '%s\n' "$dir" > "$POINTER"

    log "construyendo imagen $IMAGE"
    docker build -q -t "$IMAGE" "$ROOT/docker/sshd-test" >/dev/null

    ssh-keygen -q -t ed25519 -N '' -C friki-test -f "$dir/id_ed25519"
    pass=$(openssl rand -hex 16)

    log "arrancando $NAME"
    docker run -d --init --name "$NAME" \
        -p 127.0.0.1:2222:2222 -p 127.0.0.1:2223:2223 \
        "$IMAGE" >/dev/null

    docker exec -i "$NAME" sh -c \
        'cat > /home/friki/.ssh/authorized_keys && chown friki:friki /home/friki/.ssh/authorized_keys && chmod 600 /home/friki/.ssh/authorized_keys' \
        < "$dir/id_ed25519.pub"
    printf 'friki:%s\n' "$pass" | docker exec -i "$NAME" chpasswd

    log "esperando a sshd en 2222 y 2223"
    i=0
    until ssh-keyscan -T 2 -t ed25519 -p 2222 127.0.0.1 > "$dir/known_hosts.tmp" 2>/dev/null \
        && [ -s "$dir/known_hosts.tmp" ]; do
        i=$((i + 1))
        [ "$i" -lt 60 ] || { docker logs "$NAME" >&2; die "sshd no responde en 2222"; }
        sleep 1
    done
    i=0
    until ssh-keyscan -T 2 -t rsa -p 2223 127.0.0.1 2>/dev/null | grep -q .; do
        i=$((i + 1))
        [ "$i" -lt 60 ] || { docker logs "$NAME" >&2; die "sshd no responde en 2223"; }
        sleep 1
    done
    mv "$dir/known_hosts.tmp" "$dir/known_hosts"

    printf 'export FRIKI_TEST_HOST=127.0.0.1\n'
    printf 'export FRIKI_TEST_PORT=2222\n'
    printf 'export FRIKI_TEST_LEGACY_PORT=2223\n'
    printf 'export FRIKI_TEST_USER=friki\n'
    printf 'export FRIKI_TEST_PASSWORD=%s\n' "$pass"
    printf "export FRIKI_TEST_KEY='%s'\n" "$dir/id_ed25519"
    printf "export FRIKI_TEST_KNOWN_HOSTS='%s'\n" "$dir/known_hosts"
    log "listo: contenedor $NAME, secretos en $dir"
}

case "${1:-}" in
    up)
        trap '[ "$?" -eq 0 ] || down' EXIT
        up
        ;;
    down)
        down
        log "contenedor y secretos eliminados"
        ;;
    *)
        log "uso: $0 up|down"
        exit 2
        ;;
esac
