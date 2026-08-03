# Radxa ROCK 4C+ HEVC HDMI Playback Smoke Test

This project is a low-latency video-playback smoke test for a **Radxa ROCK 4C+ (RK3399)**. It builds an ARM64 container locally, sends it to the board, receives Annex-B HEVC over UDP, uses the RK3399 `rkvdec` stateless V4L2 decoder, and presents video through DRM/KMS on HDMI.

Desktop sharing is not in scope yet.

## Normal workflow

Start the receiver from the workstation:

```bash
./server.sh --start
```

The idle HDMI screen shows:

- `LLrdc Casting // DEVICE IPS`
- Board IPv4 addresses
- Detected HDMI output mode, for example `3840X2160 @ 60 HZ`

The first incoming HEVC frame replaces the dashboard with video playback.

Run the default 4K60 HEVC smoke test:

```bash
./test.sh
```

To stream manually (without the test wrapper):

```bash
./stream.sh --4k --fps 60 --duration 20
```

Useful variants:

```bash
./test.sh --1080p --fps 60 --duration 20
./test.sh --720p --fps 30 --duration 10
./test.sh --res 2560x1440 --fps 50
./test.sh --deploy
```

Prepare a missing HEVC Annex-B stream:

```bash
./prepare_stream.sh --h265 --res 3840x2160 --fps 60
```

Stop the receiver:

```bash
./server.sh --stop
```

## Playback path

```text
workstation UDP sender
        ↓
bounded Annex-B HEVC access-unit reassembly
        ↓
RK3399 rkvdec (V4L2 stateless decoder)
        ↓
NV12 DMA-BUF
        ↓
DRM/KMS HDMI plane
```

The receiver retains only a small bounded compressed-frame queue in normal RAM. The current HDMI dashboard is an idle screen; it is released before the playback pipeline takes DRM ownership.

## Commands

`server.sh --start` enables the dashboard by default; use `--no-dashboard` only when an idle HDMI screen is not wanted.

`test.sh` is HEVC-only. It selects `client/assets/stream_<resolution>_<fps>fps_h265.265` unless `--file` is supplied.
