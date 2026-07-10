//! Shared blocking HTTP helper (RT-026) — no curl subprocess.

use crate::StorageError;

/// GET/PUT/DELETE with optional body. Returns (status, body bytes).
pub fn request(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    extra_headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>), StorageError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| StorageError::Http(format!("http client: {e}")))?;

    let mut builder = match method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "PUT" => client.put(url),
        "POST" => client.post(url),
        "DELETE" => client.delete(url),
        "HEAD" => client.head(url),
        other => {
            return Err(StorageError::Http(format!("unsupported method {other}")));
        }
    };

    for (k, v) in extra_headers {
        builder = builder.header(*k, *v);
    }

    if let Some(b) = body {
        builder = builder
            .header("Content-Type", "application/octet-stream")
            .body(b.to_vec());
    }

    let resp = builder
        .send()
        .map_err(|e| StorageError::Http(format!("{method} {url}: {e}")))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .map_err(|e| StorageError::Http(format!("read body: {e}")))?
        .to_vec();
    Ok((status, bytes))
}
