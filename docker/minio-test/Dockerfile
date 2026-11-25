# MinIO de pruebas para el backend S3 (friki-xfer, prueba ignorada minio_ciclo_completo).
# MinIO ya no publica imágenes ni binarios comunitarios: se compila la etiqueta fijada.
FROM golang:1.25-alpine AS build
ARG MINIO_TAG=RELEASE.2025-10-15T17-29-55Z
RUN apk add --no-cache git \
 && git clone --depth 1 --branch "$MINIO_TAG" https://github.com/minio/minio /src \
 && cd /src && CGO_ENABLED=0 go build -trimpath -ldflags "-s -w" -o /out/minio .

FROM alpine:3.22
RUN apk add --no-cache ca-certificates
COPY --from=build /out/minio /usr/local/bin/minio
EXPOSE 9000
ENTRYPOINT ["minio"]
CMD ["server", "/data"]
