# OpenSSH de pruebas para frikiSSH. Un contenedor, dos sshd:
#   2222: configuración moderna (solo algoritmos seguros, Ed25519, contraseña y clave pública).
#   2223: solo algoritmos heredados que OpenSSH aún admite. El cliente seguro debe negarse a negociarlos.
# La base va fijada por tag y por digest.
FROM debian:bookworm-20260202-slim@sha256:98f4b71de414932439ac6ac690d7060df1f27161073c5036a7553723881bffbe

RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --shell /bin/sh friki \
 && install -d -m 700 -o friki -g friki /home/friki/.ssh \
 && install -d -m 755 /run/sshd /etc/friki-sshd/keys

COPY sshd_config.modern /etc/friki-sshd/modern.conf
COPY sshd_config.legacy /etc/friki-sshd/legacy.conf
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod 755 /usr/local/bin/entrypoint.sh

EXPOSE 2222 2223
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
