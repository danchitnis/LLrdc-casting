/*
 * Playback Pipeline Module
 * Manages DRM KMS geometry autodetection and persistent GStreamer playback process.
 */

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config;

#[derive(Debug)]
pub struct PlaybackTiming {
    pub seq: u32,
    pub capture_time_ms: f64,
    pub encode_duration_ms: f32,
    pub send_start_time_ms: f64,
    pub receiver_complete_time_ms: f64,
}

pub struct PlaybackBuffer {
    pub bytes: Vec<u8>,
    pub timing: Option<PlaybackTiming>,
}

impl From<Vec<u8>> for PlaybackBuffer {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes, timing: None }
    }
}

#[derive(Debug)]
pub struct PlaybackSubmission {
    pub seq: u32,
    pub capture_time_ms: f64,
    pub encode_duration_ms: f32,
    pub send_start_time_ms: f64,
    pub receiver_complete_time_ms: f64,
    pub receiver_queue_ms: f64,
}

fn receiver_queue_ms(receiver_complete_time_ms: f64, flush_time_ms: f64) -> f64 {
    (flush_time_ms - receiver_complete_time_ms).max(0.0)
}

pub struct LatencyAckCadence {
    last_sent_at: Option<Instant>,
    interval: Duration,
}

impl Default for LatencyAckCadence {
    fn default() -> Self {
        Self { last_sent_at: None, interval: Duration::from_secs(1) }
    }
}

impl LatencyAckCadence {
    pub fn should_emit(&mut self, now: Instant) -> bool {
        if self.last_sent_at.is_none_or(|last| now.duration_since(last) >= self.interval) {
            self.last_sent_at = Some(now);
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self) {
        self.last_sent_at = None;
    }
}

pub type SharedWriter = Arc<Mutex<std::sync::mpsc::SyncSender<PlaybackBuffer>>>;

pub struct PlaybackEngine {
    pub child: Child,
    pub writer_tx: std::sync::mpsc::SyncSender<PlaybackBuffer>,
    pub current_codec: String,
    pub width: u32,
    pub height: u32,
    pub render_rect: Option<String>,
    pub aspect_mode: String,
    pub dashboard_writer: SharedWriter,
    submission_tx: tokio::sync::mpsc::UnboundedSender<PlaybackSubmission>,
}

#[cfg(test)]
mod tests {
    use super::{append_kms_sink_args, encoded_pipeline_args, receiver_queue_ms, LatencyAckCadence};
    use std::time::{Duration, Instant};

    #[test]
    fn latency_acknowledgements_follow_elapsed_time_at_irregular_fps() {
        let start = Instant::now();
        let mut cadence = LatencyAckCadence::default();
        assert!(cadence.should_emit(start));
        assert!(!cadence.should_emit(start + Duration::from_millis(40)));
        assert!(!cadence.should_emit(start + Duration::from_millis(950)));
        assert!(cadence.should_emit(start + Duration::from_millis(1_250)));
        assert!(!cadence.should_emit(start + Duration::from_millis(2_100)));
        assert!(cadence.should_emit(start + Duration::from_millis(2_251)));
        cadence.reset();
        assert!(cadence.should_emit(start + Duration::from_millis(2_252)));
    }

    #[test]
    fn encoded_pipeline_sets_rk3399_pixel_aspect_before_decode() {
        let args = encoded_pipeline_args("h265");
        let capssetter = args.iter().position(|arg| arg == "capssetter").expect("capssetter");
        let decoder = args.iter().position(|arg| arg == "v4l2slh265dec").expect("decoder");
        assert!(capssetter < decoder);
        assert!(args.iter().any(|arg| arg.contains("pixel-aspect-ratio=(fraction)15/16")));
        assert!(args.iter().any(|arg| arg == "v4l2slh265dec"));
    }

    #[test]
    fn kms_sink_uses_the_complete_active_signal_rectangle() {
        let mut args = Vec::new();
        append_kms_sink_args(&mut args, "54", "33", Some("<0,0,3840,2160>"));
        assert!(args.iter().any(|arg| arg == "render-rectangle=<0,0,3840,2160>"));
    }

    #[test]
    fn receiver_queue_spans_complete_arrival_through_gstreamer_flush() {
        assert_eq!(receiver_queue_ms(1_000.0, 1_012.5), 12.5);
        assert_eq!(receiver_queue_ms(1_012.5, 1_000.0), 0.0);
    }
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
        let mut new_engine = start_persistent_playback(
            norm,
            connector,
            render_rect,
            aspect_mode,
            self.width,
            self.height,
            self.submission_tx.clone(),
        )?;
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

fn encoded_pipeline_args(norm_codec: &str) -> Vec<String> {
    let (parser, bitstream_caps, decoder) = if norm_codec == "h264" {
        ("h264parse", "video/x-h264", "v4l2slh264dec")
    } else {
        ("h265parse", "video/x-h265", "v4l2slh265dec")
    };
    vec![
        "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(),
        format!("blocksize={}", config::playback::RAW_PIPELINE_BLOCK_SIZE), "!".to_string(),
        parser.to_string(), format!("config-interval={}", config::playback::ENCODED_CONFIG_INTERVAL), "!".to_string(),
        // The RK3399 VOP cannot scale a codec-aligned 1920x1088 surface to
        // the complete 3840x2160 signal unless the decoder carries the HDMI
        // device pixel aspect. Set it before decode so the DMA-BUF feature is
        // retained all the way into kmssink.
        "capssetter".to_string(),
        format!("caps={bitstream_caps},pixel-aspect-ratio=(fraction)15/16"),
        "replace=false".to_string(), "!".to_string(),
        decoder.to_string(), "!".to_string(),
    ]
}

fn append_kms_sink_args(gst_args: &mut Vec<String>, connector: &str, plane: &str, render_rect: Option<&str>) {
    gst_args.extend([
        "kmssink".to_string(), "driver-name=rockchip".to_string(),
        format!("connector-id={connector}"), format!("plane-id={plane}"),
    ]);
    if let Some(rect) = render_rect {
        gst_args.push(format!("render-rectangle={rect}"));
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
    submission_tx: tokio::sync::mpsc::UnboundedSender<PlaybackSubmission>,
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
        println!("[PLAYBACK STARTUP] Initializing persistent GStreamer pipeline for {norm_codec} at t=0ms");
        gst_args.extend(encoded_pipeline_args(norm_codec));
    }
    append_kms_sink_args(&mut gst_args, connector, &plane, render_rect);
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
    let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<PlaybackBuffer>(queue_capacity);
    let writer_submission_tx = submission_tx.clone();
    std::thread::spawn(move || {
        while let Ok(buffer) = writer_rx.recv() {
            if stdin.write_all(&buffer.bytes).and_then(|_| stdin.flush()).is_err() {
                eprintln!("[PLAYBACK] GStreamer stdin write failed");
                break;
            }
            if let Some(timing) = buffer.timing {
                let receiver_queue_ms = receiver_queue_ms(timing.receiver_complete_time_ms, crate::clock::monotonic_epoch_ms());
                let _ = writer_submission_tx.send(PlaybackSubmission {
                    seq: timing.seq,
                    capture_time_ms: timing.capture_time_ms,
                    encode_duration_ms: timing.encode_duration_ms,
                    send_start_time_ms: timing.send_start_time_ms,
                    receiver_complete_time_ms: timing.receiver_complete_time_ms,
                    receiver_queue_ms,
                });
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
        submission_tx,
    })
}
