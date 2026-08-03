#!/usr/bin/env bash
set -euo pipefail

IFACE="enx0050b6a68eab"
IP_ADDR="192.168.120.1/24"

echo "========================================="
echo " Configuring Management Port: $IFACE"
echo "========================================="

CONF_FILE="/etc/systemd/network/10-mgmt-port.network"

echo "1. Creating systemd-networkd configuration at $CONF_FILE..."
sudo bash -c "cat <<'NET_EOF' > $CONF_FILE
[Match]
Name=$IFACE

[Network]
Address=$IP_ADDR
DHCPServer=yes

[DHCPServer]
PoolOffset=100
PoolSize=50
DefaultLeaseTimeSec=86400
EmitDNS=yes
DNS=192.168.120.1
NET_EOF"

sudo chmod 644 "$CONF_FILE"

echo "2. Restarting systemd-networkd service..."
sudo systemctl restart systemd-networkd

echo ""
echo "========================================="
echo " [✓] Management Port Configured Successfully"
echo "========================================="
echo " Interface:  $IFACE"
echo " Static IP:  192.168.120.1/24"
echo " DHCP Pool:  192.168.120.100 - 192.168.120.150"
echo "========================================="
