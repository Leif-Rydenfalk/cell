#!/bin/bash
# Cell Substrate - One-Line Installer
# Usage: curl -sSf https://cell.sh | sh

set -e

echo "Installing Cell Substrate..."

# ============================================
# Detect Environment
# ============================================
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS-$ARCH" in
    Linux-x86_64)   PLATFORM="linux-x64" ;;
    Darwin-arm64)   PLATFORM="darwin-arm64" ;;
    Darwin-x86_64)  PLATFORM="darwin-x64" ;;
    *)
        echo "Unsupported platform: $OS-$ARCH"
        exit 1
        ;;
esac

# ============================================
# Download Pre-Built Binary
# ============================================
BIN_URL="https://github.com/Leif-Rydenfalk/cell/releases/latest/download/cell-$PLATFORM"
INSTALL_DIR="$HOME/.cell/bin"

mkdir -p "$INSTALL_DIR"

echo "Downloading cell CLI..."
curl -L "$BIN_URL" -o "$INSTALL_DIR/cell"
chmod +x "$INSTALL_DIR/cell"

# ============================================
# Add to PATH
# ============================================
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo "export PATH=\"\$HOME/.cell/bin:\$PATH\"" >> "$HOME/.bashrc"
    echo "export PATH=\"\$HOME/.cell/bin:\$PATH\"" >> "$HOME/.zshrc"
    export PATH="$INSTALL_DIR:$PATH"
fi

# ============================================
# Initialize Runtime
# ============================================
echo "Initializing runtime..."
"$INSTALL_DIR/cell" init

# ============================================
# Create Demo Project
# ============================================
echo "Creating demo project..."
mkdir -p demo
cd demo

# Generate hello cell
"$INSTALL_DIR/cell" new hello --template service
# Generate client
"$INSTALL_DIR/cell" new client --template client

# ============================================
# Build Demo
# ============================================
echo "Building demo..."
cargo build --release

# ============================================
# Success
# ============================================
echo ""
echo "Installation complete!"
echo ""
echo "Try it now:"
echo "  cd demo"
echo "  cell run hello    # Start the service"
echo "  cell call hello ping \"World\"  # Call it"
echo ""
echo "Or go distributed:"
echo "  cell run hello --lan  # Enable network discovery"
echo ""
echo "Documentation: https://cell.sh/docs"