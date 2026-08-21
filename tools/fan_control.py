#!/usr/bin/env python3
"""
Radxa ROCK 4C+ (RK3399) kernel-managed PWM fan monitor
========================================================

The fan is controlled by the pwm-fan driver and the CPU thermal governor.
This tool reports that live state and provides a temporary manual override
for diagnostics; it deliberately does not run a competing thermal daemon.

Hardware wiring:
  Red wire   -> +5V (physical pin 2 or 4)
  Black wire -> Ground (physical pin 6, 9, 14, 20, or 25)
  Blue wire  -> PWM0 / physical pin 7 (GPIO4_C2)

Usage:
  python3 tools/fan_control.py status
  python3 tools/fan_control.py set <0-100>
  python3 tools/fan_control.py daemon  # intentionally disabled
"""

import glob
import os
import sys


THERMAL_ZONE = "/sys/class/thermal/thermal_zone0/temp"
THERMAL_ZONE_DIR = "/sys/class/thermal/thermal_zone0"
HWMON_ROOT = "/sys/class/hwmon"
PWM_MAX = 255

# Rising thresholds and the exact cooling state selected at each trip.
CURVE = (
    (50.0, 1, 100),
    (60.0, 2, 150),
    (68.0, 3, 200),
    (75.0, 4, 255),
)


def read_text(path, default="N/A"):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return handle.read().strip()
    except (OSError, ValueError):
        return default


def find_pwmfan_dir():
    """Find the hwmon directory owned by the kernel pwm-fan driver."""
    for name_file in sorted(glob.glob(os.path.join(HWMON_ROOT, "hwmon*", "name"))):
        if read_text(name_file) == "pwmfan":
            return os.path.dirname(name_file)
    return None


def find_pwmfan_cooling_device():
    """Find the thermal cooling device whose type is pwm-fan."""
    for type_file in sorted(glob.glob("/sys/class/thermal/cooling_device*/type")):
        if read_text(type_file) == "pwm-fan":
            return os.path.dirname(type_file)
    return None


def get_cpu_temp():
    """Read CPU temperature in Celsius."""
    try:
        return float(read_text(THERMAL_ZONE, "0")) / 1000.0
    except ValueError:
        return 0.0


def set_fan_speed_percent(percent):
    """Set a temporary diagnostic PWM override through the live hwmon driver."""
    percent = max(0.0, min(100.0, float(percent)))
    pwm_dir = find_pwmfan_dir()
    if not pwm_dir:
        print("[!] Kernel pwmfan hwmon device not found. Ensure the overlay is loaded.")
        return 1

    pwm_value = round((percent / 100.0) * PWM_MAX)
    try:
        with open(os.path.join(pwm_dir, "pwm1"), "w", encoding="utf-8") as handle:
            handle.write(str(pwm_value))
    except PermissionError:
        print(f"[!] Permission denied writing to {pwm_dir}/pwm1.")
        return 1
    except OSError as error:
        print(f"[!] Error setting fan speed: {error}")
        return 1

    print(
        f"[!] Temporary manual override: {percent:.1f}% "
        f"({pwm_value}/{PWM_MAX}). Kernel control may reassert it."
    )
    return 0


def print_curve():
    print(" Target Fan Curve  : kernel step_wise, fixed cooling stages")
    for temperature, state, pwm in CURVE:
        percentage = pwm / PWM_MAX * 100.0
        print(f"   >= {temperature:.0f}°C      : state {state}, PWM {pwm}/255 ({percentage:.1f}%)")
    print("   Hysteresis       : 5°C per stage; fan startup boost 100% for 500 ms")


def print_status():
    """Print the active kernel thermal curve and live fan state."""
    print("=================================================")
    print(" Radxa ROCK 4C+ PWM0 Fan Detailed Status")
    print("=================================================")
    print(f" CPU Temperature : {get_cpu_temp():.2f}°C")
    print(f" Thermal Policy  : {read_text(os.path.join(THERMAL_ZONE_DIR, 'policy'))}")
    print_curve()

    print(" Thermal Trips    :")
    for temp_file in sorted(glob.glob(os.path.join(THERMAL_ZONE_DIR, "trip_point_*_temp"))):
        trip_base = temp_file[:-len("_temp")]
        trip_id = trip_base.rsplit("_", 1)[-1]
        print(
            f"   {trip_id}: {read_text(temp_file)} m°C, "
            f"hyst {read_text(f'{trip_base}_hyst')} m°C, "
            f"type {read_text(f'{trip_base}_type')}"
        )

    cooling_dir = find_pwmfan_cooling_device()
    if cooling_dir:
        state = read_text(os.path.join(cooling_dir, "cur_state"))
        maximum = read_text(os.path.join(cooling_dir, "max_state"))
        print(f" Cooling Device  : {cooling_dir} state {state}/{maximum}")
    else:
        print(" Cooling Device  : pwm-fan not found")

    pwm_dir = find_pwmfan_dir()
    if pwm_dir:
        print(f" PWM Interface   : {pwm_dir}/pwm1={read_text(os.path.join(pwm_dir, 'pwm1'))}")
        print(f" PWM Enable      : {read_text(os.path.join(pwm_dir, 'pwm1_enable'))}")
    else:
        print(" PWM Interface   : pwmfan hwmon device not found")
    print("=================================================")


def run_daemon():
    """Reject the old competing userspace thermal controller."""
    print("[!] fan_control.py daemon is disabled: kernel thermal control owns PWM0.")
    print("    Use 'status' to inspect the active curve or 'set' for a temporary diagnostic override.")
    return 2


def main():
    if len(sys.argv) < 2 or sys.argv[1] == "status":
        print_status()
        return 0

    if sys.argv[1] == "set" and len(sys.argv) >= 3:
        try:
            return set_fan_speed_percent(float(sys.argv[2]))
        except ValueError:
            print("Usage: python3 tools/fan_control.py set <0-100>")
            return 2

    if sys.argv[1] == "daemon":
        return run_daemon()

    print("Usage: python3 tools/fan_control.py {status | set <0-100> | daemon}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
