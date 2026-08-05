#!/usr/bin/env bash
set -euo pipefail

# Script to configure and control PWM0 fan (Pin 7) on Radxa ROCK 4C+ (RK3399)
# Designed to be executed directly on the device (or via docker/deployment scripts).

COMMAND="${1:-setup}"
ARG2="${2:-}"

OVERLAY_NAME="rockchip-rk3399-pwm0-fan"
DTS_PATH="/tmp/${OVERLAY_NAME}.dts"
DTBO_PATH="/tmp/${OVERLAY_NAME}.dtbo"

function check_sudo() {
    if [ "$EUID" -ne 0 ]; then
        SUDO="sudo"
    else
        SUDO=""
    fi
}

function setup_local() {
    echo "================================================="
    echo " Setting up PWM Fan Overlay on Board"
    echo "================================================="

    check_sudo

    if ! command -v dtc &> /dev/null; then
        echo "Error: device tree compiler (dtc) is not installed."
        echo "Please install it with: $SUDO apt-get update && $SUDO apt-get install -y device-tree-compiler"
        exit 1
    fi

    echo "1. Generating Device Tree Source..."
    cat << "DTS_EOF" > "$DTS_PATH"
/dts-v1/;
/plugin/;

/ {
	compatible = "rockchip,rk3399";

	fragment@0 {
		target = <&pwm0>;
		__overlay__ {
			status = "okay";
			pinctrl-names = "default";
			pinctrl-0 = <&pwm0_pin>;
		};
	};

	fragment@1 {
		target-path = "/";
		__overlay__ {
			fan0: pwm-fan {
				compatible = "pwm-fan";
				pwms = <&pwm0 0 1000000 0>;
				cooling-levels = <0 100 150 200 255>;
				#cooling-cells = <2>;
			};
		};
	};

	fragment@2 {
		target = <&cpu_thermal>;
		__overlay__ {
			polling-delay = <3000>;
			polling-delay-passive = <3000>;

			trips {
				cpu_alert0: cpu_alert0 {
					temperature = <50000>;
					hysteresis = <5000>;
					type = "passive";
				};
				cpu_alert1: cpu_alert1 {
					temperature = <65000>;
					hysteresis = <5000>;
					type = "passive";
				};
			};

			cooling-maps {
				map2 {
					trip = <&cpu_alert0>;
					cooling-device = <&fan0 0 4>;
				};
			};
		};
	};
};
DTS_EOF

    echo "2. Compiling DTBO..."
    dtc -@ -I dts -O dtb -o "$DTBO_PATH" "$DTS_PATH"

    echo "3. Installing overlay to /boot/overlay-user/..."
    $SUDO mkdir -p /boot/overlay-user
    $SUDO cp "$DTBO_PATH" "/boot/overlay-user/${OVERLAY_NAME}.dtbo"

    echo "4. Updating /boot/armbianEnv.txt..."
    if [ -f /boot/armbianEnv.txt ]; then
        $SUDO sed -i "/user_overlays=/d" /boot/armbianEnv.txt
        echo "user_overlays=${OVERLAY_NAME}" | $SUDO tee -a /boot/armbianEnv.txt >/dev/null
    else
        echo "Warning: /boot/armbianEnv.txt not found. Overlay installed to /boot/overlay-user/${OVERLAY_NAME}.dtbo"
    fi

    echo ""
    echo "================================================="
    echo " [✓] PWM Fan overlay configured successfully!"
    echo " Please reboot the board to activate: 'sudo reboot'"
    echo "================================================="
}

function set_speed() {
    SPEED="${1:-255}"
    if ! [[ "$SPEED" =~ ^[0-9]+$ ]] || [ "$SPEED" -lt 0 ] || [ "$SPEED" -gt 255 ]; then
        echo "Error: Speed must be an integer between 0 and 255."
        exit 1
    fi

    check_sudo
    PWM_FILE=$(ls /sys/class/hwmon/hwmon*/pwm1 2>/dev/null | head -n 1 || true)
    if [ -n "$PWM_FILE" ]; then
        echo "Setting fan speed to $SPEED ($PWM_FILE)..."
        echo "$SPEED" | $SUDO tee "$PWM_FILE" >/dev/null
        echo "Done."
    else
        echo "Error: PWM fan hardware control file (/sys/class/hwmon/hwmon*/pwm1) not found."
        echo "Ensure the board was rebooted after running 'setup'."
        exit 1
    fi
}

function get_status() {
    echo "================================================="
    echo " PWM Fan & Thermal Status"
    echo "================================================="
    echo "=== CPU / GPU Temperatures ==="
    for t in /sys/class/thermal/thermal_zone*; do
        if [ -d "$t" ]; then
            type=$(cat "$t/type" 2>/dev/null || echo "unknown")
            temp=$(cat "$t/temp" 2>/dev/null || echo "0")
            temp_c=$(awk "BEGIN {print $temp/1000}")
            echo "Zone $(basename "$t") ($type): ${temp_c}°C"
        fi
    done

    echo ""
    echo "=== Cooling Devices ==="
    for c in /sys/class/thermal/cooling_device*; do
        if [ -d "$c" ]; then
            type=$(cat "$c/type" 2>/dev/null || echo "unknown")
            state=$(cat "$c/cur_state" 2>/dev/null || echo "N/A")
            max_state=$(cat "$c/max_state" 2>/dev/null || echo "N/A")
            echo "$(basename "$c") ($type): state $state / $max_state"
        fi
    done

    echo ""
    echo "=== PWM Control Files ==="
    ls -l /sys/class/hwmon/hwmon*/pwm* 2>/dev/null || echo "No hwmon PWM devices found."
}

case "$COMMAND" in
    setup)
        setup_local
        ;;
    speed|set-speed)
        set_speed "$ARG2"
        ;;
    status)
        get_status
        ;;
    *)
        echo "Usage: $0 {setup|speed <0-255>|status}"
        exit 1
        ;;
esac
