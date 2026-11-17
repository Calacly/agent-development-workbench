#!/usr/bin/env bash
# Sandbox test for the modern-web-guidance CLI (GoogleChrome), NOT installed as a skill.
#
#   ./mwg.sh install            fetch the pinned package into a podman volume (network on, once)
#   ./mwg.sh search "<query>"   run the CLI offline (network none, read-only, no caps)
#   ./mwg.sh retrieve "<id>"
#   ./mwg.sh list
#   ./mwg.sh scan               legacy-scan read-only mounts of ../../templates and ../../static
#   ./mwg.sh demo               a few generic searches + one retrieve + scan
#   ./mwg.sh clean              remove the volume
#
# Versions are pinned and at least 3 days old (package cooldown rule).
set -euo pipefail

IMAGE="docker.io/library/node:22.23.5-alpine@sha256:1314fb30ec37592d5845e5a160d522c0e680972aa534549940b8d73bde88481a"
PKG="modern-web-guidance"
PKG_VERSION="1.0.0" # published 2026-11-10; 1.0.1 (2026-11-16) is too new
VOLUME="mwg-sandbox"
HERE="$(cd "$(dirname "$0")" && pwd)"
# The web UI: HTML templates and CSS/JS, both mounted under /src
TEMPLATES="$(cd "$HERE/../../templates" && pwd)"
STATIC="$(cd "$HERE/../../static" && pwd)"
CLI="/opt/mwg/node_modules/$PKG/skills/$PKG/modern-web.mjs"

run_offline() {
  podman run --rm \
    --network none \
    --read-only --tmpfs /tmp \
    --cap-drop ALL --security-opt no-new-privileges \
    --user node -e HOME=/tmp \
    -e DISABLE_TELEMETRY=1 \
    -v "$VOLUME":/opt/mwg:ro \
    -v "$TEMPLATES":/src/templates:ro \
    -v "$STATIC":/src/static:ro \
    "$IMAGE" "$@"
}

cmd="${1:-help}"
shift || true

case "$cmd" in
  install)
    podman volume exists "$VOLUME" || podman volume create "$VOLUME" >/dev/null
    podman run --rm \
      --cap-drop ALL --security-opt no-new-privileges \
      -e DISABLE_TELEMETRY=1 -e npm_config_update_notifier=false -e npm_config_fund=false \
      -v "$VOLUME":/opt/mwg \
      "$IMAGE" sh -c "cd /opt/mwg && npm init -y >/dev/null && \
        npm install --ignore-scripts --save-exact --no-audit $PKG@$PKG_VERSION && \
        npm ls --all"
    ;;
  search | retrieve | list)
    run_offline node "$CLI" "$cmd" "$@"
    ;;
  scan)
    run_offline node "$CLI" legacy-scan /src --json
    ;;
  demo)
    for q in "light dismiss dialog" "responsive data table" "dark mode toggle" "sanitize untrusted html"; do
      echo "### search: $q"
      run_offline node "$CLI" search "$q"
    done
    echo "### retrieve: light-dismiss-a-dialog"
    run_offline node "$CLI" retrieve "light-dismiss-a-dialog" | head -40
    echo "### legacy-scan templates and static"
    run_offline node "$CLI" legacy-scan /src --json
    ;;
  clean)
    podman volume rm -f "$VOLUME"
    ;;
  *)
    sed -n '2,12p' "$0"
    ;;
esac
