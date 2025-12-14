#!/bin/bash
set -e

# ============================================
# Deploy to 100 Servers in 10 Minutes
# ============================================

PROVIDER=${1:-"hetzner"}  # hetzner, aws, gcp, digitalocean
NUM_SERVERS=${2:-100}

echo "🚀 Cell Substrate - Mass Deployment"
echo "═══════════════════════════════════"
echo "Provider: $PROVIDER"
echo "Servers:  $NUM_SERVERS"
echo ""

# ============================================
# Step 1: Build Worker Binary (1 minute)
# ============================================
echo "📦 [1/5] Building worker binary..."
cargo build --release -p worker
BINARY_PATH="target/release/worker"
BINARY_SIZE=$(du -h $BINARY_PATH | cut -f1)
echo "    ✓ Built: $BINARY_SIZE"

# ============================================
# Step 2: Provision Servers (3 minutes)
# ============================================
echo ""
echo "☁️  [2/5] Provisioning $NUM_SERVERS servers..."

case $PROVIDER in
    hetzner)
        # Hetzner Cloud (€3/month per server)
        echo "    Using Hetzner CX11 (1 vCPU, 2GB RAM)"
        
        for i in $(seq 1 $NUM_SERVERS); do
            hcloud server create \
                --name "cell-worker-$i" \
                --type cx11 \
                --image ubuntu-22.04 \
                --ssh-key default \
                > /dev/null 2>&1 &
            
            if (( i % 10 == 0 )); then
                echo "    ✓ Created $i/$NUM_SERVERS servers..."
            fi
        done
        wait
        
        echo "    ✓ All servers provisioned"
        echo "    💰 Cost: €$((NUM_SERVERS * 3))/month"
        ;;
        
    aws)
        # AWS EC2 t4g.nano ($3/month per server)
        echo "    Using t4g.nano (2 vCPU, 0.5GB RAM)"
        
        aws ec2 run-instances \
            --image-id ami-0c55b159cbfafe1f0 \
            --instance-type t4g.nano \
            --count $NUM_SERVERS \
            --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=cell-worker}]" \
            > servers.json
        
        echo "    ✓ All servers provisioned"
        echo "    💰 Cost: \$$((NUM_SERVERS * 3))/month"
        ;;
        
    local)
        # Local testing (Docker containers)
        echo "    Using Docker containers (free)"
        
        for i in $(seq 1 $NUM_SERVERS); do
            docker run -d \
                --name "cell-worker-$i" \
                --network host \
                -v $(pwd)/$BINARY_PATH:/app/worker \
                ubuntu:22.04 \
                /app/worker \
                > /dev/null 2>&1 &
        done
        wait
        
        echo "    ✓ All containers started"
        echo "    💰 Cost: $0/month"
        ;;
esac

# ============================================
# Step 3: Deploy Binary (2 minutes)
# ============================================
echo ""
echo "📤 [3/5] Deploying binary to servers..."

# Get server IPs
if [ "$PROVIDER" = "hetzner" ]; then
    IPS=$(hcloud server list -o columns=ipv4 | tail -n +2)
elif [ "$PROVIDER" = "aws" ]; then
    IPS=$(jq -r '.Instances[].PublicIpAddress' servers.json)
elif [ "$PROVIDER" = "local" ]; then
    IPS="127.0.0.1"
fi

# Parallel upload to all servers
echo "$IPS" | xargs -P 20 -I {} sh -c '
    echo "    → Deploying to {}"
    scp -o StrictHostKeyChecking=no '$BINARY_PATH' root@{}:/usr/local/bin/worker
    ssh -o StrictHostKeyChecking=no root@{} "chmod +x /usr/local/bin/worker"
' > /dev/null 2>&1

echo "    ✓ Binary deployed to $NUM_SERVERS servers"

# ============================================
# Step 4: Start Workers (1 minute)
# ============================================
echo ""
echo "▶️  [4/5] Starting workers..."

echo "$IPS" | xargs -P 20 -I {} sh -c '
    ssh -o StrictHostKeyChecking=no root@{} "
        export CELL_LAN=1
        export HOSTNAME=worker-$(hostname)
        nohup /usr/local/bin/worker > /dev/null 2>&1 &
    "
' > /dev/null 2>&1

echo "    ✓ All workers started"

# Wait for workers to discover each other
echo "    ⏳ Waiting for mesh formation..."
sleep 10

# ============================================
# Step 5: Verify Deployment (30 seconds)
# ============================================
echo ""
echo "✅ [5/5] Verifying deployment..."

# Run orchestrator
cargo run --release -p orchestrator 2>&1 | tee deployment.log

# Parse results
DISCOVERED=$(grep "Connected to worker swarm" deployment.log | wc -l)
THROUGHPUT=$(grep "Throughput:" deployment.log | awk '{print $2}')

echo ""
echo "═══════════════════════════════════"
echo "🎉 Deployment Complete!"
echo "═══════════════════════════════════"
echo "Servers Deployed:  $NUM_SERVERS"
echo "Workers Discovered: $DISCOVERED"
echo "Throughput:        $THROUGHPUT tasks/sec"
echo ""
echo "📊 View live stats:"
echo "   cargo run --release -p orchestrator"
echo ""
echo "🗑️  Teardown:"
echo "   ./teardown.sh $PROVIDER"