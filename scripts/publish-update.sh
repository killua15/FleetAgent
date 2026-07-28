#!/bin/bash
# Publishes a new version to the private update channel.
#
# Hangar's repository is private, and GitHub serves nothing from a private repo
# without authentication — including release assets. Worse, a release asset's id
# changes with every release, while the updater needs a URL that doesn't move.
# So the update lives in the repository itself, under `updates/`, and is fetched
# through the Contents API, which does have a stable URL per path.
#
# The cost is honest and worth stating: each published version adds ~9 MB to the
# repository, permanently. That is the price of keeping the code private without
# standing up a second host.
#
#   ./scripts/publish-update.sh 0.1.1 "What changed"
set -euo pipefail

VERSION="${1:?usage: publish-update.sh <version> [notes]}"
NOTES="${2:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEY="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/hangar.key}"

cd "$REPO_ROOT"

if [ ! -f "$KEY" ]; then
  echo "No signing key at $KEY — without it the update would build unsigned and every installed copy would refuse it." >&2
  exit 1
fi

# The version in the manifest and the version in the bundle have to agree: the
# updater compares what it downloads against what it was promised, and installs
# nothing when they differ.
npm version "$VERSION" --no-git-tag-version --allow-same-version >/dev/null
python3 - "$VERSION" <<'PY'
import json, sys
version = sys.argv[1]
path = "src-tauri/tauri.conf.json"
conf = json.load(open(path))
conf["version"] = version
json.dump(conf, open(path, "w"), indent=2)
open(path, "a").write("\n")
PY

echo "Building $VERSION…"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
  npm run tauri build -- --bundles app,dmg

BUNDLE="src-tauri/target/release/bundle/macos"
SIGNATURE="$(cat "$BUNDLE/Hangar.app.tar.gz.sig")"

mkdir -p updates
cp "$BUNDLE/Hangar.app.tar.gz" "updates/Hangar.app.tar.gz"

# `darwin-aarch64` only, because that is what this builds. An Intel Mac asking
# this manifest finds no entry for its platform and is told there is no update,
# which is true — better than handing it a binary it cannot run.
python3 - "$VERSION" "$NOTES" "$SIGNATURE" <<'PY'
import datetime, json, sys
version, notes, signature = sys.argv[1], sys.argv[2], sys.argv[3]
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
    "platforms": {
        "darwin-aarch64": {
            "signature": signature,
            "url": "https://api.github.com/repos/killua15/FleetAgent/contents/updates/Hangar.app.tar.gz",
        }
    },
}
json.dump(manifest, open("updates/latest.json", "w"), indent=2)
open("updates/latest.json", "a").write("\n")
PY

git add package.json package-lock.json src-tauri/tauri.conf.json updates
git commit -m "release: v$VERSION"
git push

echo
echo "Published v$VERSION. Installed copies will see it on their next launch,"
echo "provided the machine has an update token in Settings → general → updates."
