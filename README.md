# LLrdc Casting

LLrdc Casting is an open-source, low-latency way to cast a browser screen,
window, or tab directly to an HDMI display through a Radxa ROCK 4C+ receiver.
The sender needs no native application or browser extension, and video stays on
the local network.

```text
Browser capture + WebCodecs
            │
            │ authenticated WebTransport over the LAN
            ▼
ROCK 4C+ / RK3399 ── V4L2 hardware decode ── DRM/KMS ── HDMI

Optional: browser ── pairing code only ── Cloudflare discovery
          (Cloudflare never carries video, control, or telemetry)
```

## Why LLrdc Casting is different

LLrdc Casting joins browser-native capture and encoding to the RK3399's stateless video
decoder and direct display pipeline. Frames do not pass through a hosted relay,
a receiver desktop, or a software video player. Bounded queues, raw-input frame
dropping, synchronized latency telemetry, and adaptive bitrate keep the stream
responsive when the sender, network, or receiver comes under pressure.

## Features

- Cast a screen, window, or browser tab without installing a sender app.
- Send authenticated HEVC/H.265 or H.264 directly over LAN WebTransport.
- Decode in RK3399 hardware and present directly through DRM/KMS to HDMI.
- Select 720p, 1080p, 1440p, or 4K UHD and 30 or 60 FPS where supported.
- Preserve the source proportions or fill the connected display.
- See live output, encoder, frame, and estimated latency information.
- Pair locally with a rotating four-character code, even without Internet.
- Optionally discover a receiver through `cast.llrdc.com` without relaying media.
- Operate independent receivers with health monitoring, logs, updates, and
  rollback from a Tailscale-scoped management portal.

## Reference performance

The reference benchmark reports synchronized encoder-input-to-display latency
for HEVC 1080p30 in ultra-low-latency mode on a ROCK 4C+ over wired LAN. It
waits five seconds for startup, then averages every unsmoothed sample from the
next ten seconds. The 2026-09-01 reference run averaged **68.2 ms** from 10
synchronized samples, with no access-unit sequence gaps.

This is an instrumented pipeline estimate, not an external glass-to-glass or
camera-to-photon measurement. See [Performance](PERFORMANCE.md) for the exact
method, environment, results, and limitations.

## Requirements and compatibility

### Receiver

- Radxa ROCK 4C+ with RK3399, an HDMI display, and Debian/Armbian ARM64
- Docker, network access, and an active Tailscale connection during production
  installation
- Wired Ethernet recommended for the reference low-latency experience

The implementation depends on the Rockchip V4L2 stateless decoder and DRM/KMS
display stack. Other ARM64 boards are not currently a supported promise.

### Sender

- A computer that can reach the receiver on the same LAN
- Google Chrome with WebCodecs and WebTransport support

Chrome is the primary validated browser. Installed Safari is regression-tested
with HEVC and H.264 at 1080p30. H.264 output is limited to 1080p by the RK3399
decoder guardrails; HEVC provides the higher-resolution modes. Available
hardware encoding still depends on the sender browser and GPU.

## Install a local receiver

For a local-only receiver, run the public bootstrap on the ROCK 4C+:

```sh
curl -fsSL https://raw.githubusercontent.com/danchitnis/LLrdc-casting/main/bootstrap_device.sh -o /tmp/bootstrap_device.sh
bash /tmp/bootstrap_device.sh
```

The bootstrap installs the public ARM64 image and Tailscale-scoped management
services with cloud discovery disabled. Tailscale is not in the casting media
path. For guided Mac initialization and multi-device updates, follow
[Fleet setup](FLEET.md).

## Start casting

1. Open `https://<receiver-lan-ip>:8080/` in Chrome.
2. Accept the local certificate warning. The receiver uses a short-lived,
   self-signed certificate whose SHA-256 fingerprint authenticates WebTransport.
3. Enter the rotating code shown on the HDMI waiting screen.
4. Choose the output settings and select **Start Casting**.
5. Pick a screen, window, or tab in the browser permission prompt.

Select **Stop Casting** to return the receiver to its waiting screen.

The optional `https://cast.llrdc.com` entry point replaces steps 1–3 with a
public pairing page. It returns a private LAN endpoint and short-lived
authorization token, then the browser connects directly to the receiver.

## Important operational notes

- Direct casting and local pairing work without Cloudflare or Internet access.
- The sender must still be able to reach the receiver's private address.
- Pairing codes rotate and are rate-limited. Disabling the local code allows
  any network-reachable client to connect and should be a deliberate choice.
- The management portal is available at
  `https://<tailscale-receiver-ip>:9090/`; it does not bind to every interface.
- Management settings persist across reboots. A later development deployment
  with `server.sh --start` replaces them with repository configuration.
- Updates are manual, blocked during active casting, and roll back if the new
  receiver does not become healthy.

## Documentation

- [Setup and operations](SETUP.md)
- [Performance and benchmark method](PERFORMANCE.md)
- [Development and testing](DEVELOPMENT.md)
- [Independent receiver fleets](FLEET.md)
- [Latency and congestion control](LATENCY_AND_CONGESTION.md)
- [Aspect ratio and resolution model](ASPECT_RATIO_AND_RESOLUTION_SPEC.md)
- [ROCK 4C+ HDMI boot troubleshooting](UBOOT_HDMI_BOOT_FIX.md)
- [Optional Cloudflare pairing service](cloudflare/worker/README.md)
- [Archived implementation plans](docs/archive/)

## License

LLrdc Casting is licensed under the [Apache License 2.0](LICENSE).
