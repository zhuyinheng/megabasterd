use aes::Aes128;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cbc::Decryptor as CbcDecryptor;
use cipher::{block_padding::NoPadding, BlockDecrypt, BlockDecryptMut, KeyInit, KeyIvInit, StreamCipher};
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

/// Decrypt a MEGA folder file key using the folder share key (AES-128-ECB).
///
/// Each file node in a folder has its raw key encrypted with the folder's share key.
/// The raw key (32 bytes for files) is then processed with `derive_key` / `derive_nonce`.
pub fn decrypt_folder_file_key(enc_key: &[u8], folder_key: &[u8; 16]) -> Result<Vec<u8>, String> {
    if enc_key.is_empty() || enc_key.len() % 16 != 0 {
        return Err(format!("Invalid encrypted key length: {}", enc_key.len()));
    }
    let cipher = Aes128::new(cipher::generic_array::GenericArray::from_slice(folder_key));
    let mut result = enc_key.to_vec();
    for chunk in result.chunks_exact_mut(16) {
        cipher.decrypt_block(cipher::generic_array::GenericArray::from_mut_slice(chunk));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vector derived from MEGA's known algorithm:
    // A 32-byte key where the XOR-fold produces a known AES key
    #[test]
    fn test_derive_key_xor_fold() {
        let raw = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
        ];
        let key = derive_key(&raw).unwrap();
        // key[i] = raw[i] ^ raw[i+16]
        assert_eq!(key[0], 0x01 ^ 0x11);
        assert_eq!(key[15], 0x10 ^ 0x20);
    }

    #[test]
    fn test_derive_nonce() {
        let raw = [0u8; 32];
        let mut raw2 = raw;
        raw2[16] = 0xAB;
        raw2[17] = 0xCD;
        raw2[23] = 0xFF;
        let nonce = derive_nonce(&raw2).unwrap();
        assert_eq!(nonce[0], 0xAB);
        assert_eq!(nonce[1], 0xCD);
        assert_eq!(nonce[7], 0xFF);
    }

    #[test]
    fn test_decrypt_chunk_round_trip() {
        // Encrypt then decrypt should recover original data
        let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6,
                   0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let nonce = [0x00u8; 8];
        let original = b"Hello MEGA world!".to_vec();
        let mut data = original.clone();
        // Encrypt at offset 0
        decrypt_chunk(&mut data, &key, &nonce, 0);
        // Encrypt again (CTR is its own inverse)
        decrypt_chunk(&mut data, &key, &nonce, 0);
        assert_eq!(data, original);
    }

    #[test]
    fn test_ctr_offset_independence() {
        // Decrypting at offset 16 should use counter=1
        let key = [0xAAu8; 16];
        let nonce = [0xBBu8; 8];
        let mut block0 = [0u8; 16];
        let mut block1 = [0u8; 16];
        decrypt_chunk(&mut block0, &key, &nonce, 0);   // counter=0
        decrypt_chunk(&mut block1, &key, &nonce, 16);  // counter=1
        // Two consecutive blocks should produce different keystream
        assert_ne!(block0, block1);
    }

    #[test]
    fn test_b64_decode_url_safe() {
        // MEGA uses URL-safe base64 without padding
        let encoded = "SGVsbG8gd29ybGQ"; // "Hello world" in URL-safe b64 no pad
        let decoded = b64_decode(encoded).unwrap();
        assert_eq!(&decoded, b"Hello world");
    }

    #[test]
    fn test_decrypt_attrs() {
        // Manually construct an AES-CBC encrypted MEGA attribute block
        // key = all zeros
        use aes::Aes128;
        use cbc::Encryptor as CbcEncryptor;
        use cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        let key = [0u8; 16];
        let iv  = [0u8; 16];
        let plaintext = b"MEGA{\"n\":\"test.txt\"}\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let mut buf = plaintext.to_vec();

        type AesCbc = CbcEncryptor<Aes128>;
        let enc = AesCbc::new_from_slices(&key, &iv).unwrap();
        let ct = enc.encrypt_padded_mut::<NoPadding>(&mut buf, plaintext.len()).unwrap();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ct);

        let name = decrypt_attrs(&encoded, &key).unwrap();
        assert_eq!(name, "test.txt");
    }
}
