#!/usr/bin/env bash
# Build and install "EffectCraft AEP.app": this checkout as a local app that opens After Effects
# projects with no setup, through its own Python venv with py-aep (Contents/Resources/aep-python).
# It sits next to the official EffectCraft without colliding with it: own bundle id, own settings
# folder (~/Library/Application Support/EffectCraft AEP), .ecproj files left to the official app.
#
# Usage: packaging/macos/aep-preview-app.sh [--skip-build] [DESTINATION]   (default: /Applications)
#   PYTHON   interpreter the venv is made from, 3.11 or newer (default: python3); it must stay
#            installed, the venv runs on it. Installing py-aep needs network access once.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

SKIP_BUILD=0
DEST=/Applications
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,9p' "$0"; exit 0 ;;
    -*) echo "unknown argument: $1" >&2; exit 2 ;;
    *) DEST="$1"; shift ;;
  esac
done
APP="$DEST/EffectCraft AEP.app"
PYTHON="${PYTHON:-python3}"
CONFIG="$HOME/Library/Application Support/EffectCraft AEP"
# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0

"$PYTHON" -c 'import sys; sys.exit(sys.version_info < (3, 11))' || { echo "error: $PYTHON is older than 3.11" >&2; exit 1; }
if pgrep -f "$APP/Contents/MacOS/" >/dev/null; then
  echo "error: EffectCraft AEP is running; quit it first" >&2
  exit 1
fi

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p effectcraft)
fi

# Assembled beside the destination, then swapped in: a failure leaves the installed app as it was.
STAGE="$(mktemp -d "$DEST/.effectcraft-aep.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
NEW="$STAGE/EffectCraft AEP.app"
RES="$NEW/Contents/Resources"
echo "==> assembling EffectCraft AEP $VERSION"
mkdir -p "$NEW/Contents/MacOS" "$RES"
cp "$CARGO_TARGET_DIR/release/effectcraft" "$NEW/Contents/MacOS/EffectCraft"
cp "$ROOT/assets/app-icon/effectcraft.icns" "$RES/EffectCraft.icns"
cp "$ROOT/packaging/aep/aep_ir_export.py" "$RES/aep_ir_export.py"
cp "$ROOT/packaging/aep/requirements.txt" "$RES/aep-requirements.txt"
printf 'APPL????' >"$NEW/Contents/PkgInfo"

echo "==> Python venv with py-aep"
"$PYTHON" -m venv --copies "$RES/aep-python"
"$RES/aep-python/bin/python3" -m pip install --quiet --disable-pip-version-check -r "$ROOT/packaging/aep/requirements.txt"
# Only the interpreter is used; activate scripts and console entry points name the staging path.
find "$RES/aep-python/bin" -type f ! -name 'python*' -delete

PLIST="$NEW/Contents/Info.plist"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/${VERSION%%-*}/g" \
  -e "s/@BUILD_SHA@/${EFFECTCRAFT_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$PLIST"
plutil -replace CFBundleIdentifier -string "ai.storyteller.effectcraft.aep-preview" "$PLIST"
plutil -replace CFBundleName -string "EffectCraft AEP" "$PLIST"
plutil -replace CFBundleDisplayName -string "EffectCraft AEP" "$PLIST"
plutil -replace CFBundleDocumentTypes.0.LSHandlerRank -string "Alternate" "$PLIST"
# Listed in Finder's "Open With" for .aep (by extension: After Effects' own UTI wins when installed).
plutil -insert CFBundleDocumentTypes.1 -json \
  '{"CFBundleTypeName":"After Effects Project","CFBundleTypeRole":"Viewer","LSHandlerRank":"Alternate","CFBundleTypeExtensions":["aep"]}' "$PLIST"
plutil -remove UTExportedTypeDeclarations "$PLIST"
# The venv is sealed by the signature: Python must not write bytecode caches into it.
plutil -replace LSEnvironment -json "{\"EFFECTCRAFT_CONFIG_DIR\":\"$CONFIG\",\"PYTHONDONTWRITEBYTECODE\":\"1\"}" "$PLIST"
plutil -lint "$PLIST"

# Ad-hoc signature: enough for this Mac, not for distribution (see package.sh).
codesign --force --sign - --timestamp=none --options runtime --entitlements "$HERE/entitlements.plist" "$NEW/Contents/MacOS/EffectCraft"
codesign --force --sign - --timestamp=none --options runtime --entitlements "$HERE/entitlements.plist" "$NEW"
codesign --verify --strict --deep "$NEW"

mkdir -p "$CONFIG"
rm -rf "$APP"
mv "$NEW" "$APP"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP"
echo "==> installed $APP ($VERSION, ${EFFECTCRAFT_BUILD_SHA:0:9})"
