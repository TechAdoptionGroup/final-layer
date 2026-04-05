#!/bin/bash
# Final Layer Validator Deployment Script
# Usage: ./deploy-validator.sh <VALIDATOR_ACCOUNT_ID>
#
# Prerequisites:
#   - Ubuntu 22.04 LTS
#   - 4+ vCPU, 8GB+ RAM, 200GB+ SSD
#   - Rust toolchain (cargo)
#   - neard binary built from this repo
#
# This script sets up a new validator node from scratch.

set -e

VALIDATOR_ACCOUNT="${1:-validator.fl}"
NODE_HOME="/root/.fl-node"
NEARD_BIN="/usr/local/bin/neard"
SERVICE_NAME="fl-node"

echo "=== Final Layer Validator Setup ==="
echo "Validator account: $VALIDATOR_ACCOUNT"
echo "Node home: $NODE_HOME"

# ---- Step 1: Initialize node ----
echo ""
echo "[1/5] Initializing node..."
$NEARD_BIN --home "$NODE_HOME" init \
  --chain-id final-layer-mainnet \
  --account-id "$VALIDATOR_ACCOUNT"

# ---- Step 2: Configure genesis ----
echo ""
echo "[2/5] Configuring genesis..."
echo "  Copy your genesis.json to $NODE_HOME/genesis.json"
echo "  The genesis file must match the network you are joining."
echo "  Contact the network admin for the correct genesis.json."

# ---- Step 3: Configure boot nodes ----
echo ""
echo "[3/5] Configure boot nodes in $NODE_HOME/config.json"
echo "  Edit the 'boot_nodes' field to include the network's boot nodes."
echo "  Example: edit config.json and set:"
echo '    "boot_nodes": ["ed25519:<pubkey>@<ip>:24567"]'

# ---- Step 4: Set up systemd service ----
echo ""
echo "[4/5] Setting up systemd service..."
cat > /etc/systemd/system/${SERVICE_NAME}.service << EOF
[Unit]
Description=Final Layer Node
After=network.target

[Service]
Type=simple
User=root
ExecStart=${NEARD_BIN} --home ${NODE_HOME} run
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
echo "  Service ${SERVICE_NAME} registered."

# ---- Step 5: Firewall ----
echo ""
echo "[5/5] Opening required ports..."
if command -v ufw &>/dev/null; then
  ufw allow 24567/tcp  # P2P
  ufw allow 3030/tcp   # RPC (restrict to trusted IPs in production)
  echo "  Ports 24567 (P2P) and 3030 (RPC) opened."
fi

echo ""
echo "=== Setup complete ==="
echo ""
echo "Next steps:"
echo "  1. Copy genesis.json to $NODE_HOME/genesis.json"
echo "  2. Update $NODE_HOME/config.json with boot_nodes"
echo "  3. Generate your validator key (see docs/pqc_algorithms.md for key types)"
echo "  4. Deploy fl_staking_pool v5 contract to your validator account"
echo "  5. Start the node: systemctl start $SERVICE_NAME"
echo "  6. Check status:   systemctl status $SERVICE_NAME"
echo "  7. Follow logs:    journalctl -u $SERVICE_NAME -f"
echo ""
echo "  Your node_key.json is at: $NODE_HOME/node_key.json"
echo "  Your validator_key.json is at: $NODE_HOME/validator_key.json"
echo "  KEEP THESE FILES SECURE."
