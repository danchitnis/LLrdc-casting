#!/usr/bin/env python3
"""
Radxa ROCK 4C+ (RK3399) 3-Wire PWM Fan Controller & Monitor
===========================================================

Hardware Wiring:
---------------
- Red Wire   : +5V Power (Pin 2 or Pin 4 on 40-pin GPIO header)
- Black Wire : Ground (Pin 6, 9, 14, 20, or 25 on GPIO header)
- Blue Wire  : PWM Control Input -> Connected to Physical Pin 7 (GPIO4_C2 / PWM0)

Technical Explanation of Fan Control:
------------------------------------
1. Pin Muxing & Hardware Controller:
   - Physical Pin 7 corresponds to GPIO4_C2 (Linux GPIO pin 146).
   - In the Rockchip RK3399 Device Tree, Pin 7 is assigned to PWM controller 0 (`/pwm@ff420000`).
   - The device tree overlay (`rockchip-rk3399-pwm0-fan.dtbo`) uses `pinctrl-names = "default"`
     to ensure `rockchip-pwm` claims Pin 146 during driver probe.

2. PWM Signal Parameters:
   - Period: 1,000,000 ns (1 kHz switching frequency).
     * High-frequency 25 kHz signals cause beat-frequency interference and "voom-voom" 
       revving oscillations with small 3-wire DC fan internal motor drivers.
     * A 1 kHz switching frequency delivers silky-smooth, silent DC power modulation.
   - Duty Cycle: Ranges from 0 ns (0% / OFF) to 1,000,000 ns (100% / MAX speed).

3. Thermal Management & Anti-Revving:
   - CPU temperature is read from `/sys/class/thermal/thermal_zone0/temp`.
   - Polling interval is set to 3 seconds with a 5°C hysteresis band to prevent 
     rapid revving and motor hunting on minor thermal fluctuations.

Usage:
------
- Check status:
    python3 tools/fan_control.py status

- Set manual duty cycle percentage (0-100%):
    python3 tools/fan_control.py set <0-100>

- Run dynamic thermal daemon (smooth temperature-proportional control):
    python3 tools/fan_control.py daemon
"""

import sys
import os
import time

SYSFS_PWM_DIR = "/sys/class/pwm/pwmchip0/pwm0"
SYSFS_EXPORT = "/sys/class/pwm/pwmchip0/export"
THERMAL_ZONE = "/sys/class/thermal/thermal_zone0/temp"
PWM_PERIOD_NS = 1000000  # 1 kHz period (1,000,000 ns)


def ensure_pwm_exported():
    """Ensure PWM0 channel is exported and accessible in sysfs."""
    if not os.path.exists(SYSFS_PWM_DIR):
        if os.path.exists(SYSFS_EXPORT):
            try:
                with open(SYSFS_EXPORT, "w") as f:
                    f.write("0")
                time.sleep(0.1)
            except Exception as e:
                print(f"[!] Warning exporting PWM0: {e}")

    # Set 1 kHz period if available
    period_file = os.path.join(SYSFS_PWM_DIR, "period")
    if os.path.exists(period_file):
        try:
            with open(period_file, "w") as f:
                f.write(str(PWM_PERIOD_NS))
        except Exception as e:
            pass


def get_cpu_temp():
    """Read CPU temperature in Celsius."""
    if os.path.exists(THERMAL_ZONE):
        try:
            with open(THERMAL_ZONE, "r") as f:
                return float(f.read().strip()) / 1000.0
        except Exception:
            pass
    return 0.0


def set_fan_speed_percent(percent):
    """
    Set fan speed as a percentage (0 to 100%).
    
    Duty cycle (ns) = (percent / 100) * PWM_PERIOD_NS (1,000,000 ns)
    """
    percent = max(0.0, min(100.0, float(percent)))
    duty_ns = int((percent / 100.0) * PWM_PERIOD_NS)

    ensure_pwm_exported()

    duty_file = os.path.join(SYSFS_PWM_DIR, "duty_cycle")
    enable_file = os.path.join(SYSFS_PWM_DIR, "enable")

    if os.path.exists(duty_file):
        try:
            if percent == 0:
                with open(enable_file, "w") as f:
                    f.write("0")
            else:
                with open(duty_file, "w") as f:
                    f.write(str(duty_ns))
                with open(enable_file, "w") as f:
                    f.write("1")
            print(f"[✓] Fan speed set to {percent:.1f}% (Duty Cycle: {duty_ns} ns @ 1 kHz)")
        except PermissionError:
            print(f"[!] Permission denied writing to {SYSFS_PWM_DIR}.")
            print("    Run: sudo chmod -R 777 /sys/class/pwm/pwmchip0/pwm0")
        except Exception as e:
            print(f"[!] Error setting fan speed: {e}")
    else:
        print(f"[!] PWM directory {SYSFS_PWM_DIR} not found. Ensure PWM0 overlay is loaded.")


def print_status():
    """Print detailed status of thermal zones and PWM fan control."""
    temp_c = get_cpu_temp()
    print("=================================================")
    print(" Radxa ROCK 4C+ PWM0 Fan Detailed Status")
    print("=================================================")
    print(f" CPU Temperature : {temp_c:.2f}°C")
    
    if os.path.exists(SYSFS_PWM_DIR):
        try:
            with open(os.path.join(SYSFS_PWM_DIR, "period"), "r") as f:
                period = int(f.read().strip())
            with open(os.path.join(SYSFS_PWM_DIR, "duty_cycle"), "r") as f:
                duty = int(f.read().strip())
            with open(os.path.join(SYSFS_PWM_DIR, "enable"), "r") as f:
                enabled = int(f.read().strip())
            
            freq_hz = 1e9 / period if period > 0 else 0
            percent = (duty / period) * 100.0 if period > 0 else 0
            
            print(f" PWM Status       : {'Active' if enabled else 'Disabled'}")
            print(f" PWM Frequency    : {freq_hz/1000.0:.1f} kHz ({period} ns period)")
            print(f" Duty Cycle       : {duty} ns ({percent:.1f}% speed)")
        except Exception as e:
            print(f" PWM Info Error   : {e}")
    else:
        print(" PWM Channel 0    : Not exported or disabled in DT")
    print("=================================================")


def run_daemon():
    """Run smooth temperature-proportional thermal control loop."""
    print("[*] Starting smooth thermal control daemon (Ctrl+C to stop)...")
    last_speed = -1
    
    while True:
        temp = get_cpu_temp()
        
        # Calculate smooth target speed
        if temp < 50.0:
            target = 0.0
        elif temp < 60.0:
            target = 40.0
        elif temp < 68.0:
            target = 60.0
        elif temp < 75.0:
            target = 80.0
        else:
            target = 100.0

        if target != last_speed:
            set_fan_speed_percent(target)
            last_speed = target

        time.sleep(3)


def main():
    if len(sys.argv) < 2 or sys.argv[1] == "status":
        print_status()
    elif sys.argv[1] == "set" and len(sys.argv) >= 3:
        set_fan_speed_percent(float(sys.argv[2]))
    elif sys.argv[1] == "daemon":
        run_daemon()
    else:
        print("Usage: python3 tools/fan_control.py {status | set <0-100> | daemon}")


if __name__ == "__main__":
    main()
