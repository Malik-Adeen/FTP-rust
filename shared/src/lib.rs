pub mod encryption;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use thiserror::Error;

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

// Global Shared Key - To be moved to environment variables in Phase 2
pub const ENCRYPTION_KEY: [u8; 32] = [
    0x42, 0x8a, 0x7b, 0x1f, 0x9d, 0x3e, 0x5c, 0x6f, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18,
    0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18,
];

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
    ErrorMessage {
        text: String,
    },
}

/// Centralized helper to send messages over a TCP stream
pub fn send_message(stream: &mut TcpStream, msg: &Message) -> Result<(), ParaFlowError> {
    let json = serde_json::to_string(msg)?;
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(json.as_bytes())?;
    Ok(())
}

/// Centralized helper to read messages from a TCP stream
pub fn read_message(stream: &mut TcpStream) -> Result<Message, ParaFlowError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut json_buf = vec![0u8; len];
    stream.read_exact(&mut json_buf)?;

    let text = String::from_utf8_lossy(&json_buf);
    let msg = serde_json::from_str(&text)?;
    Ok(msg)
}
