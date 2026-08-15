use std::error::Error;

use tokio::io::AsyncWriteExt;
use wtransport::SendStream;

use crate::config::packet::MAX_UI_BYTES;

const UI_REQUEST_TYPE: &str = "get_ui";

pub fn is_ui_request(payload: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.get("type").and_then(serde_json::Value::as_str).map(str::to_owned))
        .as_deref()
        == Some(UI_REQUEST_TYPE)
}

pub async fn send_embedded_ui(
    stream: &mut SendStream,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let html = crate::http_server::embedded_index_html().as_bytes();
    if html.len() > MAX_UI_BYTES {
        return Err("embedded UI is larger than the WebTransport UI limit".into());
    }
    let length = u32::try_from(html.len())?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(html).await?;
    stream.flush().await?;
    stream.finish().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_ui_request;

    #[test]
    fn recognizes_only_ui_request_messages() {
        assert!(is_ui_request(br#"{"type":"get_ui"}"#));
        assert!(!is_ui_request(br#"{"type":"get_status"}"#));
        assert!(!is_ui_request(b"not json"));
    }
}
