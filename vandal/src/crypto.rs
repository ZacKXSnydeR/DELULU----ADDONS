use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use md5::{Md5, Digest};
use aes::Aes256;
use cbc::Decryptor;
use cipher::{KeyIvInit, BlockDecryptMut, block_padding::Pkcs7};

type Aes256CbcDec = Decryptor<Aes256>;

pub fn decrypt_cryptojs_aes(b64_str: &str, password: &str) -> Result<String> {
    let data = general_purpose::STANDARD.decode(b64_str)?;
    
    if data.len() < 16 || &data[0..8] != b"Salted__" {
        return Err(anyhow!("Invalid CryptoJS payload: missing Salted__ prefix"));
    }
    
    let salt = &data[8..16];
    let ciphertext = &data[16..];
    
    // EVP_BytesToKey equivalent for MD5, 32 byte key, 16 byte IV
    let mut key_iv = Vec::new();
    let mut prev = Vec::new();
    let pass_bytes = password.as_bytes();
    
    while key_iv.len() < 48 {
        let mut hasher = Md5::new();
        hasher.update(&prev);
        hasher.update(pass_bytes);
        hasher.update(salt);
        let hash = hasher.finalize();
        prev = hash.to_vec();
        key_iv.extend_from_slice(&prev);
    }
    
    let key = &key_iv[0..32];
    let iv = &key_iv[32..48];
    
    let dec = Aes256CbcDec::new(key.into(), iv.into());
    let mut pt_buffer = ciphertext.to_vec();
    let pt = dec.decrypt_padded_mut::<Pkcs7>(&mut pt_buffer)
        .map_err(|e| anyhow!("AES Decryption failed: {:?}", e))?;
        
    Ok(String::from_utf8(pt.to_vec())?)
}
