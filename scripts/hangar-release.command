#!/bin/bash
# Publicar Hangar, paso a paso y sin recordar comandos.
#
#   Doble clic en Finder sobre este fichero (abre Terminal), o:
#   npm run release:guided
#
# Prepara la máquina si le falta algo, propone la versión, publica con
# `scripts/release.sh` (que sigue haciendo todas sus comprobaciones) y, si
# quieres, deja un zip en el Escritorio para instalar a mano en otro Mac.
# Todo el detalle está en UPDATES.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

KEY="$HOME/.tauri/hangar.key"
BUILT_APP="src-tauri/target/release/bundle/macos/Hangar.app"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
fail() {
  printf '  \033[31m✗\033[0m %s\n' "$*"
  echo
  read -r -p "Pulsa Enter para cerrar." _ || true
  exit 1
}

# Deja un zip instalable en el Escritorio a partir de un .app.
make_zip() {
  local app="$1"
  local version
  version="$(defaults read "$PWD/$app/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null \
    || defaults read "$app/Contents/Info.plist" CFBundleShortVersionString)"
  local zip="$HOME/Desktop/Hangar-$version.zip"
  rm -f "$zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
  ok "zip listo: $zip ($(du -h "$zip" | cut -f1))"
  echo "     En el otro Mac: cierra Hangar, descomprime, arrastra Hangar.app a Aplicaciones (Reemplazar),"
  echo "     y ábrelo con clic derecho → Abrir la primera vez."
  ZIP_PATH="$zip"
}

echo
bold "Hangar — publicar una versión"
echo

# ── 1. La máquina ──────────────────────────────────────────────────────────
bold "1. Comprobando esta máquina"
[ "$(uname -s)-$(uname -m)" = "Darwin-arm64" ] || fail "Tiene que ser un Mac con chip Apple: la build es para Apple Silicon."
ok "Mac con chip Apple"
command -v node >/dev/null || fail "Falta Node.js (https://nodejs.org)."
command -v cargo >/dev/null || fail "Falta Rust (https://rustup.rs)."
ok "Node $(node -v) y $(cargo --version | cut -d' ' -f1-2)"
[ -d node_modules ] || { warn "Instalando dependencias (npm install)…"; npm install --silent; }

[ -f "$KEY" ] || fail "No está la clave de firma en $KEY. Cópiala desde el Mac donde publicas normalmente (y su .pub). Sin ella las apps instaladas rechazarían la actualización."
APP_PUBKEY="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["plugins"]["updater"]["pubkey"].strip())')"
[ -f "$KEY.pub" ] && [ "$(tr -d '[:space:]' < "$KEY.pub")" = "$APP_PUBKEY" ] \
  || fail "La clave de $KEY no es la que esperan las apps instaladas (pubkey de tauri.conf.json). Usa la clave original."
ok "clave de firma correcta"

if security find-generic-password -s hangar-supabase-service-role -w >/dev/null 2>&1; then
  ok "clave de Supabase en el Llavero"
else
  warn "Falta la clave 'service_role' de Supabase en el Llavero."
  echo "     Está en Supabase → Project Settings → API → service_role. No se mostrará al pegarla."
  read -r -s -p "     Pégala aquí y pulsa Enter: " SERVICE_KEY
  echo
  [ -n "$SERVICE_KEY" ] || fail "No se pegó nada."
  security add-generic-password -U -a hangar -s hangar-supabase-service-role \
    -l "Hangar release: Supabase service role" -w "$SERVICE_KEY"
  unset SERVICE_KEY
  ok "guardada en el Llavero"
fi
echo

# ── 2. Versión ─────────────────────────────────────────────────────────────
CURRENT="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
SUGGESTED="$(python3 -c "v='$CURRENT'.split('.'); v[2]=str(int(v[2])+1); print('.'.join(v))")"
bold "2. Versión"
echo "  Actual: v$CURRENT"
read -r -p "  Nueva versión [$SUGGESTED]: " VERSION
VERSION="${VERSION:-$SUGGESTED}"
read -r -p "  Notas (opcional, Enter para 'Hangar $VERSION'): " NOTES
echo

# ── 3. Qué hacer ───────────────────────────────────────────────────────────
bold "3. ¿Qué quieres hacer con v$VERSION?"
echo "  1) Publicar (tests, build firmada, Supabase, commit + tag + push)"
echo "  2) Solo probar (dry run: todo menos publicar)"
echo "  3) Solo el zip para instalar a mano en otro Mac"
echo "  q) Salir"
read -r -p "  Elige [1]: " CHOICE
CHOICE="${CHOICE:-1}"
echo

ZIP_PATH=""
case "$CHOICE" in
  1)
    bold "Publicando v$VERSION…"
    npm run --silent release -- "$VERSION" ${NOTES:+"$NOTES"}
    echo
    read -r -p "¿Dejo también un zip en el Escritorio para otro Mac? [s/N] " WANT_ZIP
    if [[ "$WANT_ZIP" =~ ^[sSyY] ]]; then make_zip "$BUILT_APP"; fi
    echo
    bold "Listo: v$VERSION publicada."
    echo "  Los Hangar con sesión iniciada la descargan solos; solo tendrán que pulsar Restart."
    [ -n "$ZIP_PATH" ] && echo "  Zip para instalar a mano: $ZIP_PATH"
    ;;
  2)
    bold "Probando v$VERSION sin publicar…"
    npm run --silent release -- "$VERSION" ${NOTES:+"$NOTES"} --dry-run
    echo
    bold "Dry run correcto: no se ha publicado ni cambiado nada."
    ;;
  3)
    # La build recién hecha si corresponde a la versión actual; si no, la app instalada.
    if [ -d "$BUILT_APP" ] && [ "$(defaults read "$PWD/$BUILT_APP/Contents/Info.plist" CFBundleShortVersionString)" = "$CURRENT" ]; then
      make_zip "$BUILT_APP"
    elif [ -d /Applications/Hangar.app ]; then
      make_zip /Applications/Hangar.app
    else
      fail "No hay ninguna build de Hangar para comprimir."
    fi
    ;;
  q | Q) echo "Nada hecho." ;;
  *) fail "Opción no válida: $CHOICE" ;;
esac

echo
read -r -p "Pulsa Enter para cerrar." _ || true
