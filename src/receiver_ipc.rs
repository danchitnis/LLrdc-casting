use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::admin_protocol::{ReceiverRequest, ReceiverResponse, PROTOCOL_VERSION, RECEIVER_SOCKET_PATH};
use crate::control::ControlCommand;
use crate::local_pairing::PairingState;
use crate::management::ManagementState;

pub async fn run(
    pairing: PairingState,
    management: ManagementState,
    commands: mpsc::Sender<ControlCommand>,
    ready: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = Path::new(RECEIVER_SOCKET_PATH);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    loop {
        let (stream, _) = listener.accept().await?;
        let pairing = pairing.clone();
        let management = management.clone();
        let commands = commands.clone();
        let ready = Arc::clone(&ready);
        tokio::spawn(async move {
            if let Err(error) = handle(stream, pairing, management, commands, ready).await {
                eprintln!("[RECEIVER IPC] request failed: {error}");
            }
        });
    }
}

async fn handle(
    stream: UnixStream,
    pairing: PairingState,
    management: ManagementState,
    commands: mpsc::Sender<ControlCommand>,
    ready: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut stream = reader.into_inner();
    let response = match serde_json::from_str::<ReceiverRequest>(line.trim()) {
        Err(_) => ReceiverResponse::Error { version: PROTOCOL_VERSION, code: "invalid_request".into() },
        Ok(request) if request.version() != PROTOCOL_VERSION => ReceiverResponse::Error { version: PROTOCOL_VERSION, code: "unsupported_version".into() },
        Ok(ReceiverRequest::Ping { .. }) => ReceiverResponse::Pong { version: PROTOCOL_VERSION, ready: ready.load(Ordering::Relaxed) },
        Ok(ReceiverRequest::Snapshot { .. }) => ReceiverResponse::Snapshot {
            version: PROTOCOL_VERSION,
            ready: ready.load(Ordering::Relaxed),
            management: management.snapshot(),
            pairing: pairing.snapshot(),
        },
        Ok(ReceiverRequest::StopSharing { .. }) => {
            let _ = commands.send(ControlCommand::AdminStop).await;
            ReceiverResponse::Ack { version: PROTOCOL_VERSION }
        }
        Ok(ReceiverRequest::Shutdown { reason, .. }) => {
            let _ = commands.send(ControlCommand::Shutdown { reason }).await;
            ReceiverResponse::Ack { version: PROTOCOL_VERSION }
        }
        Ok(ReceiverRequest::PairingCode { .. }) => match pairing.snapshot().code {
            Some(code) => ReceiverResponse::PairingCode { version: PROTOCOL_VERSION, code },
            None => ReceiverResponse::Error { version: PROTOCOL_VERSION, code: "pairing_code_unavailable".into() },
        },
    };
    stream.write_all(&serde_json::to_vec(&response)?).await?;
    stream.write_all(b"\n").await?;
    stream.shutdown().await?;
    Ok(())
}

pub async fn request(request: &ReceiverRequest) -> Result<ReceiverResponse, Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = tokio::time::timeout(std::time::Duration::from_secs(2), UnixStream::connect(RECEIVER_SOCKET_PATH)).await??;
    stream.write_all(&serde_json::to_vec(request)?).await?;
    stream.write_all(b"\n").await?;
    stream.shutdown().await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_line(&mut line)).await??;
    Ok(serde_json::from_str(line.trim())?)
}
