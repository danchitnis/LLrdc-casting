# Archived: Zero-copy Playback Overlay Plan

## Goal

Show a small status overlay without copying, mapping, or CPU-compositing the decoded 4K frame:

```text
HEVC -> rkvdec (V4L2 stateless) -> NV12 DMA-BUF -> DRM video plane -> HDMI
```

## Current constraint

The working player uses one external `gst-launch ... kmssink` process. A second `kmssink` for an overlay fails on this RK3399 DRM driver with `drmModeSetPlane: Permission denied` because the display path cannot have two independent KMS owners.

The IP dashboard is an idle-only screen. It explicitly releases DRM master before video playback starts.

## Required implementation

1. Move HEVC decode and DRM presentation into one process.
2. Feed access units to GStreamer `appsrc`; receive decoder-owned DMA-BUFs from `appsink`.
3. Retain one DRM master for both planes.
4. Import NV12 decoder DMA-BUFs on the video plane.
5. Allocate a small ARGB scanout buffer for `text.rs` on the RGB overlay plane.
6. Update the text buffer at 1-10 Hz and use one DRM atomic commit for both planes.
7. Retain each decoder buffer until its page-flip completion event.

## Acceptance checks

- 4K60 HEVC stays smooth with the overlay enabled.
- No CPU copy/composite of decoded pixels.
- Overlay can be enabled/disabled at server start; enable by default only after validation.
- Memory remains bounded during a 60-second test.
- Verify plane state with `modetest -M rockchip -p`.
- Measure end-to-end frame age; target a few frames and ultimately <=20 ms.

## Do not use

- A second `kmssink`
- GStreamer `compositor`, `cairooverlay`, or CPU blending on decoded frames

Those paths either fail with the current DRM-master model or risk degrading 4K60 playback.
