#!/usr/bin/env bash

set -euo pipefail

echo "========================================"
echo "    Wi-Fi Network Setup Assistant       "
echo "========================================"
echo ""

echo "Select Network Type:"
echo "1) Standard (WPA-Personal with Hashed PSK)"
echo "2) Enterprise / Eduroam (WPA-EAP PEAP/MSCHAPv2)"
read -rp "Enter choice [1-2]: " NET_TYPE

if [[ "$NET_TYPE" == "1" ]]; then
    # ----------------------------------------------------
    # STANDARD (WPA-PERSONAL)
    # ----------------------------------------------------
    read -rp "Enter Wi-Fi SSID: " SSID
    if [[ -z "$SSID" ]]; then
        echo "Error: SSID cannot be empty." >&2
        exit 1
    fi

    read -rsp "Enter Wi-Fi Password: " PASSWORD
    echo ""
    if [[ -z "$PASSWORD" ]]; then
        echo "Error: Password cannot be empty." >&2
        exit 1
    fi

    HASH_PSK=""
    if command -v wpa_passphrase &> /dev/null; then
        HASH_PSK=$(wpa_passphrase "$SSID" "$PASSWORD" | grep -E "^\s*psk=" | cut -d'=' -f2)
    elif command -v python3 &> /dev/null; then
        HASH_PSK=$(python3 -c "import hashlib; print(hashlib.pbkdf2_hmac('sha1', '''$PASSWORD'''.encode('utf-8'), '''$SSID'''.encode('utf-8'), 4096, 32).hex())")
    else
        echo "Error: Neither 'wpa_passphrase' nor 'python3' is available to compute the PSK hash." >&2
        exit 1
    fi

    if [[ -z "$HASH_PSK" ]]; then
        echo "Error: Failed to generate PSK hash." >&2
        exit 1
    fi

    echo ""
    echo "----------------------------------------"
    echo "SSID: $SSID"
    echo "Generated Hashed PSK: $HASH_PSK"
    echo "----------------------------------------"
    echo ""

    echo "Select network configuration method:"
    echo "1) wpa_supplicant (Raspberry Pi OS / Debian / Arch)"
    echo "2) NetworkManager (nmcli)"
    echo "3) Netplan (Ubuntu)"
    read -rp "Enter choice [1-3]: " CHOICE

    case "$CHOICE" in
        1)
            echo ""
            echo "Please execute the following sudo commands on the device:"
            echo ""
            echo "--- Command 1: Append configuration to /etc/wpa_supplicant/wpa_supplicant.conf ---"
            echo "sudo bash -c 'cat <<EOF >> /etc/wpa_supplicant/wpa_supplicant.conf"
            echo "network={"
            echo "    ssid=\"$SSID\""
            echo "    psk=$HASH_PSK"
            echo "}"
            echo "EOF'"
            echo ""
            echo "--- Command 2: Reconfigure wpa_supplicant ---"
            echo "sudo wpa_cli -i wlan0 reconfigure"
            ;;
        2)
            echo ""
            echo "Please execute the following sudo command on the device:"
            echo ""
            echo "sudo nmcli dev wifi connect \"$SSID\" password \"$HASH_PSK\""
            ;;
        3)
            echo ""
            echo "Please execute the following sudo command/configuration on the device:"
            echo ""
            echo "1) Edit /etc/netplan/50-cloud-init.yaml:"
            echo "----------------------------------------"
            echo "network:"
            echo "  version: 2"
            echo "  wifis:"
            echo "    wlan0:"
            echo "      dhcp4: true"
            echo "      access-points:"
            echo "        \"$SSID\":"
            echo "          password: \"$HASH_PSK\""
            echo "----------------------------------------"
            echo ""
            echo "2) Apply netplan changes:"
            echo "sudo netplan apply"
            ;;
        *)
            echo "Invalid choice." >&2
            exit 1
            ;;
    esac

elif [[ "$NET_TYPE" == "2" ]]; then
    # ----------------------------------------------------
    # ENTERPRISE / EDUROAM (WPA-EAP)
    # ----------------------------------------------------
    read -rp "Enter Wi-Fi SSID [default: eduroam]: " SSID
    SSID=${SSID:-eduroam}

    read -rp "Enter Identity/Username (e.g. user@institution.ac.uk): " IDENTITY
    if [[ -z "$IDENTITY" ]]; then
        echo "Error: Identity cannot be empty." >&2
        exit 1
    fi

    read -rsp "Enter Password: " PASSWORD
    echo ""
    if [[ -z "$PASSWORD" ]]; then
        echo "Error: Password cannot be empty." >&2
        exit 1
    fi

    read -rp "Enter EAP Method [default: peap]: " EAP_METHOD
    EAP_METHOD=${EAP_METHOD:-peap}

    read -rp "Enter Phase2 Auth [default: MSCHAPV2]: " PHASE2_AUTH
    PHASE2_AUTH=${PHASE2_AUTH:-MSCHAPV2}

    echo ""
    echo "----------------------------------------"
    echo "SSID: $SSID"
    echo "Identity: $IDENTITY"
    echo "EAP Method: $EAP_METHOD"
    echo "Phase2 Auth: $PHASE2_AUTH"
    echo "----------------------------------------"
    echo ""

    echo "Select network configuration method:"
    echo "1) wpa_supplicant (Raspberry Pi OS / Debian / Arch)"
    echo "2) NetworkManager (nmcli)"
    echo "3) Netplan (Ubuntu)"
    read -rp "Enter choice [1-3]: " CHOICE

    case "$CHOICE" in
        1)
            EAP_UPPER=$(echo "$EAP_METHOD" | tr '[:lower:]' '[:upper:]')
            echo ""
            echo "Please execute the following sudo commands on the device:"
            echo ""
            echo "--- Command 1: Append configuration to /etc/wpa_supplicant/wpa_supplicant.conf ---"
            echo "sudo bash -c 'cat <<EOF >> /etc/wpa_supplicant/wpa_supplicant.conf"
            echo "network={"
            echo "    ssid=\"$SSID\""
            echo "    key_mgmt=WPA-EAP"
            echo "    eap=$EAP_UPPER"
            echo "    identity=\"$IDENTITY\""
            echo "    password=\"$PASSWORD\""
            echo "    phase2=\"auth=$PHASE2_AUTH\""
            echo "}"
            echo "EOF'"
            echo ""
            echo "--- Command 2: Reconfigure wpa_supplicant ---"
            echo "sudo wpa_cli -i wlan0 reconfigure"
            ;;
        2)
            echo ""
            echo "Please execute the following sudo command on the device:"
            echo ""
            echo "sudo nmcli con add type wifi ifname wlan0 ssid \"$SSID\" -- \\"
            echo "  802-11-wireless-security.key-mgmt wpa-eap \\"
            echo "  802-1x.eap \"$EAP_METHOD\" \\"
            echo "  802-1x.identity \"$IDENTITY\" \\"
            echo "  802-1x.password \"$PASSWORD\" \\"
            echo "  802-1x.phase2-auth \"$(echo "$PHASE2_AUTH" | tr '[:upper:]' '[:lower:]')\""
            echo "sudo nmcli con up \"$SSID\""
            ;;
        3)
            echo ""
            echo "Please execute the following sudo command/configuration on the device:"
            echo ""
            echo "1) Edit /etc/netplan/50-cloud-init.yaml:"
            echo "----------------------------------------"
            echo "network:"
            echo "  version: 2"
            echo "  wifis:"
            echo "    renderer: networkd"
            echo "    wlan0:"
            echo "      dhcp4: true"
            echo "      access-points:"
            echo "        \"$SSID\":"
            echo "          auth:"
            echo "            key-management: eap"
            echo "            identity: \"$IDENTITY\""
            echo "            password: \"$PASSWORD\""
            echo "            method: $EAP_METHOD"
            echo "            phase2-auth: \"$PHASE2_AUTH\""
            echo "----------------------------------------"
            echo ""
            echo "2) Apply netplan changes:"
            echo "sudo netplan apply"
            ;;
        *)
            echo "Invalid choice." >&2
            exit 1
            ;;
    esac

else
    echo "Invalid network type selection." >&2
    exit 1
fi

echo ""
