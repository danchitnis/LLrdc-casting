#!/usr/bin/env bash
set -euo pipefail

# Deterministic test for the kernel-managed RK3399 fan curve.
# Run on the board as root, for example:
#   sudo /home/danial/tools/test_fan_curve.sh
#
# The test uses thermal-zone emulation only; it never drives the CPU to the
# test temperatures. A trap always writes 0 to emul_temp to restore the real
# sensor before exiting.

THERMAL_ZONE="${THERMAL_ZONE:-/sys/class/thermal/thermal_zone0}"
EMUL_TEMP="${EMUL_TEMP:-${THERMAL_ZONE}/emul_temp}"
SETTLE_SECONDS="${SETTLE_SECONDS:-4}"
HOLD_SAMPLES="${HOLD_SAMPLES:-3}"
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-1}"

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this test as root: sudo $0" >&2
    exit 2
fi

if [ ! -w "$EMUL_TEMP" ]; then
    echo "Thermal emulation is unavailable or not writable: $EMUL_TEMP" >&2
    exit 2
fi

fan_cdev=""
for type_file in /sys/class/thermal/cooling_device*/type; do
    [ -f "$type_file" ] || continue
    if [ "$(cat "$type_file")" = "pwm-fan" ]; then
        fan_cdev="${type_file%/type}"
        break
    fi
done

if [ -z "$fan_cdev" ]; then
    echo "pwm-fan cooling device not found." >&2
    exit 2
fi

pwm_file=""
for name_file in /sys/class/hwmon/hwmon*/name; do
    [ -f "$name_file" ] || continue
    if [ "$(cat "$name_file")" = "pwmfan" ]; then
        pwm_file="${name_file%/name}/pwm1"
        break
    fi
done

if [ -z "$pwm_file" ] || [ ! -r "$pwm_file" ]; then
    echo "pwmfan hwmon PWM interface not found." >&2
    exit 2
fi

cleanup() {
    # In the thermal framework, emul_temp=0 disables emulation.
    printf '0\n' > "$EMUL_TEMP" 2>/dev/null || true
}

trap cleanup EXIT
trap 'exit 130' INT TERM

expected_pwm() {
    case "$1" in
        0) printf '0' ;;
        1) printf '100' ;;
        2) printf '150' ;;
        3) printf '200' ;;
        4) printf '255' ;;
        *) return 1 ;;
    esac
}

set_temp() {
    local temp_millicelsius="$1"
    printf '%s\n' "$temp_millicelsius" > "$EMUL_TEMP"
    sleep "$SETTLE_SECONDS"
}

printf '%s\n' "RK3399 kernel fan-curve emulation test"
printf 'Cooling device: %s\n' "$fan_cdev"
printf 'PWM interface: %s\n' "$pwm_file"
printf 'Settle/hold: %ss + %sx %ss samples\n' "$SETTLE_SECONDS" "$HOLD_SAMPLES" "$SAMPLE_INTERVAL"
printf '%s\n' ""

# Temperature in °C, expected cooling state. The descending values exercise
# the 5°C hysteresis thresholds (70, 63, 55, and 45°C).
tests=(
    "44000 0"
    "50000 1"
    "59000 1"
    "60000 2"
    "67000 2"
    "68000 3"
    "74000 3"
    "75000 4"
    "69000 3"
    "62000 2"
    "54000 1"
    "44000 0"
)

failures=0
for test_case in "${tests[@]}"; do
    read -r temp_millicelsius expected_state <<< "$test_case"
    expected_pwm_value="$(expected_pwm "$expected_state")"
    set_temp "$temp_millicelsius"

    sample=1
    point_ok=1
    while [ "$sample" -le "$HOLD_SAMPLES" ]; do
        actual_temp="$(cat "$THERMAL_ZONE/temp")"
        actual_state="$(cat "$fan_cdev/cur_state")"
        actual_pwm="$(cat "$pwm_file")"
        printf '%s°C: temp=%s state=%s/%s pwm=%s/%s\n' \
            "$((temp_millicelsius / 1000))" "$actual_temp" \
            "$actual_state" "$expected_state" "$actual_pwm" "$expected_pwm_value"
        if [ "$actual_state" != "$expected_state" ] || [ "$actual_pwm" != "$expected_pwm_value" ]; then
            point_ok=0
        fi
        sample=$((sample + 1))
        [ "$sample" -le "$HOLD_SAMPLES" ] && sleep "$SAMPLE_INTERVAL"
    done

    if [ "$point_ok" -eq 1 ]; then
        echo "  PASS"
    else
        echo "  FAIL"
        failures=$((failures + 1))
    fi
done

if [ "$failures" -eq 0 ]; then
    echo ""
    echo "PASS: all fan stages and hysteresis transitions matched."
    exit 0
fi

echo ""
echo "FAIL: ${failures} temperature points did not match."
exit 1
