#!/usr/bin/env bash
set -euo pipefail

echo "========================================="
echo "   Eduroam WLAN Activation & Verification"
echo "========================================="

REAL_USER=${SUDO_USER:-$(whoami)}
USER_HOME=$(eval echo ~$REAL_USER)
CAT_CONF="$USER_HOME/.config/cat_installer/cat_installer.conf"
CONF_SRC="/etc/wpa_supplicant/wpa_supplicant.conf"
CONF_DST="/etc/wpa_supplicant/wpa_supplicant-wlan0.conf"
NET_CONF="/etc/systemd/network/20-wlan.network"
IFACE="wlan0"

# Require sudo privileges
if [[ $EUID -ne 0 ]]; then
    echo "[!] Re-running script with sudo privileges..."
    exec sudo "$0" "$@"
fi

if [[ -f "$CAT_CONF" ]]; then
    echo "[1/6] Restoring clean eduroam configuration from $CAT_CONF..."
    cp "$CAT_CONF" "$CONF_SRC"
    cp "$CAT_CONF" "$CONF_DST"
    chmod 600 "$CONF_SRC" "$CONF_DST"
elif [[ -f "$CONF_SRC" ]]; then
    echo "[1/6] Using existing $CONF_SRC..."
    cp "$CONF_SRC" "$CONF_DST"
    chmod 600 "$CONF_DST"
else
    echo "[X] Error: No configuration found. Please run ~/tools/eduroam-linux-installer.py first." >&2
    exit 1
fi

echo "[2/6] Resetting wpa_supplicant for $IFACE..."
systemctl stop wpa_supplicant 2>/dev/null || true
systemctl stop wpa_supplicant@${IFACE}.service 2>/dev/null || true

ip link set "$IFACE" down 2>/dev/null || true
sleep 1
ip link set "$IFACE" up 2>/dev/null || true

systemctl enable wpa_supplicant@${IFACE}.service
systemctl start wpa_supplicant@${IFACE}.service

echo "[3/6] Configuring systemd-networkd for DHCP on $IFACE..."
cat <<'NET_EOF' > "$NET_CONF"
[Match]
Name=wlan0

[Network]
DHCP=yes
NET_EOF
chmod 644 "$NET_CONF"

echo "[4/6] Enabling ping for unprivileged users..."
chmod u+s /usr/bin/ping 2>/dev/null || true
sysctl -w net.ipv4.ping_group_range=0 2147483647 >/dev/null 2>&1 || true

echo "[5/6] Restarting systemd-networkd..."
systemctl restart systemd-networkd

echo "[6/6] Waiting for Wi-Fi authentication & DHCP IP assignment..."
CONNECTED=false
IP_ADDR=""
SSID=""
STATE=""

for i in {1..25}; do
    STATUS=$(wpa_cli -i "$IFACE" status 2>/dev/null || true)
    STATE=$(echo "$STATUS" | grep "^wpa_state=" | cut -d'=' -f2 || echo "UNKNOWN")
    SSID=$(echo "$STATUS" | grep "^ssid=" | cut -d'=' -f2 || echo "UNKNOWN")
    IP_ADDR=$(ip -4 addr show dev "$IFACE" 2>/dev/null | awk '/inet / {print $2}' | cut -d'/' -f1 || echo "")

    if [[ "$STATE" == "COMPLETED" && -n "$IP_ADDR" ]]; then
        CONNECTED=true
        break
    fi
    sleep 1
done

echo ""
echo "========================================="
if [[ "$CONNECTED" == "true" ]]; then
    echo " [✓] WLAN Connected & Active Successfully!"
    echo " Interface: $IFACE"
    echo " SSID:      $SSID"
    echo " IP Addr:   $IP_ADDR"
else
    echo " [!] Current status after configuration:"
    echo " State:     ${STATE:-UNKNOWN}"
    echo " SSID:      ${SSID:-UNKNOWN}"
    echo " IP Addr:   ${IP_ADDR:-None}"
fi
echo "========================================="
