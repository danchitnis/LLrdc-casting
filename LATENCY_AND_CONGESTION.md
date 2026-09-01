# Latency Measurement and Congestion Control

This document describes how LLrdc Casting estimates end-to-end stream latency
and keeps that latency bounded when the sender, network, receiver, or display
cannot sustain the requested stream. It documents the behavior implemented in
the browser sender and RK3399 receiver; it is not a proposal.

## Design priority

The streaming path prioritizes freshness over requested frame rate and image
quality. A recent frame at a temporarily lower bitrate or effective FPS is more
useful for interactive screen sharing than an older frame delivered after a
large queue.

The policy therefore follows three rules:

1. Apply backpressure before encoding new input.
2. Drop only newly captured raw frames when necessary.
3. Never discard an access unit after it has been encoded and placed on the
   reliable ordered media path.

The third rule preserves H.264/H.265 reference-frame continuity. Dropping an
encoded inter-frame could corrupt playback until a later keyframe.

## What the latency estimate covers

Latency begins when a selected `VideoFrame` enters composition and encoding.
Operating-system screen-capture acquisition time before that point is outside
the measurement.

The estimate ends with one active-display refresh interval. GStreamer input
completion is measured, but hardware decode, scanout, and photon emission are
not directly instrumented. Exact camera-to-photon or screen-to-photon latency
requires external optical measurement or deeper hardware instrumentation.

The management portal presents five additive phases:

| Phase | Calculation | Meaning |
| --- | --- | --- |
| Encode | encoder output − encoder input | Browser composition and WebCodecs encoding |
| Sender queue | media write start − encoder output | Time waiting behind earlier encoded writes |
| Delivery | synchronized receiver AU completion − media write start | Reliable media delivery and receiver reassembly |
| Receiver queue/input | GStreamer flush completion − receiver AU completion | Receiver playback queue plus GStreamer input |
| Decode/display | `1000 / active display FPS` | Clearly marked one-refresh allowance |

`Estimated total` is the sum of all five phases. WebTransport
`writer.write()` duration is shown separately as media-write backpressure. It
overlaps delivery and is therefore a diagnostic, not a sixth additive phase.

## Clock synchronization

Sender and receiver timestamps use monotonic epoch clocks. Ping/pong messages
carry the client-send, server-receive, and server-send timestamps needed to
estimate their offset.

The sender retains a rolling window of eight synchronization samples and uses
the sample with the lowest network delay. Its initial uncertainty is half that
sample's synchronization round-trip delay. Displayed uncertainty increases as
the selected sample ages to account for possible clock drift.

Small negative cross-clock differences are clamped to zero only when they fit
inside the calculated uncertainty. Samples are rejected when they contain
non-finite values, are unexpectedly in the future, exceed the uncertainty in
the negative direction, are older than 30 seconds, or describe a pipeline
longer than 30 seconds.

## Sampling and smoothing

The receiver acknowledges the first timed access unit after it is flushed to
GStreamer, then acknowledges another completed timed access unit after at least
one second has elapsed. The sender reports no more than one latency sample per
second.

The current value shown in the casting UI and portal is smoothed with an
exponentially weighted moving average using an alpha of `0.25`. The retained
chart and benchmark samples are unsmoothed phase estimates, so startup can be
excluded without its smoothing history leaking into the measurement window.
The portal retains the last current value and marks its age when sampling is
interrupted instead of generating synthetic points.

## Sender queue limits

Before composing or encoding a captured frame, the congestion controller
checks the WebCodecs `encodeQueueSize` and the number of outstanding media
writes. A frame that would exceed either applicable limit is closed immediately
and counted as a dropped raw-input frame.

| Latency mode | Encoder-queue limit | Outstanding-write limit | Priority |
| --- | ---: | ---: | --- |
| ULL | 1 | 1 | Freshest possible output |
| Balanced | 2 | 2 | Moderate burst tolerance |
| Quality | 8 | 8 | More frame preservation before throttling |

These limits apply before encoding. Encoded access units are serialized through
the media writer and remain ordered.

## Congestion detection

The controller maintains two-second rolling windows and evaluates them every
two seconds. A window is congested when any of the following is true:

- sender-queue p95 is greater than one requested frame interval;
- media-write-blocked p95 is greater than one requested frame interval; or
- at least 10% of observed input frames encountered a queue/write limit.

Two consecutive congested windows are required before reducing bitrate. This
adds enough hysteresis to ignore a single short spike while reacting to
sustained pressure in roughly four seconds.

## Adaptive bitrate

The bitrate selected in the client is the ceiling. After confirmed congestion,
the controller:

1. reduces the current bitrate by 20%;
2. never reduces it below 40% of the selected ceiling; and
3. forces the next encoded frame to be a keyframe after reconfiguring WebCodecs.

If congestion persists at the floor, the sender continues rejecting new raw
input as required. Effective FPS falls, but the sender does not discard already
encoded reference frames or allow a stale queue to grow without bound.

After the first healthy evaluation, the controller waits for ten healthy
seconds. It then restores 10% of the selected ceiling per step, with recovery
steps no more frequent than every five seconds, until the ceiling is reached.
Any new congested window cancels the current healthy interval.

## Receiver backpressure

Receiver buffering is intentionally shallow so downstream pressure reaches the
sender promptly:

- the completed-access-unit channel has capacity two; and
- the encoded GStreamer input queue has capacity one.

The GStreamer writer performs an ordered `write_all` followed by `flush` before
emitting timing acknowledgement metadata. A slow decoder or display therefore
increases receiver/input time and eventually blocks upstream delivery rather
than hiding delay in a large playback queue.

## Reading the management diagnostics

The management portal updates displayed diagnostic samples at approximately
1 Hz. Congestion decisions use the separate two-second evaluation cadence.

| Diagnostic | Interpretation |
| --- | --- |
| Media write backpressure | Time an encoded write remained blocked; high values indicate downstream pressure |
| Adaptive bitrate | Current encoder bitrate compared with the user-selected ceiling |
| Dropped raw input frames | Fresh captures rejected before encoding to prevent queue growth |
| Effective sender FPS | Actual encoded output rate after input selection and congestion control |
| Access-unit size | Encoded size of the sampled access unit |
| Clock confidence | Synchronization uncertainty and the age of the selected clock sample |
| Missing sequence IDs | Encoded access units missing at the receiver; expected to remain zero on the reliable path |

Phase interpretation:

- High **Encode** indicates sender composition or encoder pressure.
- High **Sender queue** indicates encoded output is waiting to start its media
  write; congestion control should reduce future work.
- High **Delivery** with low sender and receiver queues points toward network or
  transport delivery.
- High **Receiver queue/input** indicates receiver playback or GStreamer input
  cannot keep pace.
- **Decode/display** normally equals one refresh interval and is an estimate,
  not a measured hardware-decoder duration.

## Reset behavior

Timing maps, clock synchronization, smoothing, pending acknowledgements,
congestion windows, dropped-frame counters, and adaptive bitrate are reset when
a stream starts or stops, the transport disconnects, stream ownership changes,
or the decoder is reset. A new session therefore cannot inherit congestion or
timing state from its predecessor.

## Authoritative implementation and tests

The main implementation points are:

- `client/src/lib/latency.ts`: clock synchronization, phase calculation,
  validation, report cadence, and EWMA smoothing.
- `client/src/lib/congestion.ts`: queue limits, rolling congestion windows, and
  bitrate adaptation.
- `client/src/lib/stream-worker.ts` and `client/src/lib/streamer.ts`: raw-frame
  selection, encoding, ordered media writes, and WebCodecs reconfiguration.
- `src/v4l2_decoder.rs`: timed packet parsing and access-unit reassembly.
- `src/playback.rs`: bounded GStreamer input and post-flush acknowledgements.
- `src/management.rs`: retained phase samples and management diagnostics.
- `src/config.rs`: receiver transport and playback queue capacities.

Regression coverage is in `client/src/lib/latency.test.ts`,
`client/src/lib/congestion.test.ts`, Rust module tests, and the Chrome codec and
management suites in `client/e2e/`.

When changing thresholds or queue capacities, update this document and the
corresponding deterministic tests in the same commit.
