#!/bin/bash
# Cuts a Hangar release from this Mac: bump, check, build, sign, publish.
#
#   npm run release -- 0.8.10 "What changed"
#   npm run release -- 0.8.10 --dry-run     # everything up to the build, nothing published
#
# Local rather than on GitHub Actions because Actions minutes on a private repo
# are metered, macOS ones count x10, and a four-platform release cost ~130 of the
# 2,000 free minutes a month for platforms nobody was updating from. This builds
# the one platform that is actually used (Apple Silicon) for free.
#
# Publishes to the channel the app reads: the artifact to Supabase Storage and a
# row to `app_releases` — the same two calls as `publish-to-supabase` in
# `.github/workflows/release.yml`, which stays around, manual only, for the day
# another platform is needed.
#
# Which Supabase project to publish to comes from .env.local (VITE_SUPABASE_URL,
# VITE_SUPABASE_PUBLISHABLE_KEY — public values, see .env.example).
# Secrets come from the macOS Keychain only, never the repo or an .env file:
#   security add-generic-password -a hangar -s hangar-supabase-service-role -w <key>
#   security add-generic-password -a hangar -s hangar-signing-password -w <pw>   # only if the key has one
set -euo pipefail

usage() {
  echo "usage: npm run release -- <version> [\"notes\"] [--dry-run]" >&2
  exit 2
}

VERSION=""
NOTES=""
DRY_RUN=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    -*) usage ;;
    *) if [ -z "$VERSION" ]; then VERSION="$arg"; elif [ -z "$NOTES" ]; then NOTES="$arg"; else usage; fi ;;
  esac
done
[ -n "$VERSION" ] || usage
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3, got '$VERSION'" >&2; exit 2; }
NOTES="${NOTES:-Hangar $VERSION}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

TARGET="darwin-aarch64"
KEY="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/hangar.key}"
VERSION_FILES=(package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json)

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

# ── Preflight: nothing is touched until all of this holds ────────────────────
step "Preflight"
[ "$(uname -s)-$(uname -m)" = "Darwin-arm64" ] || die "this builds $TARGET, so it has to run on an Apple Silicon Mac"
[ -f "$KEY" ] || die "no signing key at $KEY — unsigned, every installed copy would refuse the update"
SERVICE_KEY="$(security find-generic-password -s hangar-supabase-service-role -w 2>/dev/null)" \
  || die "no Supabase service key in the Keychain (service 'hangar-supabase-service-role') — see the header of this script"
SIGNING_PASSWORD="$(security find-generic-password -s hangar-signing-password -w 2>/dev/null || true)"

# The Supabase project this build signs in against is the one to publish to,
# and the one its updater asks. It isn't in the repository (see .env.example):
# it comes from the environment or from .env.local, the same file Vite reads.
env_value() {
  [ -f .env.local ] && sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" .env.local | tail -1
}
SUPABASE_URL="${VITE_SUPABASE_URL:-$(env_value VITE_SUPABASE_URL)}"
SUPABASE_URL="${SUPABASE_URL%/}"
[[ "$SUPABASE_URL" == https://*.supabase.co ]] || die "no Supabase project URL — set VITE_SUPABASE_URL in .env.local (got '$SUPABASE_URL')"
[ -n "${VITE_SUPABASE_PUBLISHABLE_KEY:-$(env_value VITE_SUPABASE_PUBLISHABLE_KEY)}" ] || die "no Supabase publishable key — set VITE_SUPABASE_PUBLISHABLE_KEY in .env.local"
# tauri.conf.json ships with no update endpoint, so a fork's build never asks
# this project for updates; the release build gets it here.
UPDATER_CONFIG="{\"plugins\":{\"updater\":{\"endpoints\":[\"$SUPABASE_URL/functions/v1/updates?target={{target}}&arch={{arch}}&current_version={{current_version}}\"]}}}"

[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || die "releases are cut from main"
[ -z "$(git status --porcelain)" ] || die "the working tree has changes — commit or stash them first"
git pull --ff-only --quiet || die "main can't fast-forward to origin/main"
git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null && die "tag v$VERSION already exists"

CURRENT="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
python3 - "$CURRENT" "$VERSION" <<'PY' || die "v$VERSION is not newer than the current v$CURRENT"
import sys
current, new = (tuple(int(n) for n in v.split(".")) for v in sys.argv[1:3])
sys.exit(0 if new > current else 1)
PY
echo "v$CURRENT → v$VERSION ($TARGET)$([ "$DRY_RUN" = 1 ] && echo ', dry run')"

# From here on the version files are edited; put them back on any failure, and
# after a dry run, so a stopped release never leaves a half-bumped tree.
COMMITTED=0
restore() {
  if [ "$COMMITTED" = 0 ]; then
    git checkout --quiet -- "${VERSION_FILES[@]}" 2>/dev/null || true
  fi
}
trap restore EXIT

# ── Bump ────────────────────────────────────────────────────────────────────
step "Bumping the version"
python3 - "$CURRENT" "$VERSION" <<'PY'
import re, sys
old, new = sys.argv[1], sys.argv[2]

def sub(path, pattern, repl, count=1):
    text = open(path).read()
    text, n = re.subn(pattern, repl, text, count=count)
    if n != count:
        sys.exit(f"{path}: expected {count} version line(s), found {n}")
    open(path, "w").write(text)

q = re.escape(old)
sub("package.json", rf'"version": "{q}"', f'"version": "{new}"')
# The top two entries only: the package itself and its "" root. Dependencies
# share version numbers and must not be touched.
sub("package-lock.json", rf'"version": "{q}"', f'"version": "{new}"', count=2)
sub("src-tauri/Cargo.toml", rf'(?m)^version = "{q}"', f'version = "{new}"')
sub("src-tauri/Cargo.lock", rf'(name = "hangar"\nversion = )"{q}"', rf'\g<1>"{new}"')
sub("src-tauri/tauri.conf.json", rf'"version": "{q}"', f'"version": "{new}"')
PY

# ── Checks: the part of CI worth keeping, run where it's free ───────────────
step "Rust tests"
(cd src-tauri && cargo test --quiet)
step "TypeScript"
npx tsc --noEmit
step "Frontend tests"
npx vitest run

# ── Build and sign ──────────────────────────────────────────────────────────
step "Building and signing"
BUILD_STARTED="$(date +%s)"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$SIGNING_PASSWORD" \
  npm run tauri build -- --bundles app --config "$UPDATER_CONFIG"

BUNDLE="src-tauri/target/release/bundle/macos"
ARTIFACT="$BUNDLE/Hangar.app.tar.gz"
[ -f "$ARTIFACT" ] && [ -f "$ARTIFACT.sig" ] || die "the build produced no signed updater artifact at $ARTIFACT"
# A leftover from an earlier build would publish the wrong version silently.
[ "$(stat -f %m "$ARTIFACT")" -ge "$BUILD_STARTED" ] || die "$ARTIFACT predates this build"
SIGNATURE="$(cat "$ARTIFACT.sig")"
OBJECT_PATH="$VERSION/$TARGET/$(basename "$ARTIFACT")"
echo "built $(du -h "$ARTIFACT" | cut -f1) → $OBJECT_PATH"

if [ "$DRY_RUN" = 1 ]; then
  step "Dry run: stopping before publishing"
  echo "would upload $ARTIFACT to $SUPABASE_URL/storage/v1/object/updates/$OBJECT_PATH"
  echo "would register v$VERSION ($TARGET) in app_releases, then commit, tag v$VERSION and push"
  exit 0
fi

# ── Publish ─────────────────────────────────────────────────────────────────
step "Uploading to Supabase Storage"
curl --fail-with-body -sS -X POST \
  "$SUPABASE_URL/storage/v1/object/updates/$OBJECT_PATH" \
  -H "Authorization: Bearer $SERVICE_KEY" \
  -H "x-upsert: true" \
  -H "Content-Type: application/octet-stream" \
  --data-binary "@$ARTIFACT" > /dev/null

step "Registering the release"
PAYLOAD="$(python3 -c '
import json, sys
print(json.dumps({
    "version": sys.argv[1],
    "target": sys.argv[2],
    "object_path": sys.argv[3],
    "signature": sys.argv[4],
    "notes": sys.argv[5],
    "channel": "beta",
    "published": True,
}))' "$VERSION" "$TARGET" "$OBJECT_PATH" "$SIGNATURE" "$NOTES")"
curl --fail-with-body -sS -X POST \
  "$SUPABASE_URL/rest/v1/app_releases?on_conflict=version,target,channel" \
  -H "apikey: $SERVICE_KEY" \
  -H "Authorization: Bearer $SERVICE_KEY" \
  -H "Content-Type: application/json" \
  -H "Prefer: resolution=merge-duplicates" \
  -d "$PAYLOAD" > /dev/null

step "Verifying"
ROWS="$(curl --fail-with-body -sS \
  "$SUPABASE_URL/rest/v1/app_releases?version=eq.$VERSION&target=eq.$TARGET&select=object_path" \
  -H "apikey: $SERVICE_KEY" -H "Authorization: Bearer $SERVICE_KEY")"
[[ "$ROWS" == *"$OBJECT_PATH"* ]] || die "app_releases has no row for v$VERSION $TARGET after the upsert: $ROWS"
curl --fail-with-body -sS -o /dev/null \
  "$SUPABASE_URL/storage/v1/object/info/updates/$OBJECT_PATH" \
  -H "Authorization: Bearer $SERVICE_KEY" || die "the uploaded object isn't in Storage"
echo "v$VERSION is live for $TARGET"

# ── Record it in git ────────────────────────────────────────────────────────
step "Committing, tagging and pushing"
git add "${VERSION_FILES[@]}"
git commit --quiet -m "release: bump to $VERSION"
COMMITTED=1
git tag "v$VERSION"
# The Release workflow is manual-only, so this push runs no Actions.
git push --quiet origin main "v$VERSION"

step "Done — v$VERSION released"
