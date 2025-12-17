use serde::{Deserialize, Serialize};

// This Enum lists every possible message our apps can speak.
// 'derive' automatically writes the code to turn these into JSON.
#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    // 1. Handshake (Client says hello)
    Hello { client_id: String },

    // 2. The Server's reply
    Welcome { session_id: String },

    // 3. Preparing to upload a file
    InitUpload { file_name: String, file_size: u64 },
}
