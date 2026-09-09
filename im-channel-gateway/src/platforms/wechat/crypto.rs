use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit};
use aes::Aes128;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ecb::{Decryptor, Encryptor};
use rand::RngCore;

use crate::error::{GatewayError, Result};

type Aes128EcbEnc = Encryptor<Aes128>;
type Aes128EcbDec = Decryptor<Aes128>;

pub fn make_headers(bot_token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        "AuthorizationType",
        reqwest::header::HeaderValue::from_static("ilink_bot_token"),
    );
    let uin: u32 = rand::thread_rng().next_u32();
    let uin_b64 = B64.encode(uin.to_string().as_bytes());
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&uin_b64) {
        headers.insert("X-WECHAT-UIN", v);
    }
    if !bot_token.is_empty() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {bot_token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    headers
}

fn parse_aes_key(key_b64: &str) -> Result<[u8; 16]> {
    let raw = key_b64.trim();
    let key_bytes: Vec<u8> = if raw.len() >= 32 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(raw).map_err(|e| GatewayError::Other(e.to_string()))?
    } else {
        let decoded = match B64.decode(raw) {
            Ok(d) => d,
            Err(_) => raw.as_bytes().to_vec(),
        };
        if decoded.len() == 16 {
            decoded
        } else if decoded.len() == 32 && decoded.iter().all(|c| c.is_ascii_hexdigit()) {
            hex::decode(String::from_utf8_lossy(&decoded).as_ref())
                .map_err(|e| GatewayError::Other(e.to_string()))?
        } else {
            decoded
        }
    };
    key_bytes
        .try_into()
        .map_err(|_| GatewayError::Other("AES key must be 16 bytes".into()))
}

pub fn aes_ecb_decrypt(data: &[u8], key_b64: &str) -> Result<Vec<u8>> {
    let key = parse_aes_key(key_b64)?;
    let cipher = Aes128EcbDec::new(&key.into());
    let mut buf = data.to_vec();
    let out = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| GatewayError::Other(format!("AES decrypt: {e}")))?;
    Ok(out.to_vec())
}

pub fn aes_ecb_encrypt(data: &[u8], key_b64: &str) -> Result<Vec<u8>> {
    let key = parse_aes_key(key_b64)?;
    let cipher = Aes128EcbEnc::new(&key.into());
    let mut buf = data.to_vec();
    let out_len = data.len() + (16 - data.len() % 16);
    buf.resize(out_len, 0);
    let out = cipher
        .encrypt_padded_mut::<Pkcs7>(&mut buf, data.len())
        .map_err(|e| GatewayError::Other(format!("AES encrypt: {e}")))?;
    Ok(out.to_vec())
}

pub fn generate_aes_key_b64() -> String {
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    B64.encode(key)
}

pub fn generate_aes_key_hex() -> String {
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    hex::encode(key)
}
