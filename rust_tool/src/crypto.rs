use aes::Aes128;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cbc::Decryptor as CbcDecryptor;
use cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit, StreamCipher};
use ctr::Ctr128BE;

/// Decode MEGA's URL-safe base64 (no padding).
/// MEGA sometimes appends ',' or whitespace – strip those first.
pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let clean: String = s
        .chars()
        .filter(|&c| c != ',' && c != '\n' && c != '\r' && c != ' ')
        .collect();
    URL_SAFE_NO_PAD
        .decode(&clean)
        .map_err(|e| format!("base64 decode error: {e}"))
}

/// XOR-fold a 32-byte MEGA file key into a 16-byte AES-128 key.
/// Java equivalent: initMEGALinkKey()
pub fn derive_key(raw_key: &[u8]) -> Result<[u8; 16], String> {
    if raw_key.len() != 32 {
        return Err(format!(
            "Expected 32-byte MEGA file key, got {} bytes",
            raw_key.len()
        ));
    }
    let mut key = [0u8; 16];
    for i in 0..16 {
        key[i] = raw_key[i] ^ raw_key[i + 16];
    }
    Ok(key)
}

/// Extract the 8-byte CTR nonce from bytes 16..24 of the MEGA file key.
/// Java equivalent: initMEGALinkKeyIV() (first half of the IV)
pub fn derive_nonce(raw_key: &[u8]) -> Result<[u8; 8], String> {
    if raw_key.len() < 24 {
        return Err("MEGA file key too short for nonce extraction".into());
    }
    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&raw_key[16..24]);
    Ok(nonce)
}

/// Decrypt MEGA file attributes (AES-128-CBC, zero IV, no padding).
/// The plaintext starts with "MEGA" followed by a JSON object {"n":"filename",...}.
/// Returns the filename on success.
pub fn decrypt_attrs(enc_attr: &str, key: &[u8; 16]) -> Result<String, String> {
    let mut data = b64_decode(enc_attr)?;

    // Pad to AES block boundary
    let rem = data.len() % 16;
    if rem != 0 {
        data.resize(data.len() + (16 - rem), 0);
    }

    let iv = [0u8; 16];
    type AesCbc = CbcDecryptor<Aes128>;
    let dec = AesCbc::new_from_slices(key, &iv)
        .map_err(|e| format!("CBC init error: {e}"))?;
    let plaintext = dec
        .decrypt_padded_mut::<NoPadding>(&mut data)
        .map_err(|e| format!("CBC decrypt error: {e}"))?;

    let text = std::str::from_utf8(plaintext)
        .map_err(|e| format!("UTF-8 decode error: {e}"))?;
    let text = text.trim_end_matches('\0');
    let json_str = text
        .strip_prefix("MEGA")
        .ok_or("Missing 'MEGA' prefix in file attributes")?;

    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;
    v.get("n")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No filename ('n') field in file attributes".into())
}

/// Decrypt a chunk in-place using AES-128-CTR.
///
/// MEGA's CTR IV = nonce (8 bytes) || counter (8 bytes, big-endian)
/// where counter = chunk_byte_offset / 16 (AES block index).
///
/// Java equivalent: forwardMEGALinkKeyIV()
pub fn decrypt_chunk(data: &mut [u8], key: &[u8; 16], nonce: &[u8; 8], chunk_offset: u64) {
    let block_index = chunk_offset / 16;
    let mut iv = [0u8; 16];
    iv[..8].copy_from_slice(nonce);
    iv[8..].copy_from_slice(&block_index.to_be_bytes());

    let mut cipher = Ctr128BE::<Aes128>::new(key.into(), &iv.into());
    cipher.apply_keystream(data);
}
