#!/usr/bin/env bash
# Script de instalación universal para c-script (linkc) en Linux y macOS
# Uso: curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh

set -e

REPO="charlessonamericantrading/c-script-"
INSTALL_DIR="${LINK_INSTALL_DIR:-$HOME/.c-script/bin}"
mkdir -p "$INSTALL_DIR"

OS="$(uname -s)"
ARCH="$(uname -m)"

echo "⚡ Instalando c-script (linkc) para $OS ($ARCH)..."

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64) ASSET="linkc-x86_64-unknown-linux-gnu.tar.gz" ;;
      *) echo "Arquitectura $ARCH no soportada automáticamente en Linux. Compilando desde el código fuente..."; ASSET="" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) ASSET="linkc-x86_64-apple-darwin.tar.gz" ;;
      arm64|aarch64) ASSET="linkc-aarch64-apple-darwin.tar.gz" ;;
      *) echo "Arquitectura $ARCH no soportada en macOS."; ASSET="" ;;
    esac
    ;;
  *)
    echo "Sistema operativo $OS no soportado por este script. Para Windows usa install.ps1"
    exit 1
    ;;
esac

DOWNLOAD_SUCCESS=false

if [ -n "$ASSET" ]; then
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
  echo "Descargando binario precompilado desde GitHub Releases..."
  if curl -fsSL "$URL" | tar -xz -C "$INSTALL_DIR" 2>/dev/null; then
    DOWNLOAD_SUCCESS=true
  fi
fi

if [ "$DOWNLOAD_SUCCESS" = false ]; then
  echo "No se pudo descargar el binario precompilado. Intentando compilar localmente con cargo..."
  if command -v cargo >/dev/null 2>&1; then
    TEMP_DIR=$(mktemp -d)
    git clone --depth 1 "https://github.com/$REPO.git" "$TEMP_DIR" 2>/dev/null || true
    if [ -d "$TEMP_DIR/compiler" ]; then
      (cd "$TEMP_DIR/compiler" && cargo build --release)
      cp "$TEMP_DIR/compiler/target/release/linkc" "$INSTALL_DIR/linkc"
      rm -rf "$TEMP_DIR"
      DOWNLOAD_SUCCESS=true
    fi
  fi
fi

if [ ! -f "$INSTALL_DIR/linkc" ]; then
  echo "❌ Error: no se pudo instalar linkc. Asegúrate de tener conexión a Internet o Rust/Cargo instalado."
  exit 1
fi

chmod +x "$INSTALL_DIR/linkc"

echo ""
echo "========================================================="
echo " 🎉 ¡c-script (linkc) instalado con éxito!"
echo " Ubicación: $INSTALL_DIR/linkc"
echo "========================================================="
echo ""
echo "Para agregarlo a tu PATH permanentemente, añade esto a tu ~/.bashrc o ~/.zshrc:"
echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
echo ""
echo "Comienza ejecutando:"
echo "  linkc --help"
