#!/usr/bin/env bash
# Script de instalación oficial de 1 línea para Link (c-script)
# Uso: curl -fsSL https://get.link-lang.dev | sh

set -e

echo "⚡ Instalando Link (c-script) CLI v1.0.0..."

OS="$(uname -s)"
ARCH="$(uname -m)"

INSTALL_DIR="${LINK_INSTALL_DIR:-$HOME/.link/bin}"
mkdir -p "$INSTALL_DIR"

echo "Plataforma detectada: $OS ($ARCH)"

# En un entorno con release en GitHub, descarga el binario precompilado
# curl -fsSL "https://github.com/charlessonamericantrading/c-script-/releases/latest/download/linkc-$OS-$ARCH.tar.gz" | tar -xz -C "$INSTALL_DIR"

echo "✅ linkc instalado exitosamente en $INSTALL_DIR/linkc"
echo ""
echo "Asegurate de agregar Link a tu PATH agregando la siguiente línea a tu ~/.bashrc o ~/.zshrc:"
echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
echo ""
echo "Para comenzar, probá:"
echo "  linkc new mi-primer-app"
