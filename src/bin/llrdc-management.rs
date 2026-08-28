#![deny(dead_code)]
#![forbid(unsafe_code)]

use llrdc_casting::{admin, cert, config, supervisor};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("admin") {
        let command = arguments.next();
        return admin::run_client(command.as_deref(), arguments.next().is_some()).await;
    }
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let settings = config::load_settings_at(std::path::Path::new(config::DEVICE_CONFIG_PATH)).unwrap_or_else(|_| config::ReceiverSettings::from_environment());
    std::env::set_var("CERTS_DIR", &settings.cert_dir);
    cert::get_or_create_identity().await.map_err(|error| std::io::Error::other(error.to_string()))?;
    let supervisor = supervisor::start().map_err(|error| std::io::Error::other(error.to_string()))?;
    let rotation_supervisor = supervisor.handle.clone();
    let certificate_path = std::path::Path::new(&settings.cert_dir).join("cert.pem");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if !cert::is_cert_valid(&certificate_path) {
                match cert::get_or_create_identity().await {
                    Ok(_) => {
                        rotation_supervisor.record_event("info", "certificate_rotated", "TLS certificate atomically rotated; receiver restart requested", true);
                        let _ = rotation_supervisor.restart("certificate_rotation").await;
                    }
                    Err(error) => rotation_supervisor.record_event("error", "certificate_rotation_failed", error.to_string(), true),
                }
            }
        }
    });
    let portal = admin::run_server(supervisor.handle.clone());
    tokio::select! {
        result = portal => result.map_err(|error| std::io::Error::other(error.to_string()))?,
        _ = tokio::signal::ctrl_c() => {},
    }
    Ok(())
}
