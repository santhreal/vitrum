#!/usr/bin/env bash
# The Linux client links the system webview, and no GitHub runner image ships
# it. Every Linux job needs the same four packages, so they are named once.
#
# Retried because a mirror that times out is the single most common way a run
# fails for a reason that has nothing to do with the change under test, and a
# red tick nobody believes is worse than a slow one.
set -euo pipefail

packages=(
  libwebkit2gtk-4.1-dev
  libxdo-dev
  libayatana-appindicator3-dev
  librsvg2-dev
)

# A self-hosted runner installs these once and keeps them. Re-running apt on
# every job is slow at best, and on a host carrying an unrelated broken
# repository it fails `apt-get update` for a reason that has nothing to do with
# the change under test.
if pkg-config --exists webkit2gtk-4.1 && pkg-config --exists libxdo; then
  echo "system webview already present, nothing to install"
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

echo "the system webview could not be installed after three attempts" >&2
exit 1
