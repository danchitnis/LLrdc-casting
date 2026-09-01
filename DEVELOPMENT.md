# LLrdc Casting Development and Testing

This guide covers development deployments, deterministic tests, management
interfaces, and release publication. End-user installation and operation are in
[Setup and Operations](SETUP.md).

## Development model

The workstation owns source, tests, and the ARM64 cross-build. The ROCK 4C+
runs the resulting binaries in a privileged host-networked container with
`/dev` mounted for V4L2 and DRM/KMS access. Do not build Rust on the board.

The container runs two Rust executables:

- `llrdc-management` is PID 1 and owns configuration, certificates, the
  management portal, durable journal, update requests, and watchdog.
- `llrdc-casting` is the supervised media receiver and communicates with the
  manager through a versioned Unix-socket protocol.

The casting and management pages are strict-TypeScript Astro applications. The
container build produces each as a self-contained HTML file and embeds it in
the corresponding Rust binary.

## Workstation requirements

- Docker with ARM64 build support
- SSH and `scp` access to the ROCK 4C+
- Node.js/npm for frontend checks
- Installed Google Chrome for the codec, cloud, and management browser suites
- Installed Safari plus **Develop → Allow Remote Automation** for the optional
  Safari regression

The host does not need a Rust toolchain. Rust tests run inside the Docker test
stage.

## Select the board

Set the development target in `config.yaml`:

```yaml
board:
  ip: "<receiver-address>"
```

Every deployment command also accepts `--board-ip=<receiver-address>`, which
takes precedence. Use the private LAN address for performance work; a Tailscale
address is suitable for administration and remote deployment.

## Build, test, deploy, and stop

Run the Rust tests without changing the receiver:

```sh
./server.sh --test
```

Build locally, transfer the verified ARM64 artifacts, and start a development
override on the receiver:

```sh
./server.sh --start --board-ip=<receiver-address>
```

The deployment tests before replacing the receiver. An initialized independent
device keeps its updater and can return to the published image through the
management portal.

Inspect all configuration flags with:

```sh
./server.sh --help
```

Example:

```sh
./server.sh --start \
  --board-ip=<receiver-address> \
  --http-port=8080 \
  --webtransport-port=4433 \
  --board-port=4434 \
  --cloud=false
```

Stop an unmanaged development receiver with:

```sh
./server.sh --stop --board-ip=<receiver-address>
```

The wrapper refuses to stop an independently supervised production receiver;
use its systemd service or management workflow deliberately.

## Frontend checks

Install the locked dependencies once:

```sh
PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm --prefix client ci
```

Run strict Astro and TypeScript checks without producing a local build:

```sh
npm --prefix client run check
```

Run the deterministic frontend modules:

```sh
npm --prefix client run test:compositor
npm --prefix client run test:guardrails
npm --prefix client run test:latency
npm --prefix client run test:congestion
npm --prefix client run test:synthetic
```

Do not use a local `npm run build` as production verification. `server.sh`
builds both self-contained pages in the container's `html-builder` stage.

## RK3399 browser suites

The local codec suite deploys with cloud discovery disabled, retrieves the live
pairing code without printing it, and uses installed Chrome with synthetic
input:

```sh
./test_browser.sh codec chrome --board-ip=<receiver-private-lan-ip>
```

It exercises three HEVC 1080p cycles, three hardware-preferred H.264 cycles,
three software-requested H.264 cycles, and one HEVC 4K boundary cycle. It also
writes the reference latency benchmark to `performance-summary.json` after the
sustained first HEVC cycle.

Safari is a separate pass that reuses the cloud-disabled deployment:

```sh
./test_browser.sh codec safari --board-ip=<receiver-private-lan-ip>
```

The management suite backs up configuration, runtime secrets, and journal
history, tests settings and watchdog recovery, then restores and verifies the
backups:

```sh
./test_browser.sh management --board-ip=<receiver-address>
```

Run the cloud suite only for Worker, registration, pairing-token, bootstrap, or
cloud-to-LAN handoff changes:

```sh
./test_browser.sh cloud --board-ip=<receiver-address>
```

Artifacts are redacted and stored in `.artefact/`. A hardware suite passes only
when the command exits zero and its run directory contains deployment output,
receiver logs, and browser diagnostics.

## Direct streamer smoke test

The non-browser HEVC smoke client remains available for focused decoder checks:

```sh
./test.sh --1080p --fps 60 --duration 20
```

Use the browser suites as the release evidence for WebCodecs, WebTransport,
pairing, telemetry, and UI behavior.

## Management interfaces

The portal binds to `server.admin_bind_address`, normally the receiver's
Tailscale address, on port `9090`.

- `GET /health/manager` succeeds when the manager is live.
- `GET /health` succeeds only when manager and receiver are ready.
- `GET /api/snapshot` returns the current redacted operational snapshot.
- `POST /api/watchdog/restart` requests a confirmed receiver restart.
- `/api/logs` endpoints provide filtered operational events and diagnostic ZIPs.

Receiver settings are written atomically to
`/var/lib/llrdc-config/config.yaml`. Deployment-owned cloud secrets remain
separate and root-only. The journal under `/var/lib/llrdc-management` rotates
across four 8 MiB segments and excludes packet contents and per-frame codec
diagnostics in production.

## Cloudflare development

The complete interactive setup is:

```sh
./setup_cloudflare.sh
```

The script creates or reconciles D1, applies migrations, installs the public
pairing key, uploads secrets, deploys the Worker, and verifies registration and
the public bootstrap. It is resumable and keeps private state under the ignored
`.cloudflare/` directory.

Worker-only checks run from `cloudflare/worker`:

```sh
npm run check
npm run db:migrate:local
npm run dev
```

Do not deploy the Worker for receiver-only or local UI changes. See the
[pairing Worker guide](cloudflare/worker/README.md) for API and token formats.

## Release workflow

Run every production gate:

```sh
./test_release.sh --board-ip=<development-board-address>
```

This runs client checks, Docker-based ARM64 Rust tests, the Chrome codec suite,
the management suite, and the cloud suite. It does not publish and never runs
`sudo`.

After reviewing the results:

```sh
./publish_docker_image.sh
```

The publisher requires a direct confirmation that tests passed. Clean commits
receive a `sha-<commit>` immutable tag; uncommitted worktrees receive a
content-fingerprinted `dev-<commit>-<snapshot>` tag. `latest` moves only after
the ARM64 image is verified.
