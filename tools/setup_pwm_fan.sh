#!/usr/bin/env bash
set -euo pipefail

# Script to configure and control PWM0 fan (Pin 7) on Radxa ROCK 4C+ (RK3399)
# Designed to be executed directly on the device (or via docker/deployment scripts).

COMMAND="${1:-setup}"
ARG2="${2:-}"

OVERLAY_NAME="rockchip-rk3399-pwm0-fan"
DTS_PATH="/tmp/${OVERLAY_NAME}.dts"
DTBO_PATH="/tmp/${OVERLAY_NAME}.dtbo"
BACKUP_STAMP="$(date -u +%Y%m%dT%H%M%SZ)"

function check_sudo() {
    if [ "$EUID" -ne 0 ]; then
        SUDO="sudo"
    else
        SUDO=""
    fi
}

function require_dtc() {
    if ! command -v dtc &> /dev/null; then
        echo "Error: device tree compiler (dtc) is not installed."
        echo "Please install it with: $SUDO apt-get update && $SUDO apt-get install -y device-tree-compiler"
        exit 1
    fi
}

function generate_dts() {
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
				fan-stop-to-start-percent = <100>;
				fan-stop-to-start-us = <500000>;
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
				fan_stage1: fan_stage1 {
					temperature = <50000>;
					hysteresis = <5000>;
					type = "active";
				};
				fan_stage2: fan_stage2 {
					temperature = <60000>;
					hysteresis = <5000>;
					type = "active";
				};
				fan_stage3: fan_stage3 {
					temperature = <68000>;
					hysteresis = <5000>;
					type = "active";
				};
				fan_stage4: fan_stage4 {
					temperature = <75000>;
					hysteresis = <5000>;
					type = "active";
				};
			};

			cooling-maps {
				map-fan-stage1 {
					trip = <&fan_stage1>;
					cooling-device = <&fan0 1 1>;
				};
				map-fan-stage2 {
					trip = <&fan_stage2>;
					cooling-device = <&fan0 2 2>;
				};
				map-fan-stage3 {
					trip = <&fan_stage3>;
					cooling-device = <&fan0 3 3>;
				};
				map-fan-stage4 {
					trip = <&fan_stage4>;
					cooling-device = <&fan0 4 4>;
				};
			};
		};
	};
};
DTS_EOF
}

function overlay_is_configured() {
    [ -f /boot/armbianEnv.txt ] && awk -v overlay="$OVERLAY_NAME" '
        /^user_overlays=/ {
            sub(/^user_overlays=/, "")
            count = split($0, entries, /[[:space:]]+/)
            for (i = 1; i <= count; i++) if (entries[i] == overlay) found = 1
        }
        END { exit(found ? 0 : 1) }
    ' /boot/armbianEnv.txt
}

function setup_local() {
    echo "================================================="
    echo " Setting up PWM Fan Overlay on Board"
    echo "================================================="

    check_sudo
    require_dtc

    echo "1. Generating Device Tree Source..."
    generate_dts

    echo "2. Compiling DTBO..."
    dtc -@ -I dts -O dtb -o "$DTBO_PATH" "$DTS_PATH"

    if [ ! -f /boot/armbianEnv.txt ]; then
        echo "Error: /boot/armbianEnv.txt is required to activate the ROCK 4C+ fan overlay." >&2
        exit 1
    fi

    if cmp -s "$DTBO_PATH" "/boot/overlay-user/${OVERLAY_NAME}.dtbo" && overlay_is_configured; then
        echo "[✓] PWM fan overlay and thermal curve are already installed."
        return 0
    fi

    echo "3. Backing up existing overlay and boot configuration..."
    if [ -f "/boot/overlay-user/${OVERLAY_NAME}.dtbo" ]; then
        $SUDO cp -p "/boot/overlay-user/${OVERLAY_NAME}.dtbo" \
            "/boot/overlay-user/${OVERLAY_NAME}.dtbo.backup-${BACKUP_STAMP}"
    fi
    if [ -f /boot/armbianEnv.txt ]; then
        $SUDO cp -p /boot/armbianEnv.txt "/boot/armbianEnv.txt.backup-${BACKUP_STAMP}"
    fi

    echo "4. Installing overlay to /boot/overlay-user/..."
    $SUDO mkdir -p /boot/overlay-user
    $SUDO cp "$DTBO_PATH" "/boot/overlay-user/${OVERLAY_NAME}.dtbo"

    echo "5. Updating /boot/armbianEnv.txt..."
    if ! overlay_is_configured; then
        if grep -q '^user_overlays=' /boot/armbianEnv.txt; then
            $SUDO sed -i "/^user_overlays=/ s/$/ ${OVERLAY_NAME}/" /boot/armbianEnv.txt
        else
            echo "user_overlays=${OVERLAY_NAME}" | $SUDO tee -a /boot/armbianEnv.txt >/dev/null
        fi
    fi

    echo ""
    echo "================================================="
    echo " [✓] PWM Fan overlay configured successfully!"
    echo " Please reboot the board to activate: 'sudo reboot'"
    echo "================================================="
}

function check_overlay() {
    check_sudo
    require_dtc
    echo "Generating and compiling overlay without installing it..."
    generate_dts
    dtc -@ -I dts -O dtb -o "$DTBO_PATH" "$DTS_PATH"
    checked_dts="${DTS_PATH%.dts}.checked.dts"
    dtc -I dtb -O dts -o "$checked_dts" "$DTBO_PATH"

    for expected in \
        'fan-stop-to-start-percent = <0x64>;' \
        'fan-stop-to-start-us = <0x7a120>;' \
        'temperature = <0xc350>;' \
        'temperature = <0xea60>;' \
        'temperature = <0x109a0>;' \
        'temperature = <0x124f8>;' \
        'cooling-device = <0x02 0x01 0x01>;' \
        'cooling-device = <0x02 0x02 0x02>;' \
        'cooling-device = <0x02 0x03 0x03>;' \
        'cooling-device = <0x02 0x04 0x04>;'; do
        if ! grep -Fq "$expected" "$checked_dts"; then
            echo "Overlay validation failed: missing ${expected}"
            exit 1
        fi
    done
    echo "[✓] Overlay compiles and contains four fixed fan-state mappings."
}

function set_speed() {
    SPEED="${1:-255}"
    if ! [[ "$SPEED" =~ ^[0-9]+$ ]] || [ "$SPEED" -lt 0 ] || [ "$SPEED" -gt 255 ]; then
        echo "Error: Speed must be an integer between 0 and 255."
        exit 1
    fi

    check_sudo
    PWM_FILE=""
    for name_file in /sys/class/hwmon/hwmon*/name; do
        [ -f "$name_file" ] || continue
        if [ "$(cat "$name_file" 2>/dev/null)" = "pwmfan" ]; then
            PWM_FILE="${name_file%/name}/pwm1"
            break
        fi
    done
    if [ -n "$PWM_FILE" ]; then
        echo "Temporary manual override: setting fan speed to $SPEED ($PWM_FILE)..."
        echo "The kernel thermal governor may change this at the next thermal update."
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
    echo "=== CPU Thermal Policy and Trips ==="
    if [ -r /sys/class/thermal/thermal_zone0/policy ]; then
        echo "Policy: $(cat /sys/class/thermal/thermal_zone0/policy)"
        for trip_temp in /sys/class/thermal/thermal_zone0/trip_point_*_temp; do
            [ -f "$trip_temp" ] || continue
            trip_base="${trip_temp%_temp}"
            trip_id="${trip_base##*_}"
            echo "Trip ${trip_id}: $(cat "$trip_temp") m°C, hyst $(cat "${trip_base}_hyst" 2>/dev/null || echo N/A) m°C, type $(cat "${trip_base}_type" 2>/dev/null || echo unknown)"
        done
    else
        echo "CPU thermal zone is unavailable."
    fi

    echo ""
    echo "=== PWM Control Files ==="
    found_pwm=0
    for name_file in /sys/class/hwmon/hwmon*/name; do
        [ -f "$name_file" ] || continue
        if [ "$(cat "$name_file" 2>/dev/null)" = "pwmfan" ]; then
            pwm_dir="${name_file%/name}"
            echo "${pwm_dir}/pwm1=$(cat "${pwm_dir}/pwm1" 2>/dev/null || echo N/A)"
            echo "${pwm_dir}/pwm1_enable=$(cat "${pwm_dir}/pwm1_enable" 2>/dev/null || echo N/A)"
            found_pwm=1
        fi
    done
    [ "$found_pwm" -eq 1 ] || echo "No pwmfan hwmon device found."
}

case "$COMMAND" in
    setup)
        setup_local
        ;;
    check)
        check_overlay
        ;;
    speed|set-speed)
        set_speed "$ARG2"
        ;;
    status)
        get_status
        ;;
    *)
        echo "Usage: $0 {setup|speed <0-255>|status}"
        echo "  speed is a temporary diagnostic override; kernel thermal control remains authoritative."
        exit 1
        ;;
esac
