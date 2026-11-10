#!/usr/bin/env sh
# Regenerates assets-spine43/ from assets/: identical files, except skeleton.spine in each JSON is rewritten to "4.3.0".
# spine2d only accepts Spine 4.3 version strings (the check is a plain prefix match); the JSON content is otherwise
# unchanged. This is the one documented deviation from "identical assets for all backends".
set -e
cd "$(dirname "$0")/.."
rm -rf assets-spine43 && mkdir -p assets-spine43 && cp -r assets/rigs assets-spine43/
find assets-spine43 -name '*.json' -exec sed -i -E 's/"spine":[[:space:]]*"[^"]*"/"spine": "4.3.0"/' {} +
grep -rho '"spine": *"[^"]*"' assets-spine43
