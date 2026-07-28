/*
 * Modular TLS Certificate & Identity Management Module
 * Handles loading, validity & expiration checks, auto-generation,
 * and persistent storage of TLS certificates for WebTransport and HTTPS.
 */

use std::error::Error;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};
use wtransport::Identity;

pub fn get_cert_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CERTS_DIR") {
        let p = PathBuf::from(dir);
        if fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    let system_certs = PathBuf::from("/certs");
    if system_certs.exists() || fs::create_dir_all(&system_certs).is_ok() {
        return system_certs;
    }
    PathBuf::from(".")
}

pub fn get_cert_and_key_paths() -> (PathBuf, PathBuf) {
    let dir = get_cert_dir();
    (dir.join("cert.pem"), dir.join("key.pem"))
}

/// Check if certificate exists at `cert_path` and remains valid (not expired)
pub fn is_cert_valid(cert_path: &Path) -> bool {
    if !cert_path.exists() {
        return false;
    }

    let file = match fs::File::open(cert_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = BufReader::new(file);

    let certs = match rustls_pemfile::certs(&mut reader) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let cert_der = match certs.first() {
        Some(c) => c,
        None => return false,
    };

    let (_, parsed_cert) = match x509_parser::parse_x509_certificate(cert_der) {
        Ok(res) => res,
        Err(_) => return false,
    };

    let not_after = parsed_cert.validity().not_after;
    let not_after_secs = not_after.timestamp();
    let now_secs = OffsetDateTime::now_utc().unix_timestamp();

    // Consider certificate expired if less than 24 hours remain
    let buffer_secs = 24 * 3600;
    if now_secs + buffer_secs >= not_after_secs {
        println!("[CERT] Certificate at {:?} is expired or expires within 24h (not_after={}, now={})", cert_path, not_after_secs, now_secs);
        return false;
    }

    let remaining_days = (not_after_secs - now_secs) / 86400;
    println!("[CERT] Certificate at {:?} is valid for another {} days.", cert_path, remaining_days);
    true
}

pub async fn get_or_create_identity() -> Result<Identity, Box<dyn Error + Send + Sync>> {
    let (cert_path, key_path) = get_cert_and_key_paths();

    let need_generate = !is_cert_valid(&cert_path) || !key_path.exists();

    if need_generate {
        println!("[CERT] Generating new persistent TLS certificate at {:?}...", cert_path);
        let subject_alt_names = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "192.168.1.72".to_string(),
            "0.0.0.0".to_string(),
        ];
        let key_pair = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(subject_alt_names)?;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(13); // WebTransport spec limit: max 14 days

        let cert = params.self_signed(&key_pair)?;
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        fs::write(&cert_path, cert_pem)?;
        fs::write(&key_path, key_pem)?;
        println!("[CERT] Successfully saved TLS cert to {:?} and {:?}", cert_path, key_path);
    } else {
        println!("[CERT] Reusing existing persistent TLS certificate from {:?}", cert_path);
    }

    let identity = Identity::load_pemfiles(&cert_path, &key_path).await?;
    Ok(identity)
}

pub fn extract_cert_hash_hex(identity: &Identity) -> String {
    if let Some(cert) = identity.certificate_chain().as_slice().first() {
        let hash = cert.hash();
        let hash_bytes = hash.as_ref();
        hash_bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":")
    } else {
        String::new()
    }
}
