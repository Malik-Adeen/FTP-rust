use argon2::{Algorithm, Argon2, Params, Version};
use shared::ParaFlowError;
use uuid::Uuid;

pub fn generate_salt() -> String {
    Uuid::new_v4().to_string()
}

fn compute_challenge_hash(password: &str, challenge: &str) -> Result<String, ParaFlowError> {
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|e| ParaFlowError::EncryptionError(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), challenge.as_bytes(), &mut output)
        .map_err(|e| ParaFlowError::EncryptionError(e.to_string()))?;
    Ok(hex::encode(output))
}

pub fn verify_user(username: &str, salt: &str, answer: &str) -> Result<bool, ParaFlowError> {
    if username != "admin" {
        return Ok(false);
    }
    let password = std::env::var("PARAFLOW_ADMIN_PASSWORD")
        .map_err(|_| ParaFlowError::SecurityError("PARAFLOW_ADMIN_PASSWORD not set".into()))?;
    let expected = compute_challenge_hash(&password, salt)?;
    Ok(answer == expected)
}
