pub mod encryption;
use hex;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use thiserror::Error;
pub const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024 + 256;

/// Unified Error Type for the ParaFlow System
#[derive(Error, Debug)]
pub enum ParaFlowError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization Error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Authentication Failed: {0}")]
    AuthError(String),

    #[error("Protocol Violation: {0}")]
    ProtocolError(String),

    #[error("Security Violation: {0}")]
    SecurityError(String),

    #[error("Encryption/Decryption Error: {0}")]
    EncryptionError(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    LoginRequest {
        client_id: String,
    },
    LoginChallenge {
        salt: String,
    },
    LoginAnswer {
        hash: String,
    },
    Welcome {
        session_id: String,
    },
    InitUpload {
        file_name: String,
        total_size: u64,
    },
    InitAck {
        chunk_size: u64,
        upload_id: String,
    },
    ChunkMeta {
        upload_id: String,
        chunk_index: u64,
        size: usize,
        hash: String,
    },
    ChunkAck {
        chunk_index: u64,
    },
    ChunkNack {
        chunk_index: u64,
    },
    Complete {
        upload_id: String,
        file_name: String,
        total_chunks: u64,
    },
    CompleteAck,
    ErrorMessage {
        text: String,
    },
}

pub fn send_message<W: Write>(stream: &mut W, msg: &Message) -> Result<(), ParaFlowError> {
    let json = serde_json::to_string(msg)?;
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(json.as_bytes())?;
    Ok(())
}

pub fn read_message<R: Read>(stream: &mut R) -> Result<Message, ParaFlowError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut json_buf = vec![0u8; len];
    stream.read_exact(&mut json_buf)?;

    let text = String::from_utf8_lossy(&json_buf);
    let msg = serde_json::from_str(&text)?;
    Ok(msg)
}

/// Loads the 32-byte encryption key from the PARAFLOW_ENCRYPTION_KEY environment variable
pub fn load_encryption_key() -> Result<[u8; 32], ParaFlowError> {
    let key_hex = std::env::var("PARAFLOW_ENCRYPTION_KEY")
        .map_err(|_| ParaFlowError::SecurityError("PARAFLOW_ENCRYPTION_KEY not set".into()))?;

    let bytes = hex::decode(key_hex)
        .map_err(|_| ParaFlowError::SecurityError("Invalid hex key format".into()))?;

    if bytes.len() != 32 {
        return Err(ParaFlowError::SecurityError(
            "Encryption key must be exactly 32 bytes".into(),
        ));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}
