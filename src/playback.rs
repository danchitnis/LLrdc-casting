/*
 * Playback Pipeline Module
 * Manages DRM KMS geometry autodetection and persistent GStreamer playback process.
 */

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

pub struct PlaybackEngine {
    pub child: Child,
    pub writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    pub current_codec: String,
}

impl PlaybackEngine {
    pub fn ensure_codec(
        &mut self,
        target_codec: &str,
        connector: &str,
        render_rect: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let norm = if target_codec.to_lowercase().contains("264") { "h264" } else { "h265" };
        if self.current_codec == norm {
            return Ok(());
        }
        println!("[PLAYBACK SWITCH] Switching GStreamer pipeline from {} to {}", self.current_codec, norm);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let new_engine = start_persistent_playback(norm, connector, render_rect)?;
        *self = new_engine;
        Ok(())
    }
}

pub fn autodetect_display_info() -> (u32, u32, u32, String, Option<String>, crate::drm_kms::EdidInfo) {
    if let Ok(card) = crate::drm_kms::open_display_card() {
        if let Ok((screen_w, screen_h, mode, conn_handle, _, edid_info)) = crate::drm_kms::autodetect_display_mode(&card) {
            let conn_id = u32::from(conn_handle).to_string();
            let target_w = screen_w.min(screen_h * 16 / 9);
            let target_h = screen_h.min(screen_w * 9 / 16);
            let offset_x = (screen_w - target_w) / 2;
            let offset_y = (screen_h - target_h) / 2;
            let rect = format!("<{},{},{},{}>", offset_x, offset_y, target_w, target_h);
            let refresh = mode.vrefresh() as u32;
            crate::drm_kms::drop_master(&card);
            drop(card);
            println!("[DISPLAY INFO] Auto-detected HDMI Connector {}, {}x{}@{}Hz, name='{}', rect={}", conn_id, screen_w, screen_h, refresh, edid_info.name, rect);
            return (screen_w, screen_h, refresh, conn_id, Some(rect), edid_info);
        }
        crate::drm_kms::drop_master(&card);
    }
    (1920, 1080, 60, "54".into(), None, crate::drm_kms::EdidInfo::default())
}

pub fn start_persistent_playback(
    codec: &str,
    connector: &str,
    render_rect: Option<&str>,
) -> Result<PlaybackEngine, Box<dyn std::error::Error>> {
    let t_start = std::time::Instant::now();
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let codec_lower = codec.to_lowercase();
    let norm_codec = if codec_lower.contains("264") { "h264" } else { "h265" };
    let (parser, decoder) = if norm_codec == "h264" {
        ("h264parse", "v4l2slh264dec")
    } else {
        ("h265parse", "v4l2slh265dec")
    };
    println!("[PLAYBACK STARTUP] Initializing persistent GStreamer pipeline for {norm_codec} ({parser} -> {decoder}) at t=0ms");

    let mut gst_args = vec![
        "-q".to_string(), "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(), "blocksize=65536".to_string(), "!".to_string(),
        parser.to_string(), "config-interval=-1".to_string(), "!".to_string(),
        decoder.to_string(), "!".to_string(),
        "kmssink".to_string(), "driver-name=rockchip".to_string(),
        format!("connector-id={connector}"), format!("plane-id={plane}"),
    ];
    if let Some(rect) = render_rect {
        gst_args.push(format!("render-rectangle={rect}"));
    }
    gst_args.extend([
        "force-modesetting=false".to_string(), "can-scale=true".to_string(),
        "sync=false".to_string(), "async=false".to_string(), "skip-vsync=true".to_string(), "max-lateness=0".to_string(),
    ]);

    println!("[PLAYBACK STARTUP] Spawning continuous gst-launch-1.0 process...");
    let mut child = Command::new("gst-launch-1.0")
        .env("GST_DEBUG", "v4l2slh265dec:4,h265parse:4,v4l2slh264dec:4,h264parse:4,kmssink:4")
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

    let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        while let Ok(access_unit) = writer_rx.recv() {
            if stdin.write_all(&access_unit).and_then(|_| stdin.flush()).is_err() {
                eprintln!("[PLAYBACK] GStreamer stdin write failed");
                break;
            }
        }
    });

    println!("[PLAYBACK READY] Persistent GStreamer pipeline active on HDMI connector {connector}, plane {plane}");
    Ok(PlaybackEngine {
        child,
        writer_tx,
        current_codec: norm_codec.to_string(),
    })
}
