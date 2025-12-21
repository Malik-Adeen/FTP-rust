use crate::{auth, storage};
use sha2::{Digest, Sha256};
use shared::{ENCRYPTION_KEY, Message, ParaFlowError, encryption, read_message, send_message};
use std::io::Read;
use std::net::TcpStream;

pub fn handle_client(mut stream: TcpStream) -> Result<(), ParaFlowError> {
    let mut current_salt = String::new();
    let mut is_authenticated = false;

    loop {
        // Read the next message; exit the loop if the connection closes
        let request = match read_message(&mut stream) {
            Ok(msg) => msg,
            Err(_) => return Ok(()),
        };

        match request {
            Message::LoginRequest { client_id } => {
                println!("Login attempt: {}", client_id);
                let salt = auth::generate_salt();
                current_salt = salt.clone();
                send_message(&mut stream, &Message::LoginChallenge { salt })?;
            }
            Message::LoginAnswer { hash } => {
                if auth::verify_user("admin", &current_salt, &hash) {
                    println!("Auth Success!");
                    is_authenticated = true;
                    send_message(
                        &mut stream,
                        &Message::Welcome {
                            session_id: "s1".to_string(),
                        },
                    )?;
                } else {
                    send_message(
                        &mut stream,
                        &Message::ErrorMessage {
                            text: "Access Denied".into(),
                        },
                    )?;
                    return Err(ParaFlowError::AuthError("Wrong Password".into()));
                }
            }
            _ if !is_authenticated => {
                return Err(ParaFlowError::SecurityError("Unauthorized Access".into()));
            }

            Message::InitUpload { file_name, .. } => {
                if file_name.ends_with(".sh") || file_name.ends_with(".exe") {
                    send_message(
                        &mut stream,
                        &Message::ErrorMessage {
                            text: "Forbidden file type".into(),
                        },
                    )?;
                    continue;
                }
                let uuid = uuid::Uuid::new_v4().to_string();
                storage::create_upload_dir(&uuid)?;
                send_message(
                    &mut stream,
                    &Message::InitAck {
                        chunk_size: 0,
                        upload_id: uuid,
                    },
                )?;
            }
            Message::ChunkMeta {
                upload_id,
                chunk_index,
                size,
                hash,
            } => {
                let mut encrypted_data = vec![0u8; size];
                stream.read_exact(&mut encrypted_data)?;

                let mut hasher = Sha256::new();
                hasher.update(&encrypted_data);
                let server_hash = hex::encode(hasher.finalize());

                if server_hash == hash {
                    match encryption::decrypt_chunk(&encrypted_data, &ENCRYPTION_KEY) {
                        Ok(decrypted_data) => {
                            storage::save_chunk(&upload_id, chunk_index, &decrypted_data)?;
                            send_message(&mut stream, &Message::ChunkAck { chunk_index })?;
                        }
                        Err(_) => send_message(&mut stream, &Message::ChunkNack { chunk_index })?,
                    }
                } else {
                    send_message(&mut stream, &Message::ChunkNack { chunk_index })?;
                }
            }
            Message::Complete {
                upload_id,
                file_name,
                total_chunks,
            } => {
                storage::merge_chunks(&upload_id, &file_name, total_chunks)?;
            }
            _ => {}
        }
    }
}
