# LLrdc Casting Setup and Operations

This guide installs and operates LLrdc Casting on the supported reference
receiver: a Radxa ROCK 4C+ with RK3399, Debian/Armbian ARM64, and an HDMI
display.

## Before you begin

The receiver needs:

- working HDMI output;
- Ethernet or Wi-Fi connectivity;
- SSH and `sudo` access during installation; and
- Internet access for the initial image download.

The casting computer needs Google Chrome and direct network reachability to the
receiver. Wired Ethernet is recommended. Tailscale is not part of the media
path, but the production installer requires it for the scoped management portal.

## Local-only installation

For one receiver that does not need `cast.llrdc.com`, run the public bootstrap
directly on the board:

```sh
curl -fsSL https://raw.githubusercontent.com/danchitnis/LLrdc-casting/main/bootstrap_device.sh -o /tmp/bootstrap_device.sh
bash /tmp/bootstrap_device.sh
```

The script installs Docker when needed, installs the production systemd
services, pulls `danchitnis/llrdc-casting:latest`, and starts with cloud
discovery disabled. The receiver remains able to cast locally without Internet
access after installation.

Confirm the services and receiver health:

```sh
systemctl status llrdc-casting.service llrdc-update.path
curl --insecure https://127.0.0.1:9090/health
```

## Guided installation from a Mac

For an independently managed receiver joined to Tailscale, run from this
repository:

```sh
./init_device.sh <tailscale-host-or-ip>
```

The initializer validates the ROCK 4C+, asks for its unique name and optional
cloud discovery, installs the shared device services and helper tools, and
checks stable health. It requests the device owner's `sudo` password directly
when system services must be changed; do not store that password in project
configuration.

To add cloud discovery later without replacing local settings, certificates,
or pairing state:

```sh
./init_device.sh --add-cloud <tailscale-host-or-ip>
```

See [Independent ROCK 4C+ Fleet](FLEET.md) for updates, development overrides,
and release-image behavior.

## Cast to the receiver

1. Read the receiver's private LAN address and rotating four-character code
   from the HDMI waiting screen.
2. Open `https://<receiver-lan-ip>:8080/` in Chrome.
3. Accept the warning for the receiver's self-signed local certificate.
4. Enter the code, choose the output settings, and select **Start Casting**.
5. Select the screen, window, or tab in Chrome's permission prompt.

The certificate is regenerated within the WebTransport validity limit and its
SHA-256 fingerprint is used to authenticate the direct connection. Video,
control, and telemetry remain between the browser and receiver.

When HDMI access is unavailable, a repository-based operator can retrieve the
current local code without logging it:

```sh
pairing_code="$(./server.sh --get-pairing-code --board-ip=<receiver-address>)"
```

Keep the value only in memory for the immediate pairing action.

## Optional public pairing page

`https://cast.llrdc.com` is an optional discovery service. The receiver
registers its private endpoint and short-lived pairing metadata; the browser
uses that response to connect directly over the LAN. Cloudflare does not carry
the casting UI after handoff, media, control traffic, or telemetry.

From a development checkout, configure the service interactively with:

```sh
./setup_cloudflare.sh
```

Useful read-only checks are:

```sh
./setup_cloudflare.sh --status
./setup_cloudflare.sh --verify
./setup_cloudflare.sh --status --json
```

The command is resumable and keeps private state in the ignored
`.cloudflare/` directory. Credential rotation is deliberate and uses
`./setup_cloudflare.sh --rotate-credentials`. See the
[pairing service guide](cloudflare/worker/README.md) for infrastructure details.

## Management portal

Independent receivers expose the portal only on the configured Tailscale
address:

```text
https://<tailscale-receiver-ip>:9090/
```

It provides:

- live stream throughput and synchronized latency diagnostics;
- connected clients and process-lifetime sharing history;
- watchdog state, health, restart, and recovery information;
- filtered operational events and a redacted diagnostic ZIP;
- receiver settings that persist across reboots; and
- manual, health-checked updates with automatic rollback.

The portal has no separate application password because it must bind to the
configured Tailscale interface and never falls back to a wildcard address.
Cloud credentials and private keys are not returned by its APIs.

Applying settings restarts only the supervised casting receiver. The manager,
portal, and WebSocket stay available. A later `server.sh --start` development
deployment resolves `config.yaml` and command-line flags again and replaces
portal-edited values.

## Device helper tools

The guided initializer installs the maintenance utilities listed in
`device/helper-tools.manifest` under `/usr/local/lib/llrdc-tools/` and creates
command links in `/usr/local/bin/`. A development deployment with
`./server.sh --start` does not install this helper bundle.

Installed tools include:

- `fan_control.py`, `setup_pwm_fan.sh`, and `test_fan_curve.sh` for the ROCK 4C+
  PWM fan and thermal policy;
- `net_monitor.py`, `scan_wifi.sh`, and `connect_wifi.sh` for network setup and
  diagnostics;
- `activate_eduroam.sh` for Eduroam activation; and
- `setup_mgmt_port.sh` for the optional USB management Ethernet port.

Fan configuration is deliberately opt-in. To install the kernel-managed curve
and reboot:

```sh
sudo setup_pwm_fan.sh setup
sudo reboot
```

After the board returns:

```sh
fan_control.py status
setup_pwm_fan.sh status
```

Manual fan-speed overrides are temporary diagnostics. The kernel thermal
governor remains authoritative during normal operation.

## Updates and recovery

In the management portal, select **Check for update** and then **Update now**.
Updates are disabled during an active cast. The updater verifies ARM64,
restarts the container, waits for stable health, and restores the previous
image on failure. There is no automatic update schedule.

For a receiver that fails before Linux starts when HDMI is attached, follow
[ROCK 4C+ HDMI Boot Fix](UBOOT_HDMI_BOOT_FIX.md). For stream geometry and
latency interpretation, see the [resolution model](ASPECT_RATIO_AND_RESOLUTION_SPEC.md)
and [latency guide](LATENCY_AND_CONGESTION.md).

## Security notes

- Keep local pairing enabled unless every client on the reachable network is
  trusted.
- Do not publish receiver private addresses, pairing codes, tokens, or
  diagnostic archives.
- The receiver listeners accept traffic on available receiver interfaces; the
  HDMI screen deliberately displays only usable private physical-network
  addresses.
- The durable journal is root-owned, redacted, and rotated. Normal logs exclude
  packet payloads, codec headers, and per-frame diagnostics.
