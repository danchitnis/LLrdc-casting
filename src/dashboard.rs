/*
 * Idle Dashboard Module
 * Encodes live clock and IP dashboard frames using ffmpeg (x265) when no client stream is active.
 */

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
        writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
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
            use std::io::Read;
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 { break; }
                if writer_tx.send(buf[..n].to_vec()).is_err() {
                    break;
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
    writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
) {
    std::thread::spawn(move || {
        let mut last_secs = 0u64;
        let mut encoder: Option<PersistentDashboardEncoder> = None;
        loop {
            if !idle_active.load(Ordering::Relaxed) {
                if encoder.is_none() {
                    println!("[IDLE THREAD] Spawning persistent native {}x{} HEVC dashboard encoder...", screen_w, screen_h);
                    encoder = PersistentDashboardEncoder::spawn(screen_w, screen_h, writer_tx.clone()).ok();
                }

                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if now_secs != last_secs {
                    last_secs = now_secs;
                    if let Some(enc) = encoder.as_mut() {
                        enc.push_frame(vrefresh);
                    }
                }
            } else if encoder.is_some() {
                println!("[IDLE THREAD] Streaming active; terminating idle HEVC dashboard encoder process.");
                encoder = None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}
