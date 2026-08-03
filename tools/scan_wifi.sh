#!/usr/bin/env bash
set -euo pipefail

echo "========================================="
echo "       Scanning Wi-Fi Networks           "
echo "========================================="

sudo ip link set wlan0 up 2>/dev/null || true

if command -v iw &>/dev/null; then
    sudo iw dev wlan0 scan | awk '
        /BSS / { bss=$2 }
        /signal:/ { signal=$2 " " $3 }
        /SSID:/ { print "SSID: " $2 "\tSignal: " signal "\tBSSID: " bss }
    '
elif command -v iwlist &>/dev/null; then
    sudo iwlist wlan0 scan | grep -E "ESSID|Quality|Signal|Channel"
elif command -v wpa_cli &>/dev/null; then
    sudo wpa_cli -i wlan0 scan >/dev/null
    sleep 2
    sudo wpa_cli -i wlan0 scan_results
else
    echo "No Wi-Fi scanning tool installed. Install iw, wireless-tools, or wavemon:"
    echo "  sudo apt update && sudo apt install -y iw wavemon wireless-tools"
fi
