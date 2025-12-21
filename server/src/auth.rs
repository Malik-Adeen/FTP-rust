use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Generates a unique salt for the Challenge-Response handshake
pub fn generate_salt() -> String {
    Uuid::new_v4().to_string()
}

/// Verifies the salted password hash provided by the client
pub fn verify_user(username: &str, salt: &str, answer: &str) -> bool {
    if username == "admin" {
        let actual_pass = "secret123"; // To be moved to environment variables in Phase 2
        let combined = format!("{}{}", actual_pass, salt);

        let mut hasher = Sha256::new();
        hasher.update(combined.as_bytes());
        let expected_hash = hex::encode(hasher.finalize());

        return answer == expected_hash;
    }
    false
}
