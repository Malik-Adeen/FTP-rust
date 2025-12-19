use clap::Parser;
use sha2::{Digest, Sha256};
use shared::Message;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use uuid::Uuid; // NEW
use shared::encryption;

// Shared encryption key (32 bytes for AES-256)
// Must match client's key
const ENCRYPTION_KEY: [u8; 32] = [
    0x42, 0x8a, 0x7b, 0x1f, 0x9d, 0x3e, 0x5c, 0x6f,
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18,
    0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90,
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18,
];

#[derive(Parser)]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value_t = 7878)]
    port: u16,
}

// UPDATED: Merge logic now looks in the temporary sub-folder
fn merge_chunks(upload_id: &str, file_name: &str, total_chunks: u64) {
    let temp_dir = format!("uploads/{}", upload_id);
    let output_path = format!("uploads/{}", file_name);

    println!(
        ">> Merging {} chunks from {} into {}...",
        total_chunks, temp_dir, output_path
    );

    let mut output_file = File::create(&output_path).unwrap();

    for i in 0..total_chunks {
        let chunk_path = format!("{}/chunk_{}", temp_dir, i);

        // Retry logic for file opening (sometimes OS is slow to release locks)
        let mut chunk_file = File::open(&chunk_path).expect("Missing chunk during merge");

        std::io::copy(&mut chunk_file, &mut output_file).unwrap();
        // We don't delete individual chunks yet, we delete the whole folder at the end
    }

    // Cleanup: Remove the temporary directory
    fs::remove_dir_all(temp_dir).unwrap();
    println!(">> Merge Complete. Saved to {}", output_path);
}

// ... (send_message stays the same) ...
fn send_message(stream: &mut TcpStream, msg: &Message) {
    let json = serde_json::to_string(msg).unwrap();
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
}

fn verify_user(username: &str, salt: &str, answer: &str) -> bool {
    // For demo purposes, we hardcode a single user
    if username == "admin" {
        let actual_pass = "secret123";
        let combined = format!("{}{}", actual_pass, salt);

        let mut hasher = Sha256::new();
        hasher.update(combined.as_bytes());
        let expected_hash = hex::encode(hasher.finalize());

        return answer == expected_hash;
    }
    false
}

fn handle_client(mut stream: TcpStream) {
    let mut current_salt = String::new();
    let mut is_authenticated = false;
    loop {
        // ... (Header reading logic same as before) ...
        let mut len_bytes = [0u8; 4];
        if stream.read_exact(&mut len_bytes).is_err() {
            return;
        }
        let len = u32::from_be_bytes(len_bytes) as usize;

        let mut json_buffer = vec![0u8; len];
        if stream.read_exact(&mut json_buffer).is_err() {
            return;
        }
        let received_text = String::from_utf8_lossy(&json_buffer);
        let request: Result<Message, _> = serde_json::from_str(&received_text);

        match request {
            // STEP 1: Login Request
            Ok(Message::LoginRequest { client_id }) => {
                println!("Login attempt: {}", client_id);
                // Generate random salt
                let salt = Uuid::new_v4().to_string();
                current_salt = salt.clone();

                send_message(&mut stream, &Message::LoginChallenge { salt });
            }
            // STEP 2: Verify Answer
            Ok(Message::LoginAnswer { hash }) => {
                if verify_user("admin", &current_salt, &hash) {
                    println!("Auth Success!");
                    is_authenticated = true;
                    send_message(
                        &mut stream,
                        &Message::Welcome {
                            session_id: "s1".to_string(),
                        },
                    );
                } else {
                    println!("Auth Failed! Disconnecting.");
                    // 1. SEND ERROR MESSAGE
                    send_message(
                        &mut stream,
                        &Message::ErrorMessage {
                            text: "Access Denied: Wrong Password".to_string(),
                        },
                    );
                    return; // Drop connection
                }
            }
            // SECURITY CHECK: Block everything else if not authenticated
            _ if !is_authenticated => {
                println!("Unauthorized request. Dropping.");
                return;
            }
            // 1. START UPLOAD: Generate UUID and Folder
            Ok(Message::InitUpload { file_name, .. }) => {
                if file_name.ends_with(".sh") || file_name.ends_with(".exe") {
                    println!("Security Alert: Rejected {}", file_name);
                    send_message(
                        &mut stream,
                        &Message::ErrorMessage {
                            text: "Security Policy Violation: Executables not allowed.".to_string(),
                        },
                    );
                    continue; // Skip the rest, client will handle the error
                }

                let uuid = Uuid::new_v4().to_string();
                let upload_folder = format!("uploads/{}", uuid);

                println!("Starting Upload: {} -> ID: {}", file_name, uuid);
                fs::create_dir_all(&upload_folder).unwrap();

                send_message(
                    &mut stream,
                    &Message::InitAck {
                        chunk_size: 0,
                        upload_id: uuid, // Send ID back to client
                    },
                );
            }

            // 2. RECEIVE CHUNK: Decrypt and save to specific folder
            Ok(Message::ChunkMeta {
                upload_id,
                chunk_index,
                size,
                hash,
            }) => {
                let mut encrypted_data = vec![0u8; size];
                if stream.read_exact(&mut encrypted_data).is_err() {
                    return;
                }

                let mut hasher = Sha256::new();
                hasher.update(&encrypted_data);
                let server_hash = hex::encode(hasher.finalize());

                if server_hash == hash {
                    // Decrypt the chunk before saving
                    match encryption::decrypt_chunk(&encrypted_data, &ENCRYPTION_KEY) {
                        Ok(decrypted_data) => {
                            // Save DECRYPTED data to sub-folder
                            let safe_path = format!("uploads/{}/chunk_{}", upload_id, chunk_index);
                            let mut f = File::create(&safe_path).unwrap();
                            f.write_all(&decrypted_data).unwrap();
                            println!("✓ Chunk {} decrypted and saved ({} bytes)", chunk_index, decrypted_data.len());
                            send_message(&mut stream, &Message::ChunkAck { chunk_index });
                        }
                        Err(e) => {
                            println!("!!! DECRYPTION FAILED on Chunk #{}: {} !!!", chunk_index, e);
                            send_message(&mut stream, &Message::ChunkNack { chunk_index });
                        }
                    }
                } else {
                    println!("!!! CORRUPTION on Chunk #{} !!!", chunk_index);
                    send_message(&mut stream, &Message::ChunkNack { chunk_index });
                }
            }

            // 3. COMPLETE: Merge from sub-folder
            Ok(Message::Complete {
                upload_id,
                file_name,
                total_chunks,
            }) => {
                println!("Upload Complete: {}", file_name);
                merge_chunks(&upload_id, &file_name, total_chunks); // Use the real number!
            }
            _ => {}
        }
    }
}

// ... (main stays the same) ...
fn main() {
    let args = Cli::parse();
    let addr = format!("0.0.0.0:{}", args.port); // 0.0.0.0 allows connections from other PCs
    let listener = TcpListener::bind(&addr).unwrap();
    println!("🌍 Server listening on {} ...", addr);
    for stream in listener.incoming() {
        if let Ok(s) = stream {
            thread::spawn(|| handle_client(s));
        }
    }
}