use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, OsRng}, AeadCore, KeyInit, Nonce
};
pub fn encrypt_chunk(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Invalid key length".to_string())?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|e| format!("Encryption failed: {:?}", e))?;

    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);

    Ok(result)
}


pub fn decrypt_chunk(encrypted_data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted_data.len() < 12 {
        return Err("Data too short".to_string());
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Invalid key length".to_string())?;

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {:?}", e))?;

    Ok(plaintext)
}

