# Telnet sin autenticación SOLO para pruebas locales (se publica en 127.0.0.1).
FROM alpine:3.20
RUN apk add --no-cache busybox-extras
EXPOSE 23
CMD ["telnetd", "-F", "-l", "/bin/sh"]
