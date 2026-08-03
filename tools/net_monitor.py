#!/usr/bin/env python3
import os
import time
import socket
import fcntl
import struct

def get_ip(ifname):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        return socket.inet_ntoa(fcntl.ioctl(
            s.fileno(),
            0x8915,  # SIOCGIFADDR
            struct.pack('256s', ifname[:15].encode('utf-8'))
        )[20:24])
    except IOError:
        return "No IP"

def get_mac(ifname):
    try:
        with open(f"/sys/class/net/{ifname}/address") as f:
            return f.read().strip()
    except Exception:
        return "-"

def get_bytes(ifname):
    try:
        with open(f"/sys/class/net/{ifname}/statistics/rx_bytes") as f:
            rx = int(f.read().strip())
        with open(f"/sys/class/net/{ifname}/statistics/tx_bytes") as f:
            tx = int(f.read().strip())
        return rx, tx
    except Exception:
        return 0, 0

def get_state(ifname):
    try:
        with open(f"/sys/class/net/{ifname}/operstate") as f:
            oper = f.read().strip()
        carrier_path = f"/sys/class/net/{ifname}/carrier"
        if os.path.exists(carrier_path):
            with open(carrier_path) as f:
                carrier = f.read().strip()
            if carrier == "0" and oper != "down":
                return "NO-CARRIER"
        return oper.upper()
    except Exception:
        return "UNKNOWN"

def format_rate(bytes_per_sec):
    if bytes_per_sec < 1024:
        return f"{bytes_per_sec:6.1f} B/s"
    elif bytes_per_sec < 1024 * 1024:
        return f"{bytes_per_sec/1024:6.1f} KB/s"
    else:
        return f"{bytes_per_sec/(1024*1024):6.1f} MB/s"

def main():
    prev_stats = {}
    prev_time = time.time()

    print("\033[2J")  # Clear screen

    try:
        while True:
            curr_time = time.time()
            dt = max(curr_time - prev_time, 0.001)
            prev_time = curr_time

            interfaces = sorted([i for i in os.listdir('/sys/class/net') if i != 'lo'])
            
            lines = []
            lines.append("\033[H")  # Move cursor to top left
            lines.append("\033[1;36m========================================================================================\033[0m")
            lines.append("\033[1;36m                      LIVE NETWORK INTERFACE MONITOR                                    \033[0m")
            lines.append("\033[1;36m========================================================================================\033[0m")
            lines.append(f"\033[1;33m{'INTERFACE':<18} {'STATE':<12} {'IP ADDRESS':<16} {'MAC ADDRESS':<18} {'RX RATE':<12} {'TX RATE':<12}\033[0m")
            lines.append("----------------------------------------------------------------------------------------")

            for iface in interfaces:
                ip = get_ip(iface)
                mac = get_mac(iface)
                state = get_state(iface)
                rx, tx = get_bytes(iface)

                prev_rx, prev_tx = prev_stats.get(iface, (rx, tx))
                rx_rate = (rx - prev_rx) / dt
                tx_rate = (tx - prev_tx) / dt
                prev_stats[iface] = (rx, tx)

                if state == "UP":
                    state_str = f"\033[1;32m{state:<12}\033[0m"
                elif state == "NO-CARRIER":
                    state_str = f"\033[1;33m{state:<12}\033[0m"
                else:
                    state_str = f"\033[1;31m{state:<12}\033[0m"

                rx_str = format_rate(rx_rate)
                tx_str = format_rate(tx_rate)

                lines.append(f"\033[1m{iface:<18}\033[0m {state_str} {ip:<16} {mac:<18} \033[32m{rx_str:<12}\033[0m \033[34m{tx_str:<12}\033[0m")

            lines.append("----------------------------------------------------------------------------------------")
            lines.append(" Press \033[1;31mCtrl+C\033[0m to exit.")

            print("\n".join(lines), flush=True)
            time.sleep(1)

    except KeyboardInterrupt:
        print("\nExiting monitor.")

if __name__ == "__main__":
    main()
