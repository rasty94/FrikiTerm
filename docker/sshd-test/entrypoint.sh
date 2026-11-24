#!/bin/sh
# Genera las claves de host en cada arranque (nunca se hornean en la imagen) y lanza los dos sshd.
set -eu
keys=/etc/friki-sshd/keys
[ -f "$keys/modern_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -f "$keys/modern_ed25519"
[ -f "$keys/legacy_rsa" ] || ssh-keygen -q -t rsa -b 2048 -N '' -f "$keys/legacy_rsa"

[ -f "$keys/jump_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -f "$keys/jump_ed25519"

/usr/sbin/sshd -D -e -f /etc/friki-sshd/modern.conf &
/usr/sbin/sshd -D -e -f /etc/friki-sshd/jump.conf &
exec /usr/sbin/sshd -D -e -f /etc/friki-sshd/legacy.conf
