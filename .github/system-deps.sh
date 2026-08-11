#!/usr/bin/env bash
# The Linux client links GTK3, and no GitHub runner image ships its
# development package. Every Linux job needs the same four, so they are named
# once.
#
# Retried because a mirror that times out is the single most common way a run
# fails for a reason that has nothing to do with the change under test, and a
# red tick nobody believes is worse than a slow one.
set -euo pipefail

packages=(
  libgtk-3-dev
  libxdo-dev
  libayatana-appindicator3-dev
  librsvg2-dev
)

# A self-hosted runner installs these once and keeps them. Re-running apt on
# every job is slow at best, and on a host carrying an unrelated broken
# repository it fails `apt-get update` for a reason that has nothing to do with
# the change under test.
#
# Asked of dpkg rather than pkg-config, because the same list above is then the
# only list: two of these four ship no pkg-config module, so asking pkg-config
# reported them missing on a machine where they were installed.
present=1
for package in "${packages[@]}"; do
  dpkg -s "${package}" >/dev/null 2>&1 || present=0
done
if [ "${present}" -eq 1 ]; then
  echo "system dependencies already present, nothing to install"
  exit 0
fi

for attempt in 1 2 3; do
  if sudo apt-get update && sudo apt-get install -y "${packages[@]}"; then
    exit 0
  fi
  echo "apt failed on attempt ${attempt}" >&2
  # Long enough for a mirror to come back, short next to a cold build.
  sleep $((attempt * 15))
done

echo "the system dependencies could not be installed after three attempts" >&2
exit 1
