use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto::{b64_decode, decrypt_attrs, decrypt_chunk, derive_key, derive_nonce};

const API_URL: &str = "https://g.api.mega.co.nz";

/// Chunk buffer size for download (1 MiB).
const CHUNK_SIZE: u64 = 1024 * 1024;

pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub download_url: String,
    pub key: [u8; 16],
    pub nonce: [u8; 8],
}

/// Parse a MEGA public file link into (file_id, key_str).
///
/// Supported formats:
///   https://mega.nz/#!<file_id>!<key>
///   https://mega.nz/file/<file_id>#<key>
pub fn parse_link(link: &str) -> Result<(String, String), String> {
    // Format: /#!<id>!<key>
    if let Some(after) = link.split("/#!").nth(1) {
        let mut parts = after.splitn(2, '!');
        let id = parts.next().ok_or("Missing file ID in link")?;
        let key = parts.next().ok_or("Missing file key in link")?;
        let key = key.split(['?', '#', '&']).next().unwrap_or(key);
        return Ok((id.to_string(), key.to_string()));
    }

    // Format: /file/<id>#<key>
    if let Some(after) = link.split("/file/").nth(1) {
        let mut parts = after.splitn(2, '#');
        let id = parts.next().ok_or("Missing file ID in link")?;
        let key = parts.next().ok_or("Missing file key in link")?;
        let key = key.split(['?', '&']).next().unwrap_or(key);
        return Ok((id.to_string(), key.to_string()));
    }

    Err(format!("Unrecognised MEGA link format: {link}"))
}

/// Fetch file metadata and a temporary download URL from the MEGA API.
///
/// Makes a single API call with {"a":"g","g":"1","p":"<id>"} which returns
/// the file size, encrypted attributes, and a CDN download URL.
pub fn get_file_info(file_id: &str, key_str: &str) -> Result<FileInfo, String> {
    let seqno = seqno();
    let req = format!(r#"[{{"a":"g","g":"1","p":"{file_id}"}}]"#);
    let resp = api_post(&req, seqno, None)?;

    let json: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("JSON parse error: {e}"))?;
    let entry = json
        .get(0)
        .ok_or("Empty MEGA API response")?;

    // Check for API-level error (negative integer)
    if let Some(code) = entry.as_i64() {
        return Err(format!("MEGA API error code: {code}"));
    }

    let size = entry
        .get("s")
        .and_then(|v| v.as_u64())
        .ok_or("Missing file size ('s') in API response")?;

    let enc_attr = entry
        .get("at")
        .and_then(|v| v.as_str())
        .ok_or("Missing file attributes ('at') in API response")?;

    let download_url = entry
        .get("g")
        .and_then(|v| v.as_str())
        .ok_or("Missing download URL ('g') in API response")?
        .to_string();

    // Derive AES key and CTR nonce from the link key material
    let raw_key = b64_decode(key_str)?;
    let key = derive_key(&raw_key)?;
    let nonce = derive_nonce(&raw_key)?;

    // Decrypt filename from attributes
    let name = decrypt_attrs(enc_attr, &key)?;

    Ok(FileInfo {
        name,
        size,
        download_url,
        key,
        nonce,
    })
}

/// Download and decrypt a MEGA file, writing output to `output_path`.
///
/// Downloads in 1 MiB chunks, decrypts each chunk with AES-128-CTR,
/// and streams directly to disk.
pub fn download_file(info: &FileInfo, output_path: &str) -> Result<(), String> {
    let mut file =
        std::fs::File::create(output_path).map_err(|e| format!("Cannot create output file: {e}"))?;

    let mut offset: u64 = 0;

    while offset < info.size {
        let chunk_end = (offset + CHUNK_SIZE).min(info.size);
        let chunk_len = chunk_end - offset;

        // MEGA CDN URL format: <base_url>/<start>-<end-1>  (or <base_url>/<start> for last chunk)
        let chunk_url = if chunk_end == info.size {
            format!("{}/{}", info.download_url, offset)
        } else {
            format!("{}/{}-{}", info.download_url, offset, chunk_end - 1)
        };

        let resp = ureq::get(&chunk_url)
            .call()
            .map_err(|e| format!("HTTP error downloading chunk at offset {offset}: {e}"))?;

        let mut data: Vec<u8> = Vec::with_capacity(chunk_len as usize);
        resp.into_reader()
            .take(chunk_len + 16) // +16 safety margin
            .read_to_end(&mut data)
            .map_err(|e| format!("Read error at offset {offset}: {e}"))?;

        if data.len() < chunk_len as usize {
            return Err(format!(
                "Short read at offset {offset}: expected {chunk_len} bytes, got {}",
                data.len()
            ));
        }
        data.truncate(chunk_len as usize);

        decrypt_chunk(&mut data, &info.key, &info.nonce, offset);

        file.write_all(&data)
            .map_err(|e| format!("Write error: {e}"))?;

        offset += chunk_len;
        print_progress(offset, info.size);
    }

    eprintln!(); // newline after progress
    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn api_post(body: &str, seqno: u64, sid: Option<&str>) -> Result<String, String> {
    let mut url = format!("{API_URL}/cs?id={seqno}");
    if let Some(sid) = sid {
        url.push_str("&sid=");
        url.push_str(sid);
    }

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(body)
        .map_err(|e| format!("API request failed: {e}"))?;

    let text = resp
        .into_string()
        .map_err(|e| format!("API response read error: {e}"))?;

    // Top-level negative integer means a global error
    if let Ok(serde_json::Value::Number(n)) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(code) = n.as_i64() {
            if code < 0 {
                return Err(format!("MEGA API error: {code}"));
            }
        }
    }

    Ok(text)
}

fn seqno() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64
}

fn print_progress(done: u64, total: u64) {
    let pct = done * 100 / total;
    let done_mb = done as f64 / (1024.0 * 1024.0);
    let total_mb = total as f64 / (1024.0 * 1024.0);
    eprint!("\r  {done_mb:.1} / {total_mb:.1} MiB  ({pct}%)   ");
    let _ = std::io::stderr().flush();
}
