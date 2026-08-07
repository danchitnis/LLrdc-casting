# Aspect Ratio and Resolution Specification

## Purpose

This document describes how LLrdc handles screen capture, encoder resolution,
aspect-ratio preservation, HDMI output scaling, and the physical display panel.

The central rule is that four different geometries must not be confused:

1. The native Chrome capture geometry.
2. The selected encoded-frame geometry.
3. The negotiated HDMI signal geometry.
4. The physical panel geometry.

The resolution selector controls only the second item. It does not change the
Chrome capture size, the HDMI signal mode, or the physical panel size.

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

The resolution selector chooses the dimensions of the frame submitted to
WebCodecs and transmitted to the board.

| Label | Encoded dimensions | Aspect | Typical use |
| --- | ---: | ---: | --- |
| `720p` | `1280x720` | `16:9` | Lower bandwidth |
| `1080p` | `1920x1080` | `16:9` | Default/full HD |
| `1440p` | `2560x1440` | `16:9` | Higher detail |
| `2160p / 4K UHD` | `3840x2160` | `16:9` | Maximum selected stream size |

The selected dimensions control:

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

All current selectable encoder resolutions are `16:9`. Therefore they have
the same aspect-ratio behavior and differ primarily in pixel count, detail,
bitrate, and encoder workload.

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
    1280x720, 1920x1080, 2560x1440, or 3840x2160
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

Preserve mode compensates for both operations before encoding. Stretch mode
does not preserve the source aspect and fills every available rectangle.

## Aspect Modes

### Stretch

Stretch mode fills the selected encoded frame:

```text
Encoded content rectangle: <0,0,encoded_width,encoded_height>
HDMI signal content:       <0,0,signal_width,signal_height>
Panel content:             <0,0,panel_width,panel_height>
```

The source is drawn across the complete encoded canvas even if its aspect
ratio differs from the encoded frame. The source may therefore appear wider
or narrower on the physical panel.

For `3456x2234` captured to `1920x1080`:

```text
Source:  3456x2234
Encoded: 1920x1080
Content: 1920x1080
```

### Preserve

Preserve mode keeps the native source aspect ratio on the physical panel.
Black bars are created in the encoded frame where the source does not fill the
physical panel aspect.

Preserve mode is not calculated by simply fitting the source into the selected
`16:9` encoded frame. If it were, the monitor would then perform
its own `16:9 -> 16:10` scaling, which can make the image look too
narrow or squeezed.

Instead, the compositor calculates the layout in physical panel coordinates
first, then projects that layout backward through the HDMI signal and the
selected encoded frame.

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

The source is now correctly fitted against the physical panel, not against
the HDMI signal.

### Step 3: Project the Physical Layout to HDMI Signal Space

The physical-panel rectangle is mapped to the negotiated HDMI signal:

```text
signal_content_x      = round(panel_content_x * Wg / Wp)
signal_content_y      = round(panel_content_y * Hg / Hp)
signal_content_width  = round(panel_content_width * Wg / Wp)
signal_content_height = round(panel_content_height * Hg / Hp)
```

This models the monitor's conversion from the HDMI signal to the physical
panel.

### Step 4: Project the Signal Layout to the Encoded Frame

The signal rectangle is mapped to the selected encoded frame:

```text
encoded_content_x      = round(signal_content_x * We / Wg)
encoded_content_y      = round(signal_content_y * He / Hg)
encoded_content_width  = round(signal_content_width * We / Wg)
encoded_content_height = round(signal_content_height * He / Hg)
```

The source is drawn into this encoded rectangle. The encoded rectangle may be
intentionally non-proportional to the source because the monitor will apply a
non-uniform signal-to-panel scale later. This is pre-compensation, not an
aspect-ratio bug.

## Worked Example

Assume:

```text
Native source:       3456x2234
Selected encoding:   1920x1080
HDMI signal:         3840x2160
Physical panel:      3840x2400
Mode:                preserve
```

The compositor calculates approximately:

```text
Physical panel content: <63,0,3713,2400>
HDMI signal content:    <63,0,3713,2160>
Encoded content:        approximately <32,0,1857,1080>
```

The one-pixel difference between implementations or telemetry displays is
normal because the layout uses integer pixel rounding and centering.

The intended physical result is:

```text
Panel content:  approximately 3713x2400
Side bars:      approximately 63px each
Source aspect:  preserved at approximately 1.547
```

The encoded frame is transmitted as `1920x1080`. The HDMI signal remains
`3840x2160`, and the monitor maps that signal to the physical `3840x2400`
panel.

## Why the Old Calculation Produced Excessive Side Bars

The old preserve calculation fit the source directly into the selected

```text
3456x2234 -> 1920x1080
content     ~= 1671x1080
side bars   ~= 124px each in the encoded frame
```

That result was correct only if the encoded `16:9` frame was displayed on a
physical `16:9` panel. In this system, the physical panel is `16:10` and the
monitor performs an additional signal-to-panel scaling step.

The current calculation fits against the physical panel first and then
pre-compensates the encoded frame.

## Runtime Geometry Discovery

### Server

The server discovers:

- Active HDMI signal dimensions from DRM/KMS mode selection.
- Physical/native panel dimensions from EDID detailed timing data.
- Maximum advertised display capability separately from the native panel
  timing.

The server exposes signal and panel geometry in `TelemetryMessage::Status`.

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
[COMPOSITOR] preserve: ... encoded=<32,0,1857,1080>, signal=<63,0,3713,2160>, panel=<63,0,3713,2400>
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
