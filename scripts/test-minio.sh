#!/bin/sh
# Levanta o derriba el MinIO de pruebas de docker/minio-test (backend S3 de friki-xfer).
#   eval "$(scripts/test-minio.sh up)"   exporta FRIKI_TEST_S3_* y AWS_* y deja el contenedor corriendo
#   scripts/test-minio.sh down           para el contenedor
# Solo stdout lleva las lineas export; los mensajes van a stderr.
# La prueba crea su propio bucket. La contraseña es aleatoria y solo vive en el entorno.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=friki/minio-test:local
NAME=friki-minio-test
PORT=9000
log() { printf '%s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }
down() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
up() {
    command -v docker >/dev/null 2>&1 || die "hace falta docker"
    command -v openssl >/dev/null 2>&1 || die "hace falta openssl"
    down
    user=friki
    pass=$(openssl rand -hex 16)
    log "construyendo imagen $IMAGE (compila MinIO; tarda)"
    docker build -q -t "$IMAGE" "$ROOT/docker/minio-test" >/dev/null
    log "arrancando $NAME"
    docker run -d --init --name "$NAME" \
        -e MINIO_ROOT_USER="$user" -e MINIO_ROOT_PASSWORD="$pass" \
        -p "127.0.0.1:$PORT:9000" \
        "$IMAGE" >/dev/null
    log "esperando a MinIO en $PORT"
    i=0
    until curl -fsS "http://127.0.0.1:$PORT/minio/health/live" >/dev/null 2>&1; do
        i=$((i + 1))
        [ "$i" -lt 60 ] || { docker logs "$NAME" >&2; die "MinIO no responde en $PORT"; }
        sleep 1
    done
    printf 'export FRIKI_TEST_S3_ENDPOINT=http://127.0.0.1:%s\n' "$PORT"
    printf 'export FRIKI_TEST_S3_BUCKET=friki-test\n'
    printf 'export AWS_REGION=us-east-1\n'
    printf 'export AWS_ACCESS_KEY_ID=%s\n' "$user"
    printf 'export AWS_SECRET_ACCESS_KEY=%s\n' "$pass"
    log "listo: contenedor $NAME"
}
case "${1:-}" in
    up)
        trap '[ "$?" -eq 0 ] || down' EXIT
        up
        ;;
    down)
        down
        log "contenedor eliminado"
        ;;
    *)
        log "uso: $0 up|down"
        exit 2
        ;;
esac
