use crate::{auth, rate_limit::AuthRateLimiter, storage};
use sha2::{Digest, Sha256};
use shared::{MAX_CHUNK_BYTES, Message, ParaFlowError, encryption, load_encryption_key, read_message, send_message};
use std::collections::HashMap;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub type UploadRegistry = Mutex<HashMap<String, String>>;

struct SessionState {
    current_salt: Option<String>,
    is_authenticated: bool,
    session_id: Option<String>,
    encryption_key: [u8; 32],
}

impl SessionState {
    fn new(encryption_key: [u8; 32]) -> Self {
        Self {
            current_salt: None,
            is_authenticated: false,
            session_id: None,
            encryption_key,
        }
    }
}

fn require_session_id(state: &SessionState) -> Result<String, ParaFlowError> {
    state
        .session_id
        .clone()
        .ok_or_else(|| ParaFlowError::SecurityError("Unauthorized Access".into()))
}

fn sanitize_file_name(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
}

fn handle_login_request(
    state: &mut SessionState,
    stream: &mut TcpStream,
    client_id: String,
    peer_addr: SocketAddr,
    rate_limiter: &Arc<AuthRateLimiter>,
) -> Result<(), ParaFlowError> {
    if rate_limiter.check_key(&peer_addr.ip()).is_err() {
        send_message(
            stream,
            &Message::ErrorMessage {
                text: "Too many login attempts. Try again later.".into(),
            },
        )?;
        return Err(ParaFlowError::SecurityError("Rate limit exceeded".into()));
    }
    println!("Login attempt: {}", client_id);
    let salt = auth::generate_salt();
    state.current_salt = Some(salt.clone());
    send_message(stream, &Message::LoginChallenge { salt })
}

fn handle_login_answer(
    state: &mut SessionState,
    stream: &mut TcpStream,
    hash: String,
) -> Result<(), ParaFlowError> {
    let salt = state
        .current_salt
        .take()
        .ok_or_else(|| ParaFlowError::ProtocolError("Missing login challenge".into()))?;

    if auth::verify_user("admin", &salt, &hash)? {
        println!("Auth Success!");
        state.is_authenticated = true;
        let session_id = Uuid::new_v4().to_string();
        state.session_id = Some(session_id.clone());
        send_message(stream, &Message::Welcome { session_id })
    } else {
        send_message(
            stream,
            &Message::ErrorMessage {
                text: "Access Denied".into(),
            },
        )?;
        Err(ParaFlowError::AuthError("Wrong Password".into()))
    }
}

fn handle_init_upload(
    session_id: &str,
    stream: &mut TcpStream,
    file_name: String,
    registry: &Arc<UploadRegistry>,
) -> Result<(), ParaFlowError> {
    let safe_name = match sanitize_file_name(&file_name) {
        Some(n) => n,
        None => {
            send_message(stream, &Message::ErrorMessage { text: "Invalid file name".into() })?;
            return Ok(());
        }
    };
    if safe_name.ends_with(".sh") || safe_name.ends_with(".exe") {
        send_message(stream, &Message::ErrorMessage { text: "Forbidden file type".into() })?;
        return Ok(());
    }

    let upload_id = Uuid::new_v4().to_string();
    storage::create_upload_dir(session_id, &upload_id)?;
    registry
        .lock()
        .unwrap()
        .insert(upload_id.clone(), session_id.to_string());
    send_message(
        stream,
        &Message::InitAck {
            chunk_size: 0,
            upload_id,
        },
    )
}

fn handle_chunk_meta(
    encryption_key: &[u8; 32],
    stream: &mut TcpStream,
    upload_id: String,
    chunk_index: u64,
    size: usize,
    hash: String,
    registry: &Arc<UploadRegistry>,
) -> Result<(), ParaFlowError> {
    if size > MAX_CHUNK_BYTES {
        return Err(ParaFlowError::ProtocolError(format!(
            "Chunk size {} exceeds maximum {}",
            size, MAX_CHUNK_BYTES
        )));
    }

    let session_id = registry
        .lock()
        .unwrap()
        .get(&upload_id)
        .cloned()
        .ok_or_else(|| ParaFlowError::SecurityError("Unknown upload id".into()))?;

    let mut encrypted_data = vec![0u8; size];
    stream.read_exact(&mut encrypted_data)?;

    let mut hasher = Sha256::new();
    hasher.update(&encrypted_data);
    let server_hash = hex::encode(hasher.finalize());

    if server_hash == hash {
        match encryption::decrypt_chunk(&encrypted_data, encryption_key) {
            Ok(decrypted_data) => {
                storage::save_chunk(&session_id, &upload_id, chunk_index, &decrypted_data)?;
                send_message(stream, &Message::ChunkAck { chunk_index })
            }
            Err(_) => send_message(stream, &Message::ChunkNack { chunk_index }),
        }
    } else {
        send_message(stream, &Message::ChunkNack { chunk_index })
    }
}

fn handle_complete(
    stream: &mut TcpStream,
    upload_id: String,
    file_name: String,
    total_chunks: u64,
    registry: &Arc<UploadRegistry>,
) -> Result<(), ParaFlowError> {
    let safe_name = sanitize_file_name(&file_name)
        .ok_or_else(|| ParaFlowError::ProtocolError("Invalid file name".into()))?;

    let session_id = registry
        .lock()
        .unwrap()
        .remove(&upload_id)
        .ok_or_else(|| ParaFlowError::SecurityError("Unknown upload id".into()))?;

    storage::merge_chunks(&session_id, &upload_id, &safe_name, total_chunks)?;
    send_message(stream, &Message::CompleteAck)
}

pub fn handle_client(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    registry: Arc<UploadRegistry>,
    rate_limiter: Arc<AuthRateLimiter>,
) -> Result<(), ParaFlowError> {
    let encryption_key = load_encryption_key()?;
    let mut state = SessionState::new(encryption_key);

    loop {
        let request = match read_message(&mut stream) {
            Ok(msg) => msg,
            Err(_) => return Ok(()),
        };

        match request {
            Message::LoginRequest { client_id } => {
                handle_login_request(&mut state, &mut stream, client_id, peer_addr, &rate_limiter)?;
            }
            Message::LoginAnswer { hash } => {
                handle_login_answer(&mut state, &mut stream, hash)?;
            }
            _ if !state.is_authenticated => {
                return Err(ParaFlowError::SecurityError("Unauthorized Access".into()));
            }
            Message::InitUpload { file_name, .. } => {
                let session_id = require_session_id(&state)?;
                handle_init_upload(&session_id, &mut stream, file_name, &registry)?;
            }
            Message::ChunkMeta {
                upload_id,
                chunk_index,
                size,
                hash,
            } => {
                handle_chunk_meta(
                    &state.encryption_key,
                    &mut stream,
                    upload_id,
                    chunk_index,
                    size,
                    hash,
                    &registry,
                )?;
            }
            Message::Complete {
                upload_id,
                file_name,
                total_chunks,
            } => {
                handle_complete(&mut stream, upload_id, file_name, total_chunks, &registry)?;
            }
            _ => {}
        }
    }
}
