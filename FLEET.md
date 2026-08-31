# Independent ROCK 4C+ Fleet

Every initialized receiver normally runs the public ARM64 image from
`danchitnis/llrdc-casting:latest` and can boot, cast, and update without access
to this repository or any other receiver.

## Initialize from the Mac

The board must be a Debian ARM64 Radxa ROCK 4C+ with working SSH, sudo,
Internet access, and joined Tailscale. Docker is installed when missing.

Initialization installs the board helper utilities under
`/usr/local/lib/llrdc-tools`, with command links in `/usr/local/bin`. It does
not modify the boot configuration or fan control automatically. An operator
can install the included ROCK 4C+ kernel fan curve later with
`sudo setup_pwm_fan.sh setup`, then reboot the board.

```sh
./init_device.sh <tailscale-host-or-ip>
```

The command validates the board, asks for its unique name and whether cloud
discovery should be enabled, installs the shared device services, pulls the
image, and verifies stable health. Cloud setup requires the shared credentials
created by `./setup_cloudflare.sh`; only a derived per-device key is copied.
Rerunning initialization is safe and can add cloud access to a previously
local-only device.

To provision cloud discovery later while preserving the device's local
settings, certificates, and pairing state, run:

```sh
./init_device.sh --add-cloud <tailscale-host-or-ip>
```

Initialized devices contain `/etc/llrdc/role` with `independent`. They all have
the same updater and management capabilities.

`server.sh --start` may place a temporary Mac-built override on any selected
initialized board. That board remains independent and its update controls stay
enabled. Applying a Docker Hub update removes the temporary override and
returns the board to the published release image.

## Fully local bootstrap

For a device that will not use `cast.llrdc.com`, download and run the public
bootstrap directly on the board:

```sh
curl -fsSL https://raw.githubusercontent.com/danchitnis/LLrdc-casting/main/bootstrap_device.sh -o /tmp/bootstrap_device.sh
bash /tmp/bootstrap_device.sh
```

This installs the same device runtime with cloud discovery disabled. Local
pairing and casting continue to work without Cloudflare or the Mac.

## Updates

Open `https://<tailscale-ip>:9090/`, select **Check for update**, and then
**Update now**. Applying an update is disabled during an active cast. The
root-owned updater verifies ARM64, restarts the container, waits for stable
health, and restores the previous image automatically on failure. There is no
automatic update schedule. If a temporary Mac development build is active,
the published image is offered as an available update even when its underlying
runtime layers have not changed.

## Publish a release

From the development worktree, run:

```sh
./test_release.sh --board-ip=<development-board-ip>
./publish_docker_image.sh
```

The first command runs client and ARM64 tests plus codec, management, and
Cloudflare hardware suites. The separate publisher simply asks the developer
whether those tests passed and proceeds only after a **yes** answer. Clean
commits use a `sha-<commit>` immutable tag; developer worktrees use a
`dev-<commit>-<snapshot>` immutable tag. Both update `latest` only after ARM64
verification. Neither command invokes `sudo`.

## Privileged installation

Device installation changes systemd services and therefore requires root.
Run `./init_device.sh <tailscale-ip>` yourself from the Mac and enter the
board's sudo password directly at its terminal prompt. Do not provide sudo
passwords to automation or store them in repository configuration.
