#!/bin/sh
# On tank: build the VM; only the SJTU mirror and cache.nixos.org, retried.
cd /var/lib/wxprobe
echo running > status
for i in 1 2 3; do
  if nix build path:/var/lib/wxprobe/wxdesk-src#nixosConfigurations.wxprobe.config.system.build.vm \
      --option substituters "https://mirror.sjtu.edu.cn/nix-channels/store https://cache.nixos.org" \
      -o /var/lib/wxprobe/result -L > build.log 2>&1; then
    echo ok > status; exit 0
  fi
  sleep 5
done
echo fail > status
