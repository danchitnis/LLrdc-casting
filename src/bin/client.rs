/*
 * Safe Rust WebTransport QUIC UDP Dev Client
 * Connects to the Rock 5C+ board and sends a static H.264 frame.
 */

use std::error::Error;
use std::time::Duration;
use wtransport::ClientConfig;
use wtransport::Endpoint;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let target_ip = args.get(1).map(|s| s.as_str()).unwrap_or("192.168.1.72");
    let target_port = args.get(2).map(|s| s.as_str()).unwrap_or("4433");
    let url = format!("https://{}:{}", target_ip, target_port);

    println!("=====================================================");
    println!(" WebTransport QUIC H.264 Dev Client (Workstation)");
    println!(" Target Server: {}", url);
    println!("=====================================================\n");

    // Static H.264 Annex-B NAL payload (SPS, PPS, IDR Frame slice)
    let h264_payload: &[u8] = &[
        // NAL 1: SPS
        0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0x01, 0x40, 0x16,
        0xec, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x0f, 0x23, 0xc6, 0x0c, 0x65,
        // NAL 2: PPS
        0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x38, 0x80,
        // NAL 3: IDR Keyframe Slice
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x10, 0xff, 0xfe, 0xf6,
        0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
        0x00, 0x40, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
    ];

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_native_certs()
        .build();

    let client = Endpoint::client(config)?;

    println!("[CLIENT] Connecting to {} via WebTransport QUIC over UDP...", url);
    let connection = client.connect(url).await?;
    println!("[CLIENT SUCCESS] Connected to Rock 5C+ Server!");

    println!("[CLIENT] Opening WebTransport unidirectional stream...");
    let mut send_stream = connection.open_uni().await?.await?;

    println!("[CLIENT] Transmitting static H.264 frame ({} bytes)...", h264_payload.len());
    send_stream.write_all(h264_payload).await?;
    send_stream.finish().await?;
    println!("[CLIENT SUCCESS] H.264 frame stream transmission complete!");

    // Send datagram as well
    println!("[CLIENT] Sending low-latency datagram packet...");
    connection.send_datagram(h264_payload)?;
    println!("[CLIENT SUCCESS] Datagram transmitted successfully!");

    tokio::time::sleep(Duration::from_secs(1)).await;
    println!("[CLIENT] Done.");
    Ok(())
}
