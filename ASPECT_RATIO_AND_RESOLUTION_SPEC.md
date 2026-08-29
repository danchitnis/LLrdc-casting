# Aspect Ratio and Resolution Specification

## Purpose

This document describes how LLrdc handles screen capture, encoder resolution,
aspect-ratio preservation, HDMI output scaling, and the physical display panel.

The central rule is that four different geometries must not be confused:

1. The native Chrome capture geometry.
2. The selected encoded-frame geometry.
3. The negotiated HDMI signal geometry.
4. The physical panel geometry.

The encoder-resolution selector controls only the second item. It does not change
the Chrome capture size, the HDMI signal mode, or the physical panel size.

## Geometry Spaces

### 1. Native Capture Space

Chrome captures the entire selected monitor using `getDisplayMedia()` without
width or height constraints. The browser is requested to provide a monitor
surface:

```ts
{
  video: {
    displaySurface: 'monitor',
    frameRate: { ideal: targetFps }
  },
  monitorTypeSurfaces: 'include',
  selfBrowserSurface: 'exclude',
  audio: false
}
```

The client validates that the returned track has:

```ts
track.getSettings().displaySurface === 'monitor'
```

The capture dimensions come from the returned track and the actual
`VideoFrame` objects. They are not forced to match the selected encoding
resolution.

Example:

```text
Native capture: 3456x2234
Source aspect: 3456 / 2234 = approximately 1.547
```

The capture is full-screen. The client does not crop it to `16:9` before
aspect-ratio processing.

### 2. Encoded Frame Space

The encoder-resolution selector chooses the dimensions of the frame submitted to
WebCodecs and transmitted to the board.

| Label | Encoded dimensions | Typical use |
| --- | ---: | --- |
| `720p` | `1280x720` | Lower bandwidth |
| `1080p` | `1920x1080` | Default/full HD |
| `1440p` | `2560x1440` | Higher detail |
| `2160p / 4K UHD` | `3840x2160` | Maximum selected stream size |

The calculated encoded dimensions control:

- The `OffscreenCanvas` size.
- The `VideoEncoder` width and height.
- The encoded-frame dimensions in the packet header.
- The automatic bitrate calculation.
- The amount of source detail retained by the encoder.

The selected dimensions do not control:

- The dimensions requested from Chrome.
- The HDMI connector mode.
- The physical display panel.
- The KMS render rectangle.

Stretch output uses the active HDMI signal aspect, so it intentionally stretches
the source when the source aspect differs from the signal. Preserve output uses
the selected encoder canvas and adjusts the browser-composed content rectangle
to the physical panel aspect so the source can be preserved through the
HDMI-to-panel scale.

Preserve is a browser-compositor operation. It does not change the KMS
destination rectangle. KMS always presents the decoded frame at 100% of the
active HDMI signal, including 100% of its height:

```text
KMS render rectangle: <0,0,signal_width,signal_height>
```

### 3. HDMI Signal Space

The server discovers the active HDMI signal from DRM/KMS display mode
information. This is the signal negotiated by the board and the HDMI chip.

The value is dynamic. It must not be replaced with a hardcoded value such as
`3840x2160` in the client or compositor.

Example:

```text
Negotiated HDMI signal: 3840x2160
Signal aspect: 3840 / 2160 = 16:9
```

The server maps the decoded encoded frame to the complete active signal:

```text
KMS render rectangle: <0,0,signal_width,signal_height>
```

For the example above:

```text
KMS render rectangle: <0,0,3840,2160>
```

KMS does not calculate aspect ratio from the laptop capture dimensions. It
only scales the already-composed encoded frame across the active HDMI signal.
The selected encoder resolution is not a KMS display mode and does not change
the KMS rectangle. For example, `1280x720`, `1920x1080`, `2560x1440`, and
`3840x2160` are all presented into the same full HDMI signal rectangle.

### 4. Physical Panel Space

The physical panel has its own native geometry. It cannot be changed by the
encoder resolution selector or by choosing a different HDMI input mode.

The server derives the panel timing from EDID/native detailed timing data and
publishes it separately from the maximum advertised capability.

Example:

```text
Physical panel: 3840x2400
Panel aspect: 3840 / 2400 = 16:10
```

The monitor accepts the negotiated `3840x2160` HDMI signal and internally maps
that signal to the physical `3840x2400` panel. This is a second scaling stage
after KMS.

## End-to-End Pipeline

The complete video path is:

```text
Chrome monitor capture
    native source dimensions
        |
        v
Browser OffscreenCanvas compositor
    preserve or stretch
    selected encoded dimensions
        |
        v
WebCodecs VideoEncoder
    fixed encoder resolution selected in the UI
        |
        v
WebTransport / QUIC
        |
        v
RK3399 decoder
    decoded encoded frame
        |
        v
KMS / kmssink
    full active HDMI signal rectangle
        |
        v
HDMI chip / monitor scaler
    negotiated signal geometry -> physical panel geometry
        |
        v
Physical display panel
```

There are two scaling operations after the browser compositor:

1. KMS scales the selected encoded frame to the active HDMI signal.
2. The monitor scales the HDMI signal to the physical panel.

The custom encoder resolution changes the amount of data processed by the
browser encoder, transport, decoder, and KMS input plane. It can therefore
change bandwidth, encoder workload, decoder workload, and frame-buffer traffic.
It does not create a custom HDMI mode, apply a second aspect-ratio decision, or
change the KMS destination rectangle. KMS only receives the decoded frame and
scales it to the full active signal.

Preserve mode compensates for the signal-to-panel scaling before encoding. The
encoded canvas uses the physical panel aspect, so KMS can scale it to the
16:9 HDMI signal and the monitor can scale that signal back to the panel without
changing the source aspect. Stretch mode does not preserve the source aspect
and fills the encoded canvas.

## Aspect Modes

### Stretch

Stretch mode ignores the source aspect ratio when drawing into the encoder
canvas. It fills the entire encoded frame, so a source with a different aspect
ratio is geometrically stretched. KMS then presents that already-stretched
frame across the full HDMI signal.

For a `1920x1080` encoder frame:

```text
Encoded content rectangle: <0,0,encoded_width,encoded_height>
HDMI signal content:       <0,0,signal_width,signal_height>
Panel content:             <0,0,panel_width,panel_height>
```

For `3456x2234` captured into a `1920x1080` encoder frame, the source is drawn
across all `1920x1080` pixels and is therefore stretched from approximately
`1.547:1` to `16:9`.

```text
Source:  3456x2234
Encoded: 1920x1080
Content: 1920x1080
```

### Preserve

Preserve mode keeps the native source aspect ratio on the physical panel. The
compositor first fits the source to the physical panel aspect, then projects
that rectangle backward into the HDMI signal and encoder spaces. Black bars are
created in the encoded frame where the source does not fill the panel-shaped
content area.

For `3456x2234` captured into a `1920x1080` encoder frame:

```text
Source:  3456x2234        approximately 1.547:1
Encoded: 1920x1080        16:9 canvas
Content: approximately <32,0,1857,1080>
```

The content rectangle preserves the source aspect. The encoded canvas may have
a different aspect from the source because it is pre-compensated for the
`3840x2160` HDMI signal being mapped to the physical `3840x2400` panel. KMS
still uses the complete HDMI rectangle in both Preserve and Stretch modes.

## Preserve Calculation

Use these variables:

```text
Ws, Hs = native source width and height
We, He = selected encoded-frame width and height
Wg, Hg = negotiated HDMI signal width and height
Wp, Hp = physical panel width and height
```

### Step 1: Calculate Source and Panel Aspect Ratios

```text
source_aspect = Ws / Hs
panel_aspect  = Wp / Hp
```

For the deployed example:

```text
source_aspect = 3456 / 2234 = approximately 1.547
panel_aspect  = 3840 / 2400 = 1.600
```

The source is narrower than the panel.

### Step 2: Fit the Source in Physical Panel Space

If the source is narrower than the panel:

```text
panel_content_height = Hp
panel_content_width  = round(Hp * source_aspect)
panel_content_x      = floor((Wp - panel_content_width) / 2)
panel_content_y      = 0
```

If the source is wider than the panel:

```text
panel_content_width  = Wp
panel_content_height = round(Wp / source_aspect)
panel_content_x      = 0
panel_content_y      = floor((Hp - panel_content_height) / 2)
```

The source is now correctly fitted against the physical panel.

### Step 3: Project the Physical Layout to HDMI Signal Space

The physical-panel rectangle is mapped to the negotiated HDMI signal:

```text
signal_content_width  = round(panel_content_width * Wg / Wp)
signal_content_height = round(panel_content_height * Hg / Hp)
signal_content_x      = floor((Wg - signal_content_width) / 2)
signal_content_y      = floor((Hg - signal_content_height) / 2)
```

This models the monitor's conversion from the HDMI signal to the physical
panel.

### Step 4: Project the Signal Layout to the Encoded Frame

```text
encoded_content_width  = round(signal_content_width * We / Wg)
encoded_content_height = round(signal_content_height * He / Hg)
encoded_content_x      = floor((We - encoded_content_width) / 2)
encoded_content_y      = floor((He - encoded_content_height) / 2)
```

The source is drawn into this encoded rectangle. The encoded rectangle may be
intentionally non-proportional to the source because KMS scales the encoded
frame to the signal. This is the required signal pre-compensation, not a crop.

## Worked Example

Assume:

```text
Native source:       3456x2234
Selected encoding:   1920x1080
Encoded output:     1920x1080 (preserve canvas)
HDMI signal:         3840x2160
Physical panel:      3840x2400
Mode:                preserve
```

The compositor calculates approximately:

```text
Physical panel content: approximately <63,0,3713,2400>
HDMI signal content:    approximately <63,0,3713,2160>
Encoded content:        approximately <31,0,1857,1080>
```

The one-pixel difference between implementations or telemetry displays is
normal because the layout uses integer pixel rounding and centering.

The intended physical result is:

```text
Signal content: approximately 3713x2160
Side bars:      approximately 63px each on the HDMI signal
Source aspect:  preserved at approximately 1.547
```

The encoded frame is transmitted at the selected encoder resolution. The HDMI signal remains
`3840x2160`, and the monitor maps that signal to the physical `3840x2400`
panel.

## Why the Old Calculation Produced Excessive Side Bars

The old preserve calculation fit the source directly into a fixed `16:9`
selected frame:

```text
3456x2234 -> 1920x1080
content     ~= 1671x1080
side bars   ~= 124px each in the encoded frame
```

That result was correct only if the encoded `16:9` frame was displayed on a
physical `16:9` panel. In this system, the physical panel is `16:10` and the
monitor performs an additional signal-to-panel scaling step.

The current calculation uses a panel-shaped encoded canvas, fits the source in
that canvas, and projects the resulting rectangle through the HDMI signal.

## Runtime Geometry Discovery

### Server

The server discovers:

- Active HDMI signal dimensions from DRM/KMS mode selection.
- Physical/native panel dimensions from EDID detailed timing data.
- Maximum advertised display capability separately from the native panel
  timing.

The server exposes signal and panel geometry in `TelemetryMessage::Status`.

Refresh-rate telemetry keeps the active mode separate from capability:

- `display_fps` is the currently negotiated HDMI scanout rate (for example,
  30 Hz).
- `display_max_fps` is the highest refresh rate advertised by the monitor
  EDID/driver (for example, 60 Hz), for diagnostics and capability reporting.
  It is not necessarily the currently usable scanout rate.

The client gates its FPS selector on `display_fps`, so a 60 FPS option is
disabled while the active HDMI mode is 30 Hz, even if a faster mode is
advertised.

### Client

The client receives and stores:

```ts
interface DisplayGeometry {
  signalWidth: number;
  signalHeight: number;
  panelWidth: number;
  panelHeight: number;
}
```

The client will not start streaming until valid HDMI signal and panel geometry
telemetry has arrived. It does not silently assume `3840x2160` or `3840x2400`;
it reports a display-geometry error if the telemetry is unavailable.

### Control Telemetry

The start command reports all relevant geometry, including:

- Native capture width and height.
- Encoded width and height.
- Aspect mode.
- Encoded content rectangle.
- HDMI signal content rectangle.
- Physical panel content rectangle.
- HDMI signal width and height.
- Panel width and height.

This makes it possible to diagnose whether an aspect problem originates in
Chrome capture, browser compositing, KMS scaling, or monitor scaling.

## KMS Responsibilities

KMS is intentionally simple in this architecture:

```text
render-rectangle = <0,0,signal_width,signal_height>
```

KMS must not:

- Recalculate aspect ratio from native laptop dimensions.
- Add a second preserve/stretch decision.
- Use the selected encoder resolution as the physical display resolution.
- Replace the browser-composed bars.

The browser compositor owns preserve/stretch behavior. KMS only presents the
encoded frame across the active signal.

## Verification

### Browser Verification

For a real monitor capture, the log should report values similar to:

```text
[SOURCE] Capture dimensions: 3456x2234 (monitor)
[DISPLAY] HDMI signal=3840x2160, panel=3840x2400
[COMPOSITOR] preserve: ... encoded=<31,0,1857,1080>, signal=<63,0,3713,2160>, panel=<63,0,3713,2400>
```

Stretch mode should report full rectangles:

```text
encoded=<0,0,1920,1080>
signal=<0,0,3840,2160>
panel=<0,0,3840,2400>
```

### Board Verification

Use the repository workflow:

```bash
./server.sh --start
./test.sh -d 5
ssh 100.100.1.72 "docker logs --tail 80 llrdc-casting"
```

The board logs should confirm:

- A valid active HDMI mode.
- A valid native/panel EDID resolution.
- Full-display KMS rectangle.
- Complete frame reassembly.
- Valid decoded bitstreams.
- No stream or decoder errors.

The direct `test.sh` stream is a transport and decoder test. It does not
exercise the browser compositor's preserve/stretch behavior; those modes
must be verified through the Chrome Web UI.

## Source References

- `client/src/lib/compositor.ts`: aspect layout and frame composition.
- `client/src/lib/streamer.ts`: Chrome capture, encoder configuration, and geometry telemetry.
- `src/drm_kms.rs`: DRM mode and EDID/native panel discovery.
- `src/control.rs`: control commands and geometry telemetry schema.
- `src/main.rs`: server state and display geometry propagation.
- `src/playback.rs`: full-display KMS rendering and persistent GStreamer playback.
