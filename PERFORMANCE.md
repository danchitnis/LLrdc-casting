# LLrdc Casting Performance

LLrdc Casting prioritizes fresh output over preserving every captured frame. Its
ultra-low-latency mode limits encoder and media-write queues to one item, drops
raw input before encoding when those limits are reached, and reduces bitrate
after sustained congestion. Encoded reference frames remain ordered.

## Reference result

| Configuration | Measurement window | Samples | Average | Sequence gaps |
| --- | ---: | ---: | ---: | ---: |
| HEVC, 1920×1080, 30 FPS, ultra-low latency | 5–15 seconds | 10 | 68.2 ms | 0 |

The values are synchronized **encoder-input-to-display estimates**. They are
not camera-to-photon or externally measured glass-to-glass latency.

## Reference configuration

- Measured: 2026-09-01 00:44:20 UTC
- Receiver: Radxa ROCK 4C+ / RK3399, Linux 6.18.45-current-rockchip64,
  AArch64
- Decoder/display: Rockchip V4L2 stateless HEVC decode to DRM/KMS HDMI
- Display: 3840×2160 at 30 Hz
- Sender: macOS on Apple silicon, installed Chrome 152.0.0.0 using WebCodecs
- Source: deterministic synthetic test pattern
- Stream: HEVC, 1920×1080 selection (1920×1088 encoded), 30 FPS, automatic
  bitrate at a configured 6 Mbps ceiling
- Priority: ultra-low latency
- Transport: authenticated WebTransport to the receiver's RFC1918 address over
  its `end0` wired-LAN interface
- Display estimate: one refresh period of the active HDMI mode

The benchmark artifact records the run timestamp, browser user agent, sender
platform, receiver kernel and architecture, selected configuration, active
display rate, sample count, arithmetic mean, and sequence integrity. It contains no
receiver address, pairing code, connection token, or private credential.

## Measurement boundary

Timing starts when a selected `VideoFrame` enters composition and encoding. A
timed access unit carries sender monotonic timestamps to the receiver. The
receiver acknowledges it after the complete access unit has been written and
flushed into the playback pipeline.

The reported total is the sum of:

1. browser composition and WebCodecs encoding;
2. sender queue time before the media write;
3. synchronized WebTransport delivery and receiver reassembly;
4. receiver queue and GStreamer input time; and
5. one active-display refresh period as the decode/display allowance.

The estimate does not include browser or operating-system work before the
`VideoFrame` enters LLrdc Casting, an instrumented hardware decoder completion
event, the display's internal processing, pixel response time, or observation
with an external high-speed camera.

See [Latency Measurement and Congestion Control](LATENCY_AND_CONGESTION.md) for
clock synchronization, validation, smoothing, backpressure, and adaptation
details.

## Benchmark procedure

Run the receiver and sender on wired Ethernet and target the receiver's private
RFC1918 address rather than its Tailscale address:

```sh
./test_browser.sh codec chrome --board-ip=<receiver-private-lan-ip>
```

During the first HEVC 1080p cycle, the suite:

1. confirms installed-Chrome HEVC support and direct WebTransport;
2. starts the deterministic 30 FPS synthetic source in ultra-low-latency mode;
3. waits five seconds without retaining startup samples;
4. retains every unsmoothed estimate from the next ten seconds;
5. calculates their arithmetic mean;
6. requires zero missing access-unit sequence IDs; and
7. separately verifies that fresh synchronized samples continue while the
   management portal has focus.

The versioned result is written under the run's Chrome artifact directory:

```text
.artefact/codec-<UTC timestamp>-<pid>/chrome/performance-summary.json
```

## Result artifact

The JSON document has this stable top-level shape:

```json
{
  "schema_version": 2,
  "metric": "average_estimated_encoder_input_to_display_ms",
  "measured_at": "<ISO-8601 UTC>",
  "environment": {},
  "configuration": {},
  "sample_count": 0,
  "warmup_seconds": 5,
  "measurement_seconds": 10,
  "sample_kind": "unsmoothed_phase_estimate",
  "average_ms": 0,
  "sequence_gaps": 0
}
```

Only results produced by a passing hardware suite under the documented
reference conditions should be copied into this file or the README.
