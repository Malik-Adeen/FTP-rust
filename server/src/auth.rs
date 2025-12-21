use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Generates a unique salt for the Challenge-Response handshake
pub fn generate_salt() -> String {
    Uuid::new_v4().to_string()
}

pub fn verify_user(username: &str, salt: &str, answer: &str) -> bool {
    if username == "admin" {
        // Load password from environment variable
        let actual_pass = std::env::var("PARAFLOW_ADMIN_PASSWORD")
            .unwrap_or_else(|_| "default_fallback_change_me".to_string());

        let combined = format!("{}{}", actual_pass, salt);
        let mut hasher = Sha256::new();
        hasher.update(combined.as_bytes());
        let expected_hash = hex::encode(hasher.finalize());

        return answer == expected_hash;
    }
    false
}
