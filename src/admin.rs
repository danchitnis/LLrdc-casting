use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::local_pairing::PairingState;

const ADMIN_SOCKET_PATH: &str = "/run/llrdc-casting-admin.sock";

pub async fn run_server(pairing_state: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_path = Path::new(ADMIN_SOCKET_PATH);
    match std::fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;

    loop {
        let (stream, _) = listener.accept().await?;
        let state = pairing_state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_request(stream, state).await {
                eprintln!("[ADMIN SOCKET] Request failed: {error}");
            }
        });
    }
}

async fn handle_request(
    stream: UnixStream,
    pairing_state: PairingState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    reader.read_line(&mut request).await?;
    let mut stream = reader.into_inner();

    if request.trim() != "pairing-code" {
        stream.write_all(b"ERROR invalid admin command\n").await?;
        return Ok(());
    }

    let snapshot = pairing_state.snapshot();
    if let Some(code) = snapshot.code {
        stream.write_all(code.as_bytes()).await?;
        stream.write_all(b"\n").await?;
    } else {
        stream.write_all(b"ERROR pairing code unavailable\n").await?;
    }
    Ok(())
}

pub async fn run_client(command: Option<&str>, has_extra_arguments: bool) -> Result<(), Box<dyn std::error::Error>> {
    if command != Some("pairing-code") || has_extra_arguments {
        return Err("usage: llrdc-casting admin pairing-code".into());
    }

    let mut stream = UnixStream::connect(ADMIN_SOCKET_PATH).await?;
    stream.write_all(b"pairing-code\n").await?;
    stream.shutdown().await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8(response)?;
    let response = response.trim();
    if let Some(error) = response.strip_prefix("ERROR ") {
        return Err(error.to_string().into());
    }
    if response.len() != 4 || !response.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("admin socket returned an invalid pairing code".into());
    }
    println!("{response}");
    Ok(())
}
