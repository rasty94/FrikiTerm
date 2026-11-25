# Telnet sin autenticación SOLO para pruebas locales (se publica en 127.0.0.1).
FROM alpine:3.20@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc
RUN apk add --no-cache busybox-extras
EXPOSE 23
CMD ["telnetd", "-F", "-l", "/bin/sh"]
