#!/bin/sh
# Run a command on tank through h610; output to a file first, retry on 255.
out=$(mktemp)
for i in 1 2 3; do
  ssh -o ConnectTimeout=15 -o ServerAliveInterval=15 -J root@h610 root@tank "$@" > "$out" 2>&1 < /dev/null
  rc=$?
  [ $rc -ne 255 ] && break
  sleep 3
done
cat "$out"; rm -f "$out"
exit $rc
