#!/bin/bash
set -e

echo "🔬 Cell Substrate 60-Second Demo"
echo "=================================="

# ============================================
# MACHINE 1 (Your Laptop)
# ============================================
laptop_setup() {
    echo "📍 Setting up LOCAL cell..."
    
    # Create project structure
    mkdir -p hello/src client/src
    
    # Generate files (copy from artifacts above)
    # ... (files would be created here)
    
    # Build and run
    cargo build --release -p hello
    
    echo "🚀 Starting cell on localhost..."
    cargo run --release -p hello &
    
    sleep 2
    
    echo "📞 Testing local connection..."
    cargo run --release -p client
}

# ============================================
# MACHINE 2 (Cloud Server)
# ============================================
cloud_setup() {
    echo "☁️  Setting up CLOUD cell..."
    
    # Get cloud IP
    CLOUD_IP=$(curl -s ifconfig.me)
    echo "📡 Cloud IP: $CLOUD_IP"
    
    # Enable LAN discovery
    export CELL_LAN=1
    export CELL_IP=$CLOUD_IP
    
    # Build and run
    cargo build --release -p hello
    
    echo "🌐 Starting cell on $CLOUD_IP..."
    cargo run --release -p hello
}

# ============================================
# CROSS-NETWORK TEST
# ============================================
cross_network_test() {
    echo "🌍 Testing cross-network communication..."
    
    # Wait for discovery
    sleep 5
    
    # Client will auto-discover cloud cell
    export CELL_LAN=1
    cargo run --release -p client
    
    echo "✅ SUCCESS! Laptop ↔ Cloud mesh established!"
}

# ============================================
# USAGE
# ============================================
case "${1:-local}" in
    local)
        laptop_setup
        ;;
    cloud)
        cloud_setup
        ;;
    test)
        cross_network_test
        ;;
    *)
        echo "Usage: $0 {local|cloud|test}"
        exit 1
        ;;
esac