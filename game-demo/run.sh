#!/bin/bash
# Run the complete game demo using the orchestrator

set -e

echo "🎮 Cell Game Engine Demo"
echo "========================"

# Clean any previous state
rm -rf .cell/run .cell/io .cell/neighbors 2>/dev/null || true

# Run orchestrator (auto-discovers and starts all cells)
echo ""
echo "🚀 Starting orchestrator..."
cargo run --release -p orchestrator

# Note: The orchestrator runs in the foreground.
# Press Ctrl+C to stop.