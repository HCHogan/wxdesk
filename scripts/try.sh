#!/bin/sh
# Build wxdesk on tank and run it in the VM that runs now, without booting
# it again: the guest shares tank's /nix/store. $1 is a script, run in the
# guest as root, that starts it from the store path in $WXDESK.
S=$(cd "$(dirname "$0")" && pwd)
cd "$S/.." || exit 1
B64=$(tar cz --exclude .git --exclude target --exclude result . | base64 | tr -d '\n')
START=$(base64 < "$1" | tr -d '\n')
$S/tank.sh "rm -rf /var/lib/wxprobe/wxdesk-src && mkdir -p /var/lib/wxprobe/wxdesk-src && cd /var/lib/wxprobe/wxdesk-src && echo $B64 | base64 -d | tar xz && cd /var/lib/wxprobe && nix build path:/var/lib/wxprobe/wxdesk-src#wxdesk --option substituters 'https://mirror.sjtu.edu.cn/nix-channels/store https://cache.nixos.org' -o /var/lib/wxprobe/wxdesk -L > wxdesk-build.log 2>&1 || { grep -E '^error' -A12 wxdesk-build.log | head -40; exit 1; }; P=\$(readlink /var/lib/wxprobe/wxdesk); echo \$P; /var/lib/wxprobe/g \"echo $START | base64 -d > /root/start-wxdesk.sh && WXDESK=\$P sh /root/start-wxdesk.sh\""
