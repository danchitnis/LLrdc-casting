/*
 * Playback Pipeline Module
 * Manages DRM KMS geometry autodetection and persistent GStreamer playback process.
 */

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::config;

pub type SharedWriter = Arc<Mutex<std::sync::mpsc::SyncSender<Vec<u8>>>>;

// The RK3399 HDMI bridge advertises a 120x70 mm 4K mode. kmssink turns that
// device aspect ratio into a 15/16 pixel aspect ratio. Apply that correction
// to the parsed bitstream caps before the V4L2 decoder: the decoder then
// propagates it onto its native DMA-BUF output, keeping the decoder -> KMS
// path zero-copy. Applying it after decode with capssetter drops the
// memory:DMABuf feature and forces a slow system-memory presentation path.
pub struct PlaybackEngine {
    pub child: Child,
    pub writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    pub current_codec: String,
    pub width: u32,
    pub height: u32,
    pub render_rect: Option<String>,
    pub aspect_mode: String,
    pub dashboard_writer: SharedWriter,
}

impl PlaybackEngine {
    pub fn ensure_configuration(
        &mut self,
        target_codec: &str,
        connector: &str,
        render_rect: Option<&str>,
        aspect_mode: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let norm = normalize_codec(target_codec);
        if self.current_codec == norm
            && self.render_rect.as_deref() == render_rect
            && self.aspect_mode == aspect_mode
        {
            return Ok(());
        }
        println!(
            "[PLAYBACK SWITCH] Reconfiguring GStreamer pipeline: codec {} -> {}, rectangle {:?} -> {:?}, aspect {} -> {}",
            self.current_codec,
            norm,
            self.render_rect,
            render_rect,
            self.aspect_mode,
            aspect_mode,
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut new_engine = start_persistent_playback(norm, connector, render_rect, aspect_mode, self.width, self.height)?;
        if let Ok(mut writer) = self.dashboard_writer.lock() {
            *writer = new_engine.writer_tx.clone();
        }
        new_engine.dashboard_writer = self.dashboard_writer.clone();
        *self = new_engine;
        Ok(())
    }
}

fn normalize_codec(codec: &str) -> &str {
    let codec_lower = codec.to_lowercase();
    if codec_lower == "raw" || codec_lower == "bgra" || codec_lower == "dashboard" {
        "raw"
    } else if codec_lower.contains("264") {
        "h264"
    } else {
        "h265"
    }
}

pub fn autodetect_display_info() -> (u32, u32, u32, String, Option<String>, crate::drm_kms::EdidInfo) {
    if let Ok(card) = crate::drm_kms::open_display_card() {
        if let Ok((screen_w, screen_h, mode, conn_handle, _, edid_info)) = crate::drm_kms::autodetect_display_mode(&card) {
            let conn_id = u32::from(conn_handle).to_string();
            let rect = format!("<0,0,{screen_w},{screen_h}>");
            let refresh = mode.vrefresh() as u32;
            crate::drm_kms::drop_master(&card);
            drop(card);
            println!("[DISPLAY INFO] Auto-detected HDMI Connector {}, {}x{}@{}Hz, name='{}', rect={}", conn_id, screen_w, screen_h, refresh, edid_info.name, rect);
            return (screen_w, screen_h, refresh, conn_id, Some(rect), edid_info);
        }
        crate::drm_kms::drop_master(&card);
    }
    (
        config::display::DEFAULT_MAX_WIDTH,
        config::display::DEFAULT_MAX_HEIGHT,
        config::display::DEFAULT_MAX_FPS,
        config::playback::DEFAULT_DISPLAY_CONNECTOR_ID.into(),
        Some(format!(
            "<0,0,{},{}>",
            config::display::DEFAULT_MAX_WIDTH,
            config::display::DEFAULT_MAX_HEIGHT
        )),
        crate::drm_kms::EdidInfo::default(),
    )
}

pub fn start_persistent_playback(
    codec: &str,
    connector: &str,
    render_rect: Option<&str>,
    aspect_mode: &str,
    width: u32,
    height: u32,
) -> Result<PlaybackEngine, Box<dyn std::error::Error>> {
    let t_start = std::time::Instant::now();
    let plane = config::env_string_or("DRM_PLANE_ID", config::server::DEFAULT_DRM_PLANE_ID);
    let norm_codec = normalize_codec(codec);
    let mut gst_args = vec!["-q".to_string()];
    if norm_codec == "raw" {
        println!("[PLAYBACK STARTUP] Initializing raw BGRA dashboard pipeline at {width}x{height} at t=0ms");
        gst_args.extend([
            "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(), format!("blocksize={}", config::playback::RAW_PIPELINE_BLOCK_SIZE), "!".to_string(),
            "rawvideoparse".to_string(), "format=bgra".to_string(), format!("width={width}"), format!("height={height}"), format!("framerate={}", config::playback::RAW_PIPELINE_FRAMERATE), "!".to_string(),
        ]);
    } else {
        let (parser, bitstream_caps, decoder) = if norm_codec == "h264" {
            ("h264parse", "video/x-h264", "v4l2slh264dec")
        } else {
            ("h265parse", "video/x-h265", "v4l2slh265dec")
        };
        println!("[PLAYBACK STARTUP] Initializing persistent GStreamer pipeline for {norm_codec} ({parser} -> {decoder}) at t=0ms");
        gst_args.extend([
            "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(), format!("blocksize={}", config::playback::RAW_PIPELINE_BLOCK_SIZE), "!".to_string(),
            parser.to_string(), format!("config-interval={}", config::playback::ENCODED_CONFIG_INTERVAL), "!".to_string(),
            "capssetter".to_string(),
            format!("caps={bitstream_caps},pixel-aspect-ratio=(fraction){}", config::playback::KMS_DEVICE_PIXEL_ASPECT_RATIO),
            "replace=false".to_string(), "!".to_string(),
            decoder.to_string(), "!".to_string(),
        ]);
    }
    gst_args.extend([
        "kmssink".to_string(), "driver-name=rockchip".to_string(),
        format!("connector-id={connector}"), format!("plane-id={plane}"),
    ]);
    if let Some(rect) = render_rect {
        gst_args.push(format!("render-rectangle={rect}"));
    }
    gst_args.extend([
        "force-modesetting=false".to_string(), "can-scale=true".to_string(),
        "sync=false".to_string(), "async=false".to_string(), "skip-vsync=true".to_string(), "max-lateness=0".to_string(),
    ]);

    println!("[PLAYBACK STARTUP] Spawning continuous gst-launch-1.0 process...");
    let mut child = Command::new("gst-launch-1.0")
        .env("GST_DEBUG", "v4l2slh265dec:4,h265parse:4,v4l2slh264dec:4,h264parse:4,rawvideoparse:4,kmssink:4")
        .args(&gst_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
    if let Some(stderr) = child.stderr.take() {
        let t_stderr = t_start;
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                let lower = line.to_lowercase();
                if lower.contains("error") || lower.contains("warn") || lower.contains("corrupt") || lower.contains("missing") || lower.contains("drop") {
                    println!("[LAYER 2 ALERT] GStreamer Decoder Event (at {:.1}ms): {}", t_stderr.elapsed().as_secs_f32() * 1000.0, line);
                } else if lower.contains("resolution changed") || lower.contains("colorimetry") {
                    println!("[DECODER CAPS CHANGE] GStreamer (at {:.1}ms): {}", t_stderr.elapsed().as_secs_f32() * 1000.0, line);
                }
            }
        });
    }

    // Raw dashboard frames can be large; keep only two queued so
    // a stalled KMS sink cannot retain hundreds of megabytes of stale clocks.
    let queue_capacity = if norm_codec == "raw" {
        config::playback::RAW_QUEUE_CAPACITY
    } else {
        config::playback::ENCODED_QUEUE_CAPACITY
    };
    let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(queue_capacity);
    std::thread::spawn(move || {
        while let Ok(access_unit) = writer_rx.recv() {
            if stdin.write_all(&access_unit).and_then(|_| stdin.flush()).is_err() {
                eprintln!("[PLAYBACK] GStreamer stdin write failed");
                break;
            }
        }
    });

    println!("[PLAYBACK READY] Persistent GStreamer pipeline active on HDMI connector {connector}, plane {plane}");
    let dashboard_writer = Arc::new(Mutex::new(writer_tx.clone()));
    Ok(PlaybackEngine {
        child,
        writer_tx,
        current_codec: norm_codec.to_string(),
        width,
        height,
        render_rect: render_rect.map(str::to_owned),
        aspect_mode: aspect_mode.to_string(),
        dashboard_writer,
    })
}
