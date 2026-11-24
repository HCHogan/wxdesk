#!/bin/sh
# Ship the working tree to tank, build the VM there and boot it again.
# The VM keeps the probe's state in /var/lib/wxprobe (its disk holds the
# login) and runs as the transient unit wxprobe-vm.
S=$(cd "$(dirname "$0")" && pwd)
cd "$S/.." || exit 1
B64=$(tar cz --exclude .git --exclude target --exclude result . | base64 | tr -d '\n')
$S/tank.sh "mkdir -p /var/lib/wxprobe/wxdesk-src && cd /var/lib/wxprobe/wxdesk-src && echo $B64 | base64 -d | tar xz && sh scripts/build.sh; cat /var/lib/wxprobe/status; grep -E '^error' -A12 /var/lib/wxprobe/build.log | head -40; if [ \"\$(cat /var/lib/wxprobe/status)\" = ok ]; then systemctl stop wxprobe-vm; systemctl reset-failed wxprobe-vm 2>/dev/null; systemd-run --unit=wxprobe-vm --working-directory=/var/lib/wxprobe --setenv=PATH=/run/current-system/sw/bin /var/lib/wxprobe/result/bin/run-wxprobe-vm; fi"
