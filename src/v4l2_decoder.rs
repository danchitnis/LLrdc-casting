//! Bounded access-unit reassembly for the direct RKMPP playback path.
//!
//! Compressed network data is the only video data kept in normal RAM. Decoded
//! pixels never enter Rust memory: `mpp_decoder` exports them as DMA-BUFs.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::config::packet::{
    CHUNK_BYTES, H264_MAX_HEIGHT, H264_MAX_WIDTH, H265_MAX_HEIGHT, H265_MAX_WIDTH,
    MAX_ACCESS_UNIT_BYTES, MAX_IN_FLIGHT_ACCESS_UNITS, PACKET_HEADER_BYTES,
};
use crate::config::transport::DATAGRAM_TAG_BYTES;

const HEADER_LEN: usize = PACKET_HEADER_BYTES;
const MAX_CHUNKS: usize = (MAX_ACCESS_UNIT_BYTES + CHUNK_BYTES - 1) / CHUNK_BYTES;
const MAX_IN_FLIGHT: usize = MAX_IN_FLIGHT_ACCESS_UNITS;
const ASSEMBLY_TTL: Duration = crate::config::packet::ACCESS_UNIT_ASSEMBLY_TTL;

static LAST_COMPLETED_SEQ: AtomicU32 = AtomicU32::new(0);
static STATS_CHUNKS: AtomicU64 = AtomicU64::new(0);
static STATS_COMPLETED: AtomicU64 = AtomicU64::new(0);
static STATS_DROPPED_TIMEOUT: AtomicU64 = AtomicU64::new(0);
static STATS_DROPPED_EVICTED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub seq: u32,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    pub access_unit: Vec<u8>,
    pub first_packet_at: Instant,
}

struct Assembly {
    seq: u32,
    total_chunks: u16,
    width: u16,
    height: u16,
    codec: &'static str,
    chunks: Vec<Option<Vec<u8>>>,
    received: usize,
    first_packet_at: Instant,
}

static ASSEMBLIES: LazyLock<Mutex<Vec<Assembly>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub fn reset_decoder_pipeline() {
    LAST_COMPLETED_SEQ.store(0, Ordering::Relaxed);
    if let Ok(mut assemblies) = ASSEMBLIES.lock() {
        assemblies.clear();
    }
}

pub fn process_udp_chunk(packet: &[u8]) -> Option<VideoFrame> {
    if packet.len() >= 4 && &packet[..4] == b"STOP" {
        reset_decoder_pipeline();
        return Some(VideoFrame {
            seq: 0,
            width: 0,
            height: 0,
            codec: "stop".to_string(),
            access_unit: Vec::new(),
            first_packet_at: Instant::now(),
        });
    }
    if packet.len() <= HEADER_LEN { return None; }
    STATS_CHUNKS.fetch_add(1, Ordering::Relaxed);

    let codec = match &packet[..DATAGRAM_TAG_BYTES] {
        b"H265" | b"HEVC" => "hevc",
        b"H264" | b"VIDC" => "h264",
        _ => return None,
    };
    let seq = u32::from_be_bytes(
        packet[DATAGRAM_TAG_BYTES..DATAGRAM_TAG_BYTES + 4]
            .try_into()
            .ok()?,
    );

    // Drop stale chunks from older already-completed frames, but handle stream sequence resets
    let last_seq = LAST_COMPLETED_SEQ.load(Ordering::Relaxed);
    if seq <= last_seq && last_seq > 0 {
        if seq <= 5 || seq < last_seq {
            // Sequence reset detected (e.g. new client or new stream restart); reset sequence counter
            reset_decoder_pipeline();
        } else {
            return None;
        }
    }

    let chunk_index_offset = DATAGRAM_TAG_BYTES + 4;
    let total_chunks_offset = chunk_index_offset + 2;
    let width_offset = total_chunks_offset + 2;
    let height_offset = width_offset + 2;
    let chunk_index = u16::from_be_bytes(packet[chunk_index_offset..chunk_index_offset + 2].try_into().ok()?);
    let total_chunks = u16::from_be_bytes(packet[total_chunks_offset..total_chunks_offset + 2].try_into().ok()?);
    let width = u16::from_be_bytes(packet[width_offset..width_offset + 2].try_into().ok()?);
    let height = u16::from_be_bytes(packet[height_offset..height_offset + 2].try_into().ok()?);
    if total_chunks == 0 || total_chunks as usize > MAX_CHUNKS || chunk_index >= total_chunks || width == 0 || height == 0 {
        return None;
    }
    let (max_width, max_height) = if codec == "h264" {
        (H264_MAX_WIDTH, H264_MAX_HEIGHT)
    } else {
        (H265_MAX_WIDTH, H265_MAX_HEIGHT)
    };
    if width as usize > max_width || height as usize > max_height {
        return None;
    }
    let payload = &packet[HEADER_LEN..];
    if payload.len() > MAX_ACCESS_UNIT_BYTES { return None; }

    let now = Instant::now();
    let mut assemblies = ASSEMBLIES.lock().expect("assembly mutex poisoned");

    if seq == 1 && chunk_index == 0 {
        println!("[PROBE RECV] seq=1 first chunk arrived at {:?}", Instant::now());
    }

    // Retain non-expired assemblies and count dropped timeouts
    let initial_len = assemblies.len();
    assemblies.retain(|entry| now.duration_since(entry.first_packet_at) <= ASSEMBLY_TTL && entry.seq > last_seq);
    let expired_count = (initial_len - assemblies.len()) as u64;
    if expired_count > 0 {
        STATS_DROPPED_TIMEOUT.fetch_add(expired_count, Ordering::Relaxed);
    }

    if let Some(index) = assemblies.iter().position(|entry| entry.seq == seq) {
        let entry = &mut assemblies[index];
        if entry.total_chunks != total_chunks || entry.width != width || entry.height != height || entry.codec != codec { return None; }
        let slot = &mut entry.chunks[chunk_index as usize];
        if slot.is_none() {
            *slot = Some(payload.to_vec());
            entry.received += 1;
        }
    } else {
        if assemblies.len() == MAX_IN_FLIGHT {
            assemblies.remove(0);
            STATS_DROPPED_EVICTED.fetch_add(1, Ordering::Relaxed);
        }
        let mut chunks = (0..total_chunks).map(|_| None).collect::<Vec<_>>();
        chunks[chunk_index as usize] = Some(payload.to_vec());
        assemblies.push(Assembly { seq, total_chunks, width, height, codec, chunks, received: 1, first_packet_at: now });
    }

    let completed_index = assemblies.iter().position(|entry| entry.seq == seq && entry.received == entry.total_chunks as usize)?;
    let completed = assemblies.remove(completed_index);

    // Update last completed sequence number and purge all older stale assemblies
    LAST_COMPLETED_SEQ.store(completed.seq, Ordering::Relaxed);
    assemblies.retain(|entry| entry.seq > completed.seq);

    let actual_len: usize = completed.chunks.iter().map(|chunk| chunk.as_ref().map_or(0, Vec::len)).sum();
    if actual_len == 0 || actual_len > MAX_ACCESS_UNIT_BYTES { return None; }
    let mut access_unit = Vec::with_capacity(actual_len);
    for chunk in completed.chunks { access_unit.extend_from_slice(chunk.as_deref()?); }

    let completed_count = STATS_COMPLETED.fetch_add(1, Ordering::Relaxed) + 1;
    if completed_count == 1 || completed_count % 60 == 0 {
        let chunks_cnt = STATS_CHUNKS.load(Ordering::Relaxed);
        let timeout_cnt = STATS_DROPPED_TIMEOUT.load(Ordering::Relaxed);
        let evicted_cnt = STATS_DROPPED_EVICTED.load(Ordering::Relaxed);
        let total_attempts = completed_count + timeout_cnt + evicted_cnt;
        let rate = if total_attempts > 0 {
            (completed_count as f64 / total_attempts as f64)
                * crate::config::telemetry::DEFAULT_DELIVERY_RATE_PERCENT as f64
        } else {
            crate::config::telemetry::DEFAULT_DELIVERY_RATE_PERCENT as f64
        };
        println!("[FRAME INTEGRITY] Completed={completed_count} DroppedTimeout={timeout_cnt} Evicted={evicted_cnt} TotalChunks={chunks_cnt} DeliveryRate={rate:.1}%");
    }

    let frame = VideoFrame { seq: completed.seq, width: completed.width as u32, height: completed.height as u32, codec: completed.codec.into(), access_unit, first_packet_at: completed.first_packet_at };
    validate_access_unit_bitstream(&frame);
    Some(frame)
}

fn validate_access_unit_bitstream(frame: &VideoFrame) {
    let buf = &frame.access_unit;
    if buf.len() < 4 {
        eprintln!("[BITSTREAM ERROR] seq={} access unit length too short ({} bytes)", frame.seq, buf.len());
        return;
    }

    let mut nal_types = Vec::new();
    let mut i = 0;
    let mut has_start_code = false;

    while i <= buf.len().saturating_sub(4) {
        let is_start_4 = buf[i] == 0 && buf[i+1] == 0 && buf[i+2] == 0 && buf[i+3] == 1;
        let is_start_3 = buf[i] == 0 && buf[i+1] == 0 && buf[i+2] == 1;

        if is_start_4 || is_start_3 {
            has_start_code = true;
            let header_offset = if is_start_4 { i + 4 } else { i + 3 };
            if header_offset < buf.len() {
                let header_byte = buf[header_offset];
                let nal_type = if frame.codec == "hevc" {
                    (header_byte >> 1) & 0x3f
                } else {
                    header_byte & 0x1f
                };
                let name = match (frame.codec.as_str(), nal_type) {
                    ("hevc", 32) => "VPS",
                    ("hevc", 33) => "SPS",
                    ("hevc", 34) => "PPS",
                    ("hevc", 35) => "AUD",
                    ("hevc", 19) | ("hevc", 20) => "IDR",
                    ("hevc", 21) => "CRA",
                    ("hevc", 0) | ("hevc", 1) => "P-SLICE",
                    ("h264", 7) => "SPS",
                    ("h264", 8) => "PPS",
                    ("h264", 9) => "AUD",
                    ("h264", 5) => "IDR",
                    ("h264", 1) => "P-SLICE",
                    _ => "NAL",
                };
                nal_types.push(format!("{}({})", name, nal_type));
            }
            i = header_offset;
        } else {
            i += 1;
        }
    }

    let nal_summary = nal_types.join(", ");
    let is_key = nal_types.iter().any(|t| t.starts_with("IDR") || t.starts_with("CRA"));
    let has_vps = nal_types.iter().any(|t| t.starts_with("VPS"));

    if frame.seq == 1 || frame.seq % 60 == 0 || !has_start_code || (is_key && frame.codec == "hevc" && !has_vps) {
        let latency_us = Instant::now().duration_since(frame.first_packet_at).as_micros();
        println!(
            "[BITSTREAM VALIDATOR] seq={} ({}, {}x{}, {}B, reassembly={}us) | NALs: [{}] | ValidStartCode: {}",
            frame.seq,
            if is_key { "KEYFRAME" } else { "DELTA" },
            frame.width, frame.height,
            buf.len(),
            latency_us,
            nal_summary,
            if has_start_code { "YES" } else { "NO" }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(seq: u32, index: u16, total: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = b"H265".to_vec(); out.extend_from_slice(&seq.to_be_bytes()); out.extend_from_slice(&index.to_be_bytes()); out.extend_from_slice(&total.to_be_bytes()); out.extend_from_slice(&3840u16.to_be_bytes()); out.extend_from_slice(&2160u16.to_be_bytes()); out.extend_from_slice(payload); out
    }
    #[test]
    fn reassembles_out_of_order_without_duplicate_bytes() {
        reset_decoder_pipeline();
        assert!(process_udp_chunk(&packet(7, 1, 2, b"world")).is_none());
        assert!(process_udp_chunk(&packet(7, 1, 2, b"world")).is_none());
        let frame = process_udp_chunk(&packet(7, 0, 2, b"hello ")).unwrap();
        assert_eq!(frame.access_unit, b"hello world");
    }
}
