use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto::{
    b64_decode, decrypt_attrs, decrypt_chunk, decrypt_folder_file_key, derive_key, derive_nonce,
};

const API_URL: &str = "https://g.api.mega.co.nz";

/// Sequential chunk size (1 MiB) used for single-threaded downloads.
const CHUNK_SIZE: u64 = 1024 * 1024;

/// Minimum parallel chunk size (5 MiB) per thread.
const PAR_CHUNK_MIN: u64 = 5 * 1024 * 1024;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub download_url: String,
    pub key: [u8; 16],
    pub nonce: [u8; 8],
}

pub struct FolderNode {
    pub handle: String,
    pub name: String,
    pub size: u64,
    pub key: [u8; 16],
    pub nonce: [u8; 8],
}

// ── Link parsing ──────────────────────────────────────────────────────────────

/// Returns true if the link is a folder link.
pub fn is_folder_link(link: &str) -> bool {
    link.contains("/folder/") || link.contains("/#F!")
}

/// Parse a MEGA public *file* link → (file_id, key_str).
///
/// Supported formats:
///   https://mega.nz/#!<id>!<key>
///   https://mega.nz/file/<id>#<key>
pub fn parse_link(link: &str) -> Result<(String, String), String> {
    if let Some(after) = link.split("/#!").nth(1) {
        let mut parts = after.splitn(2, '!');
        let id = parts.next().ok_or("Missing file ID in link")?;
        let key = parts.next().ok_or("Missing file key in link")?;
        let key = key.split(['?', '#', '&']).next().unwrap_or(key);
        return Ok((id.to_string(), key.to_string()));
    }
    if let Some(after) = link.split("/file/").nth(1) {
        let mut parts = after.splitn(2, '#');
        let id = parts.next().ok_or("Missing file ID in link")?;
        let key = parts.next().ok_or("Missing file key in link")?;
        let key = key.split(['?', '&']).next().unwrap_or(key);
        return Ok((id.to_string(), key.to_string()));
    }
    Err(format!("Unrecognised MEGA file link: {link}"))
}

/// Parse a MEGA public *folder* link → (folder_id, folder_key_str).
///
/// Supported format:
///   https://mega.nz/folder/<id>#<key>
pub fn parse_folder_link(link: &str) -> Result<(String, String), String> {
    if let Some(after) = link.split("/folder/").nth(1) {
        let mut parts = after.splitn(2, '#');
        let id = parts.next().ok_or("Missing folder ID in link")?;
        let key = parts.next().ok_or("Missing folder key in link")?;
        let key = key.split(['?', '&']).next().unwrap_or(key);
        return Ok((id.to_string(), key.to_string()));
    }
    Err(format!("Unrecognised MEGA folder link: {link}"))
}

// ── Single-file API ───────────────────────────────────────────────────────────

/// Fetch file metadata and a temporary download URL from the MEGA API.
pub fn get_file_info(file_id: &str, key_str: &str) -> Result<FileInfo, String> {
    let seqno = seqno();
    let req = format!(r#"[{{"a":"g","g":"1","p":"{file_id}"}}]"#);
    let resp = api_post(&req, seqno, None, None)?;

    let json: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("JSON parse error: {e}"))?;
    let entry = json.get(0).ok_or("Empty MEGA API response")?;

    if let Some(code) = entry.as_i64() {
        return Err(format!("MEGA API error code: {code}"));
    }

    let size = entry
        .get("s").and_then(|v| v.as_u64())
        .ok_or("Missing file size in API response")?;
    let enc_attr = entry
        .get("at").and_then(|v| v.as_str())
        .ok_or("Missing file attributes in API response")?;
    let download_url = entry
        .get("g").and_then(|v| v.as_str())
        .ok_or("Missing download URL in API response")?
        .to_string();

    let raw_key = b64_decode(key_str)?;
    let key = derive_key(&raw_key)?;
    let nonce = derive_nonce(&raw_key)?;
    let name = decrypt_attrs(enc_attr, &key)?;

    Ok(FileInfo { name, size, download_url, key, nonce })
}

// ── Folder API ────────────────────────────────────────────────────────────────

/// List all files (type=0) in a public MEGA folder.
pub fn list_folder(folder_id: &str, folder_key_str: &str) -> Result<Vec<FolderNode>, String> {
    let folder_key_bytes = b64_decode(folder_key_str)?;
    if folder_key_bytes.len() != 16 {
        return Err(format!(
            "Expected 16-byte folder key, got {} bytes",
            folder_key_bytes.len()
        ));
    }
    let mut folder_key = [0u8; 16];
    folder_key.copy_from_slice(&folder_key_bytes);

    let seqno = seqno();
    let resp = api_post(r#"[{"a":"f","c":1,"r":1}]"#, seqno, None, Some(folder_id))?;

    let json: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("JSON parse error: {e}"))?;
    let files = json
        .get(0).and_then(|v| v.get("f")).and_then(|v| v.as_array())
        .ok_or("Missing 'f' array in folder listing response")?;

    let mut nodes = Vec::new();

    for node in files {
        let t = node.get("t").and_then(|v| v.as_u64()).unwrap_or(99);
        if t != 0 {
            continue; // skip folder nodes, root, inbox, trash
        }

        let handle = match node.get("h").and_then(|v| v.as_str()) {
            Some(h) => h.to_string(),
            None => continue,
        };
        let size = match node.get("s").and_then(|v| v.as_u64()) {
            Some(s) => s,
            None => continue,
        };
        let enc_attr = match node.get("a").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => continue,
        };
        let k_field = match node.get("k").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => continue,
        };

        // Key field: "<owner_handle>:<base64_encrypted_key>" or just "<base64_encrypted_key>"
        let key_b64 = match k_field.find(':') {
            Some(pos) => &k_field[pos + 1..],
            None => k_field,
        };

        let enc_key = match b64_decode(key_b64) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let raw_key = match decrypt_folder_file_key(&enc_key, &folder_key) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let key = match derive_key(&raw_key) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let nonce = match derive_nonce(&raw_key) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let name = decrypt_attrs(enc_attr, &key).unwrap_or_else(|_| handle.clone());

        nodes.push(FolderNode { handle, name, size, key, nonce });
    }

    Ok(nodes)
}

/// Get a temporary download URL for a file node inside a folder.
pub fn get_node_download_url(folder_id: &str, node_handle: &str) -> Result<String, String> {
    let seqno = seqno();
    let req = format!(r#"[{{"a":"g","g":1,"n":"{node_handle}"}}]"#);
    let resp = api_post(&req, seqno, None, Some(folder_id))?;

    let json: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("JSON parse error: {e}"))?;
    let entry = json.get(0).ok_or("Empty API response")?;

    if let Some(code) = entry.as_i64() {
        return Err(format!("MEGA API error: {code}"));
    }

    entry.get("g").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing download URL in API response".to_string())
}

// ── Download ──────────────────────────────────────────────────────────────────

/// Download and decrypt a single file.
///
/// When `threads == 1` the download is sequential (1 MiB chunks streamed to
/// disk).  When `threads > 1` the file is split into `threads` parallel
/// chunks, each downloaded and decrypted concurrently, then written at the
/// correct offset with `pwrite`.
pub fn download_file(info: &FileInfo, output_path: &str, threads: usize) -> Result<(), String> {
    if threads <= 1 {
        download_sequential(info, output_path)
    } else {
        download_parallel(info, output_path, threads)
    }
}

/// Download all files in a public MEGA folder.
///
/// `parallel` controls how many files are downloaded at once.
/// Each file itself is downloaded sequentially (parallel chunk download is
/// used for single-file mode; keeping folder mode simple avoids thread
/// explosion).
pub fn download_folder(
    folder_id: &str,
    folder_key_str: &str,
    output_dir: &str,
    parallel: usize,
) -> Result<(), String> {
    eprintln!("Listing folder contents...");
    let nodes = list_folder(folder_id, folder_key_str)?;

    if nodes.is_empty() {
        eprintln!("Folder is empty (no downloadable files found).");
        return Ok(());
    }
    eprintln!("Found {} file(s).", nodes.len());

    std::fs::create_dir_all(output_dir)
        .map_err(|e| format!("Cannot create output directory '{output_dir}': {e}"))?;

    // Process files in batches of `parallel`.
    for batch in nodes.chunks(parallel.max(1)) {
        let mut handles: Vec<std::thread::JoinHandle<Result<(), String>>> = Vec::new();

        for node in batch {
            let folder_id = folder_id.to_string();
            let handle_str = node.handle.clone();
            let name = node.name.clone();
            let size = node.size;
            let key = node.key;
            let nonce = node.nonce;
            let dir = output_dir.trim_end_matches('/').to_string();

            handles.push(std::thread::spawn(move || {
                let output_path = format!("{dir}/{name}");
                eprintln!(
                    "[{}]  {:.2} MiB  →  {}",
                    name,
                    size as f64 / (1024.0 * 1024.0),
                    output_path
                );
                let download_url = get_node_download_url(&folder_id, &handle_str)?;
                let info = FileInfo { name: name.clone(), size, download_url, key, nonce };
                download_sequential(&info, &output_path)?;
                eprintln!("[{}] Done.", name);
                Ok(())
            }));
        }

        for h in handles {
            h.join().map_err(|_| "Download thread panicked".to_string())??;
        }
    }

    Ok(())
}

// ── Internal download helpers ─────────────────────────────────────────────────

/// Sequential download: stream 1 MiB chunks, decrypt in-place, write to file.
fn download_sequential(info: &FileInfo, output_path: &str) -> Result<(), String> {
    let mut file = std::fs::File::create(output_path)
        .map_err(|e| format!("Cannot create '{output_path}': {e}"))?;

    let mut offset: u64 = 0;

    while offset < info.size {
        let chunk_end = (offset + CHUNK_SIZE).min(info.size);
        let chunk_len = chunk_end - offset;

        let chunk_url = if chunk_end == info.size {
            format!("{}/{}", info.download_url, offset)
        } else {
            format!("{}/{}-{}", info.download_url, offset, chunk_end - 1)
        };

        let resp = ureq::get(&chunk_url)
            .call()
            .map_err(|e| format!("HTTP error at offset {offset}: {e}"))?;

        let mut data: Vec<u8> = Vec::with_capacity(chunk_len as usize);
        resp.into_reader()
            .take(chunk_len + 16)
            .read_to_end(&mut data)
            .map_err(|e| format!("Read error at offset {offset}: {e}"))?;

        if data.len() < chunk_len as usize {
            return Err(format!(
                "Short read at {offset}: expected {chunk_len} bytes, got {}",
                data.len()
            ));
        }
        data.truncate(chunk_len as usize);

        decrypt_chunk(&mut data, &info.key, &info.nonce, offset);
        file.write_all(&data).map_err(|e| format!("Write error: {e}"))?;

        offset += chunk_len;
        print_progress(offset, info.size);
    }

    eprintln!();
    Ok(())
}

/// Parallel download: split into `threads` chunks, download concurrently,
/// write each chunk at the correct file offset using `pwrite` (no seek lock).
fn download_parallel(info: &FileInfo, output_path: &str, threads: usize) -> Result<(), String> {
    // Pre-allocate the output file.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(output_path)
        .map_err(|e| format!("Cannot create '{output_path}': {e}"))?;
    file.set_len(info.size)
        .map_err(|e| format!("Cannot pre-allocate file: {e}"))?;
    let file = Arc::new(file);

    // Chunk size: divide evenly, but at least PAR_CHUNK_MIN; always a
    // multiple of 16 so CTR block counters stay aligned.
    let raw = ((info.size + threads as u64 - 1) / threads as u64).max(PAR_CHUNK_MIN);
    let chunk_size = (raw + 15) & !15;

    let mut handles: Vec<std::thread::JoinHandle<Result<(), String>>> = Vec::new();
    let mut offset = 0u64;

    while offset < info.size {
        let end = (offset + chunk_size).min(info.size);
        let file = Arc::clone(&file);
        let url = info.download_url.clone();
        let key = info.key;
        let nonce = info.nonce;
        let total = info.size;

        handles.push(std::thread::spawn(move || {
            let len = end - offset;
            let chunk_url = if end == total {
                format!("{url}/{offset}")
            } else {
                format!("{url}/{offset}-{}", end - 1)
            };

            let resp = ureq::get(&chunk_url)
                .call()
                .map_err(|e| format!("HTTP error at offset {offset}: {e}"))?;

            let mut data: Vec<u8> = Vec::with_capacity(len as usize + 16);
            resp.into_reader()
                .take(len + 16)
                .read_to_end(&mut data)
                .map_err(|e| format!("Read error at offset {offset}: {e}"))?;

            data.truncate(len as usize);
            decrypt_chunk(&mut data, &key, &nonce, offset);

            // pwrite: no seek needed, thread-safe on POSIX.
            file.write_at(&data, offset)
                .map_err(|e| format!("Write error at offset {offset}: {e}"))?;

            Ok(())
        }));

        offset = end;
    }

    let chunk_count = handles.len();
    for h in handles {
        h.join().map_err(|_| "Download thread panicked".to_string())??;
    }

    eprintln!(
        "  {:.1} MiB  ({} parallel chunks)",
        info.size as f64 / (1024.0 * 1024.0),
        chunk_count
    );
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn api_post(
    body: &str,
    seqno: u64,
    sid: Option<&str>,
    folder_id: Option<&str>,
) -> Result<String, String> {
    let mut url = format!("{API_URL}/cs?id={seqno}");
    if let Some(sid) = sid {
        url.push_str("&sid=");
        url.push_str(sid);
    }
    if let Some(n) = folder_id {
        url.push_str("&n=");
        url.push_str(n);
    }

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(body)
        .map_err(|e| format!("API request failed: {e}"))?;

    let text = resp
        .into_string()
        .map_err(|e| format!("API response read error: {e}"))?;

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
