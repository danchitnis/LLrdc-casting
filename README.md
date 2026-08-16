# LLrdc Casting

LLrdc Casting turns a compatible ARM board into a low-latency casting
receiver. Share a screen, window, or browser tab from Chrome and show it on a
connected HDMI display without sending the content through a cloud service.

## Features

- **Browser-to-display casting:** Start casting directly from Chrome without a
  separate capture application or desktop client.
- **Very low latency:** Designed for presentations, demonstrations, remote
  control, interactive content, and other activities where immediate feedback
  matters.
- **Local and private:** Screen content stays on the local network between the
  sender computer and the receiver.
- **Display-aware output:** The receiver reports the connected display and lets
  you preserve the source aspect ratio or fill the display.
- **Flexible quality:** Choose 720p, 1080p, 1440p, or 4K UHD, with 30 or 60 FPS
  and selectable quality and latency preferences.
- **Hardware-assisted playback:** The receiver is designed to use the board's
  video capabilities so the sharing computer does not have to do all the
  playback work.
- **Idle status screen:** See the receiver's network and display status while
  no content is being shared.

## Requirements

### Receiver board

- A Radxa ROCK 4C+ / RK3399 or compatible ARM64 board with an HDMI output
- Linux or Armbian installed and reachable over SSH
- Docker installed and running
- An HDMI display connected to the board

### Sender computer

- Docker, SSH, and `scp` for deployment
- Access to this project directory
- Google Chrome for casting
- A network connection to the receiver

The sender computer and receiver must be able to communicate over the local
network.

The HTTP, WebTransport, and direct UDP receiver listeners bind to `0.0.0.0`,
so they accept connections through any address available on the receiver. The
idle IP screen intentionally displays only usable private IPv4 addresses on
physical Ethernet or Wi-Fi interfaces; loopback, Docker, Tailscale, link-local,
and other virtual addresses are filtered out. Cloud registration prefers
Ethernet and falls back to Wi-Fi when Ethernet is unavailable.

## Set Up the Board

These steps are required once for a new board.

1. Connect the board to the network and HDMI display. Note its IP address.
2. Confirm SSH access and Docker:

   ```bash
   ssh <user>@<receiver-ip> "uname -m && docker info"
   ```

   The board should report an ARM64 architecture and Docker should return its
   system information.

3. If Docker is not installed, install it on the board:

   ```bash
   ssh -t <user>@<receiver-ip> "sudo apt update && sudo apt install -y docker.io docker-cli && sudo systemctl enable --now docker"
   ```

   Log in again after installation if your user was added to the `docker`
   group, then verify with `docker info`.

4. On the sender computer, set the receiver address in `config.yaml`:

   ```yaml
   board:
     ip: "<receiver-ip>"
   ```

   You can instead provide `--board-ip=<receiver-ip>` when starting the
   server.

## Build and Start the Server

Run this from the project directory on the sender computer:

```bash
./server.sh --start
```

Run all Rust unit tests locally without connecting to or changing the receiver:

```bash
./server.sh --test
```

## Browser Hardware Regression Suites

Install the client test dependencies once, including Playwright's test runner:

```bash
PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm --prefix client ci
```

The local codec suite deploys with Cloudflare discovery disabled and pairs
directly with the receiver. Chrome is the default (and only) browser for the
plain command; use the explicit Safari form after Chrome when you want the
installed Safari WebDriver pass. Safari never uses Playwright WebKit and reuses
the existing cloud-disabled deployment without redeploying:

```bash
./test_browser.sh codec          # branded Chrome (default)
./test_browser.sh codec chrome   # same as above
./test_browser.sh codec safari   # installed Safari, H.265 + H.264 at 1080p
```

The cloud suite is intentionally separate. It deploys with discovery enabled,
waits for receiver registration, obtains an unexpired code from D1, pairs
through `cast.llrdc.com`, and runs one HEVC handoff cycle:

```bash
./test_browser.sh cloud
```

Run both suites in sequence with `./test_browser.sh all`. A board address can
be supplied explicitly with `--board-ip=<address>`; otherwise the `board.ip`
value in `config.yaml` is used. The suites require SSH/Docker access to the
RK3399, a working HDMI mode, installed branded Chrome with the required codec
support. The Safari form additionally requires Safari **Develop → Allow Remote
Automation** enabled. The cloud suite additionally requires authenticated
Wrangler/D1 access. Failure artifacts are written below the repository-level
`.artefact/` directory and are ignored by git. Each invocation cleans that
directory first; browser-specific codec runs write their own timestamped run
directory, and `all` keeps Chrome codec and cloud results separate.

The deployment build runs the same tests before compiling and transferring the
release binary. A test failure stops the deployment before the receiver is
stopped or replaced.

The command builds the receiver software on the sender computer, transfers
what is needed to the board, and starts the receiver. The HDMI display should
show the LLrdc waiting screen when the receiver is ready.

All server deployment settings can be provided as flags. See the complete
list with:

```bash
./server.sh --help
```

For example:

```bash
./server.sh --start \
  --board-ip=<receiver-ip> \
  --http-port=8080 \
  --webtransport-port=4433 \
  --board-port=4434 \
  --cloud=false
```

The receiver also exposes a Tailscale-only management portal. It binds to
`server.admin_bind_address` (or `--admin-bind-address`) and defaults to port
`9090`; it never falls back to a wildcard address. Open:

```text
https://<tailscale-receiver-ip>:9090/
```

The portal shows live measured stream traffic, connected devices, process-life
sharing history, receiver health, and structured events. **Stop sharing** sends
an isolated admin command through the application boundary and restores the
idle dashboard. The portal has no separate password because access is limited
to the configured Tailscale interface.

To use an address without editing `config.yaml`:

```bash
./server.sh --start --board-ip=<receiver-ip>
```

To stop the receiver:

```bash
./server.sh --stop --board-ip=<receiver-ip>
```

## Start Casting

To configure the optional `cast.llrdc.com` discovery service interactively
after logging in with Wrangler:

```bash
./setup_cloudflare.sh
```

The script creates the D1 database, applies migrations, generates and uploads
the required credentials, deploys the Worker/UI, and restarts the receiver.
It is safe to rerun; generated private state stays in `.cloudflare/`.
SSH access to the receiver must already be configured and working. The script
does not install SSH or change receiver SSH settings. If needed, it uses your
existing `sudo` access only to install the public pairing key.

1. For offline/LAN-only casting, open the receiver's current IP in Chrome:

   ```text
   https://<receiver-ip>:8080/
   ```

2. Accept the receiver's local certificate warning, as in the previous workflow.
3. Retrieve the live pairing code over SSH when HDMI access is unavailable:

   ```bash
   pairing_code="$(./server.sh --get-pairing-code)"
   ```

   Enter that value in the browser. This works without Cloudflare. The HDMI
   idle screen is the manual fallback.
4. Select the source and picture settings you want to use.
5. Select **Start Casting**.
6. Choose the screen, window, or browser tab to share, then approve the
   browser permission prompt.
7. Confirm that the shared content appears on the HDMI display.

This direct-IP workflow works without Internet access or Cloudflare. The
optional `https://cast.llrdc.com` workflow serves a minimal pairing page. It
uses the Worker only for pairing discovery, then loads the full casting UI from
the receiver over the authenticated LAN WebTransport connection; WebTransport
video and control traffic still goes directly over the LAN. Enable discovery
for a deployment with `./server.sh --start --cloud=true`. The Worker
credentials described in
[`cloudflare/worker/README.md`](cloudflare/worker/README.md) are required only
when that optional workflow is wanted.

For deliberate local stress testing, use a fixed code for one deployment:

```bash
./server.sh --start --cloud=false --pairing-code=0000
```

Random rotating pairing codes remain the default. Fixed-code mode is rejected
when Cloudflare discovery is enabled and should not be persisted in configuration.

Select **Stop Casting** when finished. The receiver returns to its
waiting screen after the stream becomes inactive.

## Available Controls

Before starting a stream, you can choose:

- Screen capture or the built-in test pattern
- Output resolution from 720p through 4K UHD
- Preserved aspect ratio or stretched output
- 30 or 60 frames per second
- Video quality and bandwidth preference
- Ultra-low-latency, balanced, or quality-focused encoding

Settings are locked while casting is active. Stop casting before changing
them.

## License

This project is distributed under the terms in [LICENSE](LICENSE).
