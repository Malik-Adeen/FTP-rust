pub mod encryption;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    // 1. Client starts login
    LoginRequest {
        client_id: String,
    },

    // 2. Server sends a random challenge
    LoginChallenge {
        salt: String,
    },

    // 3. Client answers the challenge
    LoginAnswer {
        hash: String,
    },

    // 4. Success! (Contains session_id)
    Welcome {
        session_id: String,
    },

    // ... (Keep InitUpload, ChunkMeta, etc. exactly the same) ...
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
