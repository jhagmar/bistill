#!/bin/sh
# GitHub Actions runs the job as a normal user. A root shell can still write
# a mode 0555 directory and read a mode 0000 lock, so the permission tests
# would not match CI. Drop to the uid that owns this tree before ci.sh.
set -eu

uid=$(stat -c '%u' /src)
gid=$(stat -c '%g' /src)
if [ "$uid" -eq 0 ]; then
  uid=1000
  gid=1000
fi

mkdir -p /tmp/ci-home /src/target
chown -R "$uid:$gid" /tmp/ci-home /opt/cargo /src/target

export HOME=/tmp/ci-home
exec setpriv --reuid="$uid" --regid="$gid" --clear-groups --inh-caps=-all /src/ci/ci.sh
