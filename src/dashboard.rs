/*
 * Idle Dashboard Module
 * Feeds live clock and IP dashboard frames directly to the idle KMS pipeline.
 */

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::playback::SharedWriter;

pub fn configured_dashboard_codec() -> String {
    match std::env::var("IDLE_DASHBOARD_MODE").ok().as_deref() {
        Some("hevc") | Some("h265") => "hevc".to_string(),
        _ => "raw".to_string(),
    }
}

pub fn raw_dashboard_dimensions(screen_w: u32, screen_h: u32) -> (u32, u32) {
    if screen_w <= 1920 {
        return (screen_w, screen_h);
    }

    let width = 1920;
    let height = ((screen_h as u64 * width as u64) / screen_w as u64) as u32;
    (width, height & !1)
}

pub struct RawDashboardFeeder {
    writer_tx: SharedWriter,
    width: u32,
    height: u32,
}

impl RawDashboardFeeder {
    pub fn new(width: u32, height: u32, writer_tx: SharedWriter) -> Self {
        Self { writer_tx, width, height }
    }

    pub fn push_frame(&mut self, vrefresh: u32) {
        let ips = crate::net::get_active_ipv4_addresses();
        let mut pixels = vec![0u32; (self.width * self.height) as usize];
        crate::text::draw_ip_dashboard_argb(&mut pixels, self.width, self.height, vrefresh, &ips);

        let raw_bytes = unsafe {
            std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4)
        };
        let frame = raw_bytes.to_vec();

        if let Ok(writer) = self.writer_tx.lock() {
            match writer.try_send(frame) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    eprintln!("[IDLE DASHBOARD] Dropping raw frame because GStreamer is backlogged");
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    eprintln!("[IDLE DASHBOARD] GStreamer raw pipeline disconnected");
                }
            }
        }
    }
}

pub struct PersistentDashboardEncoder {
    child: Child,
    stdin: std::process::ChildStdin,
    width: u32,
    height: u32,
}

impl PersistentDashboardEncoder {
    pub fn spawn(
        width: u32,
        height: u32,
        writer_tx: SharedWriter,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new("ffmpeg")
            .args([
                "-y",
                "-r", "1",
                "-f", "rawvideo",
                "-pixel_format", "bgra",
                "-video_size", &format!("{}x{}", width, height),
                "-i", "-",
                "-c:v", "libx265",
                "-preset", "ultrafast",
                "-tune", "zerolatency",
                "-flush_packets", "1",
                "-x265-params", "keyint=3600:bframes=0:no-scenecut=1",
                "-pix_fmt", "yuv420p",
                "-f", "hevc",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().ok_or("ffmpeg stdin failed")?;
        let mut stdout = child.stdout.take().ok_or("ffmpeg stdout failed")?;

        // Parallel reader thread: forwards stdout stream chunks directly to GStreamer writer_tx
        std::thread::spawn(move || {
            let mut buf = [0u8; 131072];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 { break; }
                if let Ok(writer) = writer_tx.lock() {
                    let _ = writer.send(buf[..n].to_vec());
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            width,
            height,
        })
    }

    pub fn push_frame(&mut self, vrefresh: u32) {
        let ips = crate::net::get_active_ipv4_addresses();
        let mut pixels = vec![0u32; (self.width * self.height) as usize];
        crate::text::draw_ip_dashboard_argb(&mut pixels, self.width, self.height, vrefresh, &ips);

        let raw_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4)
        };

        if self.stdin.write_all(raw_bytes).and_then(|_| self.stdin.flush()).is_err() {
            eprintln!("[IDLE ENCODER] Pipe write to ffmpeg stdin failed");
        }
    }
}

impl Drop for PersistentDashboardEncoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_idle_dashboard_thread(
    screen_w: u32,
    screen_h: u32,
    vrefresh: u32,
    idle_active: Arc<AtomicBool>,
    writer_tx: SharedWriter,
) {
    let dashboard_codec = configured_dashboard_codec();
    std::thread::spawn(move || {
        let mut last_secs = 0u64;
        let mut raw_feeder = if dashboard_codec == "raw" {
            Some(RawDashboardFeeder::new(screen_w, screen_h, writer_tx.clone()))
        } else {
            None
        };
        let mut encoder: Option<PersistentDashboardEncoder> = None;
        loop {
            if !idle_active.load(Ordering::Relaxed) {
                if dashboard_codec == "hevc" && encoder.is_none() {
                    println!("[IDLE THREAD] Spawning fallback {}x{} HEVC dashboard encoder...", screen_w, screen_h);
                    encoder = PersistentDashboardEncoder::spawn(screen_w, screen_h, writer_tx.clone()).ok();
                }

                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if now_secs != last_secs {
                    last_secs = now_secs;
                    if let Some(feeder) = raw_feeder.as_mut() {
                        feeder.push_frame(vrefresh);
                    } else if let Some(enc) = encoder.as_mut() {
                        enc.push_frame(vrefresh);
                    }
                }
            } else if encoder.is_some() {
                println!("[IDLE THREAD] Streaming active; terminating fallback idle HEVC encoder process.");
                encoder = None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}
