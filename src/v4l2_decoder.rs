//! Bounded access-unit reassembly for the direct RKMPP playback path.
//!
//! Compressed network data is the only video data kept in normal RAM. Decoded
//! pixels never enter Rust memory: `mpp_decoder` exports them as DMA-BUFs.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const HEADER_LEN: usize = 16;
const CHUNK_BYTES: usize = 1350;
const MAX_ACCESS_UNIT_BYTES: usize = 4 * 1024 * 1024;
const MAX_CHUNKS: usize = (MAX_ACCESS_UNIT_BYTES + CHUNK_BYTES - 1) / CHUNK_BYTES;
const MAX_IN_FLIGHT: usize = 4;
const ASSEMBLY_TTL: Duration = Duration::from_millis(35);

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
    ASSEMBLIES.lock().expect("assembly mutex poisoned").clear();
}

pub fn process_udp_chunk(packet: &[u8]) -> Option<VideoFrame> {
    if packet.len() <= HEADER_LEN { return None; }
    let codec = match &packet[..4] {
        b"H265" | b"HEVC" => "hevc",
        b"H264" | b"VIDC" => "h264",
        _ => return None,
    };
    let seq = u32::from_be_bytes(packet[4..8].try_into().ok()?);
    let chunk_index = u16::from_be_bytes(packet[8..10].try_into().ok()?);
    let total_chunks = u16::from_be_bytes(packet[10..12].try_into().ok()?);
    let width = u16::from_be_bytes(packet[12..14].try_into().ok()?);
    let height = u16::from_be_bytes(packet[14..16].try_into().ok()?);
    if total_chunks == 0 || total_chunks as usize > MAX_CHUNKS || chunk_index >= total_chunks || width == 0 || height == 0 {
        return None;
    }
    let payload = &packet[HEADER_LEN..];
    if payload.len() > CHUNK_BYTES { return None; }

    let now = Instant::now();
    let mut assemblies = ASSEMBLIES.lock().expect("assembly mutex poisoned");
    assemblies.retain(|entry| now.duration_since(entry.first_packet_at) <= ASSEMBLY_TTL);
    if let Some(index) = assemblies.iter().position(|entry| entry.seq == seq) {
        let entry = &mut assemblies[index];
        if entry.total_chunks != total_chunks || entry.width != width || entry.height != height || entry.codec != codec { return None; }
        let slot = &mut entry.chunks[chunk_index as usize];
        if slot.is_none() {
            *slot = Some(payload.to_vec());
            entry.received += 1;
        }
    } else {
        if assemblies.len() == MAX_IN_FLIGHT { assemblies.remove(0); }
        let mut chunks = (0..total_chunks).map(|_| None).collect::<Vec<_>>();
        chunks[chunk_index as usize] = Some(payload.to_vec());
        assemblies.push(Assembly { seq, total_chunks, width, height, codec, chunks, received: 1, first_packet_at: now });
    }

    let completed_index = assemblies.iter().position(|entry| entry.seq == seq && entry.received == entry.total_chunks as usize)?;
    let completed = assemblies.remove(completed_index);
    let actual_len: usize = completed.chunks.iter().map(|chunk| chunk.as_ref().map_or(0, Vec::len)).sum();
    if actual_len == 0 || actual_len > MAX_ACCESS_UNIT_BYTES { return None; }
    let mut access_unit = Vec::with_capacity(actual_len);
    for chunk in completed.chunks { access_unit.extend_from_slice(chunk.as_deref()?); }
    Some(VideoFrame { seq: completed.seq, width: completed.width as u32, height: completed.height as u32, codec: completed.codec.into(), access_unit, first_packet_at: completed.first_packet_at })
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
