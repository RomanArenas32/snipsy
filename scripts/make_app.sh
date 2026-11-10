#!/bin/bash
# Arma Snipsy.app (con el ícono de Snipi) en dist/ y, con --install, la copia a /Applications.
#   ./scripts/make_app.sh            → dist/Snipsy.app
#   ./scripts/make_app.sh --install  → además /Applications/Snipsy.app
set -euo pipefail
cd "$(dirname "$0")/.."

APP=dist/Snipsy.app
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "→ Compilando (release)…"
cargo build --release -p snipsy

echo "→ Dibujando el ícono…"
SNIPSY_ICON_OUT="$WORK/icon.png" cargo test --release -p snipsy app_icon -q >/dev/null
test -s "$WORK/icon.png"
ICONSET="$WORK/Snipsy.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size "$WORK/icon.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$WORK/icon.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$WORK/Snipsy.icns"

echo "→ Armando ${APP}…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/snipsy "$APP/Contents/MacOS/snipsy"
cp "$WORK/Snipsy.icns" "$APP/Contents/Resources/Snipsy.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Snipsy</string>
  <key>CFBundleDisplayName</key><string>Snipsy</string>
  <key>CFBundleIdentifier</key><string>app.snipsy.editor</string>
  <key>CFBundleExecutable</key><string>snipsy</string>
  <key>CFBundleIconFile</key><string>Snipsy</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Firma local (ad hoc): sin ella macOS puede negarse a abrir la app modificada.
codesign --force --deep --sign - "$APP" >/dev/null
echo "✅ ${APP} listo."

if [[ "${1:-}" == "--install" ]]; then
  rm -rf /Applications/Snipsy.app
  cp -R "$APP" /Applications/
  # Que el Dock y Finder vean el ícono nuevo.
  touch /Applications/Snipsy.app
  echo "✅ Instalado en /Applications/Snipsy.app"
fi
