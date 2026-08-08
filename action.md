# Action Plan: Resolving Choppy Video Streaming & Latency Optimization in LLrdc-casting

This document outlines the step-by-step action plan to eliminate choppy video streaming, reduce end-to-end latency, and improve video presentation quality in `LLrdc-casting`, incorporating architectural insights and best practices from `LLrdc`.

---

## 1. Executive Summary & Root Cause Analysis

Based on deep analysis of `LLrdc-casting` and comparative benchmarking against `LLrdc`, four primary root causes were identified for the current choppy streaming behavior:

1. **Destructive Delta-Frame Discarding in Receiver (`src/main.rs`):**
   - The loop in `main.rs` drains the MPSC channel using `while let Ok(newer) = rx.try_recv() { frame = newer; }`.
   - When GStreamer's `stdin` write takes longer than 16ms, intermediate H.265/H.264 P-frames are discarded.
   - Dropping reference frames breaks the HEVC temporal decoder state, causing severe macroblocking, picture tearing, and frozen video until the next keyframe.

2. **GStreamer `fdsrc` Buffering & Missing Caps:**
   - GStreamer is launched with `fdsrc blocksize=262144` (256KB) without explicit video caps (`alignment=au`).
   - Standard 1080p delta frames are ~15KB–40KB. `fdsrc` accumulates frames until reaching 256KB before pushing downstream to `h265parse`, causing frame batching (bursts of 5 frames followed by an 80ms freeze).

3. **UDP Packet Bursting at the Sender (`client/client.mjs`):**
   - The streamer loops synchronously over all ~50–300 UDP chunks (1350 bytes each) per frame without pacing.
   - Bursting hundreds of packets in microseconds overruns kernel socket buffers (`SO_RCVBUF`). Losing even 1 chunk causes `v4l2_decoder.rs` to drop the entire frame after a 50ms timeout.

4. **Sender Timer Jitter & Pacing:**
   - Node.js `setTimeout` loop scheduling suffers from 2ms–15ms event loop jitter, causing erratic frame delivery intervals (5ms–35ms spikes instead of a consistent 16.6ms).

---

## 2. Key Architectural Lessons from `LLrdc`

- **Native WebTransport (HTTP/3 over QUIC):** QUIC handles congestion control, packet pacing, and loss recovery at the transport layer. Sending complete compressed frames over QUIC streams avoids single-packet chunk drop cascades.
- **AUD (Access Unit Delimiter) Slicing:** Instructing encoders to insert AUD markers (`-aud 1`) allows instant frame boundary parsing (`0x09` for H.264, NAL 35 for H.265), eliminating 1-frame buffering delays.
- **Sequential Decoder Delivery:** Every inter-frame encoded frame must reach the decoder sequentially. If frame dropping is necessary for latency catch-up, an IDR keyframe must be requested.
- **Real-Time Priority Scheduling:** Elevating display and input processing to `SCHED_FIFO` prevents thread preemption during high bit-rate stream playback.

---

## 3. Phase-by-Phase Action Plan

### Phase 1: GStreamer & Pipeline Fixes (Immediate Priority)

**Target Files:** `src/main.rs`

1. **Eliminate Non-Keyframe Discarding:**
   - Remove the destructive `while let Ok(newer) = rx.try_recv() { frame = newer; }` logic from `src/main.rs`.
   - Ensure every reassembled video frame is delivered sequentially to GStreamer's `stdin`.

2. **Optimize GStreamer Pipeline Command & Caps:**
   - Remove `blocksize=262144` from `fdsrc`.
   - Add explicit caps and parameters:
     ```bash
     fdsrc fd=0 ! video/x-h265,stream-format=byte-stream,alignment=au ! h265parse config-interval=-1 ! v4l2slh265dec ! kmssink driver-name=rockchip connector-id=<connector> plane-id=<plane> force-modesetting=false sync=false skip-vsync=true max-lateness=0
     ```
   - Set `do-timestamp=true` on `fdsrc` to ensure proper downstream frame timing.

---

### Phase 2: Sender Pacing & Chunk Assembly Optimizations

**Target Files:** `client/client.mjs`, `src/v4l2_decoder.rs`

1. **Implement UDP Packet Pacing in Sender (`client/client.mjs`):**
   - Introduce micro-gapping or packet pacing between UDP chunks in `sendVideoFrame` to avoid kernel socket buffer overflow.
   - Use high-resolution timer (`performance.now()`) with drift compensation for target frame rates (30/60 FPS).

2. **Enhance Frame Assembly & AUD Slicing (`src/v4l2_decoder.rs`):**
   - Ensure `v4l2_decoder.rs` validates that every completed `access_unit` starts with a valid start code (`0x00000001` or `0x000001`) and includes AUD / SPS / PPS NAL units.
   - Log assembly completion metrics clearly without locking worker threads.

---

### Phase 3: WebTransport QUIC Integration

**Target Files:** `src/webtransport_server.rs`, `client/client.mjs` (or browser client)

1. **Activate Full WebTransport QUIC Stream Path:**
   - Utilize the `wtransport` server implementation in `src/webtransport_server.rs` to accept unidirectional QUIC streams.
   - Wire client frame transmissions to send complete frame payloads over QUIC streams with a 13-byte header (4-byte packet length + 1-byte type + 8-byte timestamp), bypassing custom 1350-byte UDP fragmentation.

2. **Enable WebTransport Datagram Fallback:**
   - Allow ultra-low-latency WebTransport datagrams for smaller keyframe/delta payloads.

---

### Phase 4: Process Scheduling & System Performance

**Target Files:** `src/main.rs`

1. **Elevate Process Priority (`SCHED_FIFO`):**
   - Use `nix::sys::sched` or `libc::sched_setscheduler` in `src/main.rs` to set `SCHED_FIFO` real-time priority for the video playback thread and `gst-launch-1.0` child process.

---

## 4. Verification & Testing Plan

1. **Synthetic Stream Benchmark:**
   - Execute `./stream.sh 127.0.0.1 4434 1080p 60 H265 -d 20` and measure frame delivery rate in `dmesg` / kernel monitor output.
2. **Frame Integrity Verification:**
   - Monitor `[FRAME INTEGRITY]` logs in `v4l2_decoder.rs` to ensure `DeliveryRate` exceeds 99.5% with 0 dropped timeouts.
3. **Decoder Error Check:**
   - Ensure `[LAYER 1 ALERT]` and `[LAYER 2 ALERT]` logs in `main.rs` report zero `rkvdec` corrupt or missing reference frame errors.
4. **Visual Smoothness Verification:**
   - Verify smooth 60 FPS video playback on the HDMI output without stuttering or micro-freezes.

---

*Note: Implementation will begin upon user confirmation.*
