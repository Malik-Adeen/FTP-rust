# Security Issues Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement session isolation, remove hardcoded encryption key usage, and refactor server handling into clear phases.

**Architecture:** Update the storage layer to scope uploads by session id, refactor `handle_client()` into small, explicit phase handlers, and load the encryption key from the environment in both client and server. Update README to reflect the new behavior and remaining integrity limitations.

**Tech Stack:** Rust, stdlib, `uuid`, `sha2`, `aes-gcm`, `serde`.

---

### Task 1: Session-Scoped Storage + Tests

**Files:**
- Modify: `server/src/storage.rs`
- Test: `server/src/storage.rs`

- [ ] **Step 1: Write failing tests for session-scoped storage**

Replace `server/src/storage.rs` with:

```rust
use std::fs::{self, File};
use std::io::{self};

pub fn create_upload_dir(session_id: &str, upload_id: &str) -> io::Result<()> {
    let path = format!("uploads/{}/{}", session_id, upload_id);
    fs::create_dir_all(path)
}

pub fn save_chunk(session_id: &str, upload_id: &str, chunk_index: u64, data: &[u8]) -> io::Result<()> {
    let path = format!("uploads/{}/{}/chunk_{}", session_id, upload_id, chunk_index);
    fs::write(path, data)
}

pub fn merge_chunks(session_id: &str, upload_id: &str, file_name: &str, total_chunks: u64) -> io::Result<()> {
    let temp_dir = format!("uploads/{}/{}", session_id, upload_id);
    let output_path = format!("uploads/{}/{}", session_id, file_name);

    println!(
        ">> Merging {} chunks from {} into {}...",
        total_chunks, temp_dir, output_path
    );

    let mut output_file = File::create(&output_path)?;

    for i in 0..total_chunks {
        let chunk_path = format!("{}/chunk_{}", temp_dir, i);
        let mut chunk_file = File::open(&chunk_path)?;
        std::io::copy(&mut chunk_file, &mut output_file)?;
    }

    fs::remove_dir_all(&temp_dir)?;
    let session_dir = format!("uploads/{}", session_id);
    if fs::read_dir(&session_dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
    {
        fs::remove_dir_all(&session_dir)?;
    }
    println!(">> Merge Complete. Saved to {}", output_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::path::Path;
    use uuid::Uuid;

    fn with_temp_dir<F: FnOnce()>(f: F) {
        let base = std::env::temp_dir().join(format!("paraflow_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&base).unwrap();
        f();
        std::env::set_current_dir(prev).unwrap();
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn creates_session_scoped_upload_dir() {
        with_temp_dir(|| {
            create_upload_dir("s1", "u1").unwrap();
            assert!(Path::new("uploads/s1/u1").exists());
        });
    }

    #[test]
    fn saves_and_merges_chunks_in_session_dir() {
        with_temp_dir(|| {
            create_upload_dir("s1", "u1").unwrap();
            save_chunk("s1", "u1", 0, b"hello").unwrap();
            save_chunk("s1", "u1", 1, b"world").unwrap();

            merge_chunks("s1", "u1", "out.bin", 2).unwrap();

            let mut merged = Vec::new();
            File::open("uploads/s1/out.bin")
                .unwrap()
                .read_to_end(&mut merged)
                .unwrap();
            assert_eq!(merged, b"helloworld");
            assert!(!Path::new("uploads/s1/u1").exists());
        });
    }
}
```

- [ ] **Step 2: Run storage tests (expect failures initially)**

Run: `cargo test -p server storage`
Expected: FAIL due to missing updated call sites.

- [ ] **Step 3: Commit Task 1**

```bash
git add server/src/storage.rs
git commit -m "test: add session-scoped storage coverage"
```

### Task 2: Refactor Server Handler + Session Isolation

**Files:**
- Modify: `server/src/handler.rs`

- [ ] **Step 1: Implement phased handlers and session state**

Replace `server/src/handler.rs` with:

```rust
use crate::{auth, storage};
use sha2::{Digest, Sha256};
use shared::{Message, ParaFlowError, encryption, load_encryption_key, read_message, send_message};
use std::io::Read;
use std::net::TcpStream;
use uuid::Uuid;

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

fn handle_login_request(
    state: &mut SessionState,
    stream: &mut TcpStream,
    client_id: String,
) -> Result<(), ParaFlowError> {
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

    if auth::verify_user("admin", &salt, &hash) {
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
) -> Result<(), ParaFlowError> {
    if file_name.ends_with(".sh") || file_name.ends_with(".exe") {
        send_message(
            stream,
            &Message::ErrorMessage {
                text: "Forbidden file type".into(),
            },
        )?;
        return Ok(());
    }

    let upload_id = Uuid::new_v4().to_string();
    storage::create_upload_dir(session_id, &upload_id)?;
    send_message(
        stream,
        &Message::InitAck {
            chunk_size: 0,
            upload_id,
        },
    )
}

fn handle_chunk_meta(
    session_id: &str,
    encryption_key: &[u8; 32],
    stream: &mut TcpStream,
    upload_id: String,
    chunk_index: u64,
    size: usize,
    hash: String,
) -> Result<(), ParaFlowError> {
    let mut encrypted_data = vec![0u8; size];
    stream.read_exact(&mut encrypted_data)?;

    let mut hasher = Sha256::new();
    hasher.update(&encrypted_data);
    let server_hash = hex::encode(hasher.finalize());

    if server_hash == hash {
        match encryption::decrypt_chunk(&encrypted_data, encryption_key) {
            Ok(decrypted_data) => {
                storage::save_chunk(session_id, &upload_id, chunk_index, &decrypted_data)?;
                send_message(stream, &Message::ChunkAck { chunk_index })
            }
            Err(_) => send_message(stream, &Message::ChunkNack { chunk_index }),
        }
    } else {
        send_message(stream, &Message::ChunkNack { chunk_index })
    }
}

fn handle_complete(
    session_id: &str,
    upload_id: String,
    file_name: String,
    total_chunks: u64,
) -> Result<(), ParaFlowError> {
    storage::merge_chunks(session_id, &upload_id, &file_name, total_chunks)
}

pub fn handle_client(mut stream: TcpStream) -> Result<(), ParaFlowError> {
    let encryption_key = load_encryption_key()?;
    let mut state = SessionState::new(encryption_key);

    loop {
        let request = match read_message(&mut stream) {
            Ok(msg) => msg,
            Err(_) => return Ok(()),
        };

        match request {
            Message::LoginRequest { client_id } => {
                handle_login_request(&mut state, &mut stream, client_id)?;
            }
            Message::LoginAnswer { hash } => {
                handle_login_answer(&mut state, &mut stream, hash)?;
            }
            _ if !state.is_authenticated => {
                return Err(ParaFlowError::SecurityError("Unauthorized Access".into()));
            }
            Message::InitUpload { file_name, .. } => {
                let session_id = require_session_id(&state)?;
                handle_init_upload(&session_id, &mut stream, file_name)?;
            }
            Message::ChunkMeta {
                upload_id,
                chunk_index,
                size,
                hash,
            } => {
                let session_id = require_session_id(&state)?;
                handle_chunk_meta(
                    &session_id,
                    &state.encryption_key,
                    &mut stream,
                    upload_id,
                    chunk_index,
                    size,
                    hash,
                )?;
            }
            Message::Complete {
                upload_id,
                file_name,
                total_chunks,
            } => {
                let session_id = require_session_id(&state)?;
                handle_complete(&session_id, upload_id, file_name, total_chunks)?;
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 2: Run server tests**

Run: `cargo test -p server`
Expected: PASS.

- [ ] **Step 3: Commit Task 2**

```bash
git add server/src/handler.rs
git commit -m "refactor: split handler phases and add session isolation"
```

### Task 3: Client Env Key + README Updates

**Files:**
- Modify: `client/src/main.rs`
- Modify: `README.md`

- [ ] **Step 1: Load encryption key from env in client**

Replace `client/src/main.rs` with:

```rust
use clap::{Parser, Subcommand};
use hex;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use shared::{Message, ParaFlowError, encryption, load_encryption_key, read_message, send_message};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ParaFlow Client")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Upload {
        #[arg(short, long)]
        file: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(short, long, default_value_t = 7878)]
        port: u16,
        #[arg(short, long, default_value_t = 4)]
        threads: usize,
        #[arg(long, default_value = "secret123")]
        secret: String,
    },
}

const BANNER: &str = r#"
 ______                    _______ __                 
|   __ \.---.-.----.---.-.|    ___|  |.-----.--.--.--.
|    __/|  _  |   _|  _  ||    ___|  ||  _  |  |  |  |
|___|   |___._|__| |___._||___|   |__||_____|________|
"#;

fn connect_and_auth(address: &str, password: &str) -> Result<TcpStream, ParaFlowError> {
    let mut stream = TcpStream::connect(address)?;

    send_message(
        &mut stream,
        &Message::LoginRequest {
            client_id: "admin".to_string(),
        },
    )?;

    if let Message::LoginChallenge { salt } = read_message(&mut stream)? {
        let combined = format!("{}{}", password, salt);
        let mut hasher = Sha256::new();
        hasher.update(combined.as_bytes());
        let answer = hex::encode(hasher.finalize());

        send_message(&mut stream, &Message::LoginAnswer { hash: answer })?;

        match read_message(&mut stream)? {
            Message::Welcome { .. } => Ok(stream),
            Message::ErrorMessage { text } => Err(ParaFlowError::AuthError(text)),
            _ => Err(ParaFlowError::ProtocolError(
                "Unexpected message during auth".into(),
            )),
        }
    } else {
        Err(ParaFlowError::ProtocolError("Expected Challenge".into()))
    }
}

fn read_chunk(filename: &str, chunk_index: u64) -> Vec<u8> {
    let mut file = File::open(filename).expect("File not found");
    let chunk_size = 4 * 1024 * 1024;
    file.seek(SeekFrom::Start(chunk_index * chunk_size))
        .unwrap();
    let mut buffer = Vec::new();
    let _ = file.take(chunk_size).read_to_end(&mut buffer);
    buffer
}

fn main() {
    println!("\x1b[36m{}\x1b[0m", BANNER);
    let cli = Cli::parse();

    match &cli.command {
        Commands::Upload {
            file,
            host,
            port,
            threads,
            secret,
        } => {
            let encryption_key = match load_encryption_key() {
                Ok(key) => key,
                Err(err) => {
                    eprintln!("❌ Missing encryption key: {}", err);
                    std::process::exit(1);
                }
            };

            let filename = file.to_str().expect("Invalid filename");
            if !file.exists() {
                eprintln!("Error: File not found");
                return;
            }

            let file_size = std::fs::metadata(file).unwrap().len();
            let chunk_size = 4 * 1024 * 1024;
            let total_chunks = (file_size + chunk_size - 1) / chunk_size;
            let server_addr = format!("{}:{}", host, port);

            let m = MultiProgress::new();
            let pb_total = m.add(ProgressBar::new(file_size));
            pb_total.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap().progress_chars("#>-"));
            pb_total.set_message("Total Progress");

            let mut current_upload_id = String::new();
            {
                let mut setup_stream = match connect_and_auth(&server_addr, secret) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("❌ Connection Failed: {}", e);
                        std::process::exit(1);
                    }
                };

                send_message(
                    &mut setup_stream,
                    &Message::InitUpload {
                        file_name: filename.to_string(),
                        total_size: file_size,
                    },
                )
                .unwrap();

                match read_message(&mut setup_stream).unwrap() {
                    Message::InitAck { upload_id, .. } => {
                        println!("Authorized! Upload ID: {}", upload_id);
                        current_upload_id = upload_id;
                    }
                    Message::ErrorMessage { text } => {
                        eprintln!("❌ Upload Rejected: {}", text);
                        std::process::exit(1);
                    }
                    _ => panic!("Server sent unexpected message"),
                }
            }

            let upload_id_arc = Arc::new(current_upload_id.clone());
            let secret_arc = Arc::new(secret.clone());
            let job_queue = Arc::new(Mutex::new((0..total_chunks).collect::<Vec<u64>>()));
            let mut handles = vec![];

            for worker_id in 0..*threads {
                let queue = Arc::clone(&job_queue);
                let id = Arc::clone(&upload_id_arc);
                let pass = Arc::clone(&secret_arc);
                let addr = server_addr.clone();
                let fname = filename.to_string();
                let key = encryption_key;

                let pb_worker = m.add(ProgressBar::new_spinner());
                pb_worker.set_style(
                    ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {msg}").unwrap(),
                );
                pb_worker.set_prefix(format!("Worker {}", worker_id));
                let pb_total_clone = pb_total.clone();

                handles.push(thread::spawn(move || {
                    let mut stream =
                        connect_and_auth(&addr, &pass).expect("Worker failed to authenticate");
                    pb_worker.set_message("Connected");

                    loop {
                        let chunk_index = {
                            let mut q = queue.lock().unwrap();
                            match q.pop() {
                                Some(i) => i,
                                None => break,
                            }
                        };

                        loop {
                            pb_worker.set_message(format!("Uploading Chunk #{}", chunk_index));
                            let chunk_data = read_chunk(&fname, chunk_index);
                            let size_u64 = chunk_data.len() as u64;

                            let encrypted_chunk =
                                encryption::encrypt_chunk(&chunk_data, &key)
                                    .expect("Encryption failed");

                            let mut hasher = Sha256::new();
                            hasher.update(&encrypted_chunk);
                            let hash = hex::encode(hasher.finalize());

                            send_message(
                                &mut stream,
                                &Message::ChunkMeta {
                                    upload_id: id.to_string(),
                                    chunk_index,
                                    size: encrypted_chunk.len(),
                                    hash,
                                },
                            )
                            .unwrap();

                            stream.write_all(&encrypted_chunk).unwrap();

                            match read_message(&mut stream).unwrap() {
                                Message::ChunkAck { .. } => {
                                    pb_total_clone.inc(size_u64);
                                    break;
                                }
                                Message::ChunkNack { .. } => {
                                    pb_worker
                                        .set_message(format!("⚠️ Chunk #{} Retry...", chunk_index));
                                    thread::sleep(Duration::from_millis(500));
                                }
                                _ => {}
                            }
                        }
                    }
                    pb_worker.finish_with_message("Done");
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            pb_total.finish_with_message("Upload Complete!");

            let mut stream =
                connect_and_auth(&server_addr, secret).expect("Final completion failed");
            send_message(
                &mut stream,
                &Message::Complete {
                    upload_id: current_upload_id,
                    file_name: filename.to_string(),
                    total_chunks,
                },
            )
            .unwrap();
            println!("Done.");
        }
    }
}
```

- [ ] **Step 2: Update README to reflect new behavior**

Replace `README.md` with:

```markdown
<p align="center">
<pre>
 ███████████                                ███████████ ████                          
░░███░░░░░███                              ░░███░░░░░░█░░███                          
 ░███    ░███  ██████   ████████   ██████   ░███   █ ░  ░███   ██████  █████ ███ █████
 ░██████████  ░░░░░███ ░░███░░███ ░░░░░███  ░███████    ░███  ███░░███░░███ ░███░░███ 
 ░███░░░░░░    ███████  ░███ ░░░   ███████  ░███░░░█    ░███ ░███ ░███ ░███ ░███ ░███ 
 ░███         ███░░███  ░███      ███░░███  ░███  ░     ░███ ░███ ░███ ░░███████████  
 █████       ░░████████ █████    ░░████████ █████       █████░░██████   ░░████░████   
░░░░░         ░░░░░░░░ ░░░░░      ░░░░░░░░ ░░░░░       ░░░░░  ░░░░░░     ░░░░ ░░░░    
</pre>
</p>

**ParaFlow** is a robust, concurrent file transfer solution engineered in Rust. It utilizes a multi-threaded architecture to split files into data chunks and transmit them in parallel across multiple TCP streams, effectively maximizing bandwidth utilization. The system prioritizes data integrity and fault tolerance through cryptographic verification and automatic error correction protocols.

## Key Features

* **Concurrency & Performance:** Implements a thread-pool architecture to facilitate the parallel transmission of file chunks, significantly reducing transfer times for large datasets.
* **Encrypted Transfers:** Chunks are encrypted on the client using AES-256-GCM and decrypted on the server before being saved.
* **Challenge-Response Authentication:** Secures the control channel using a Salted SHA-256 challenge-response mechanism. The server validates against `PARAFLOW_ADMIN_PASSWORD` (fallback: `default_fallback_change_me`) while the client defaults to `secret123` unless overridden via `--secret`.
* **Session Isolation:** Each successful login creates a UUID-based session id and isolates uploads under a per-session directory.
* **File Restrictions:** The server rejects uploads that end with `.sh` or `.exe`.
* **Custom Protocol:** JSON messages with length-prefix framing for control and metadata, plus raw encrypted bytes for chunk payloads.

> **Warning:** The README previously claimed full integrity verification and automatic re-queue on corruption. The current implementation only hashes the encrypted chunk bytes and compares the client-provided hash; it does not verify plaintext integrity or perform server-driven re-queue logic.
> **Warning:** The encryption key must be provided via `PARAFLOW_ENCRYPTION_KEY` (64 hex characters). The server and client will exit if it is not set.

## Installation

Ensure the Rust toolchain (Cargo) is installed on your system.

```bash
# Clone the repository
git clone https://github.com/yourusername/paraflow.git
cd paraflow

# Compile the project in release mode for optimal performance
cargo build --release

```

## Usage Guidelines

The system consists of two binaries: `server` (receiver) and `client` (sender).

### Server Configuration

The server initializes a listener for incoming TCP connections and manages the reassembly of file chunks.

```bash
# Start server on default port (7878)
cargo run -p server

# Start server on a specific port
cargo run -p server -- --port 9000

```

### Client Operations

The client handles file segmentation, hashing, and parallel distribution to worker threads.

```bash
# Standard upload
cargo run -p client -- upload --file data.bin

# High-performance upload (8 threads) to a remote host
cargo run -p client -- upload --file video.mp4 --host 192.168.1.50 --port 9000 --threads 8

# Authenticated upload (Default secret: 'secret123')
cargo run -p client -- upload --file sensitive.doc --secret <password>

```

## Architectural Overview

1. **Handshake & Authentication:** The client initiates a connection. The server responds with a cryptographic salt. The client computes the salted hash of the password and returns it for verification.
2. **Session Negotiation:** After successful authentication, the server sends a UUID session id and allocates a session-scoped staging directory.
3. **Parallel Distribution:** The client splits the source file into 4MB chunks. These tasks are distributed via a mutex-locked job queue to a pool of worker threads.
4. **Integrity Verification:** The server calculates the SHA-256 hash of the encrypted chunk bytes and compares it against the client-provided hash.
* **ACK:** Hash match. The decrypted chunk is committed to disk.
* **NACK:** Hash mismatch. The server rejects the chunk. The client retries based on its own retry loop.

5. **Final Assembly:** Once all chunks are successfully acknowledged, the server merges the segments into the final artifact and cleans up the staging area.

## Security Policies

* **Authentication:** The client defaults to `secret123` unless overridden with `--secret`. The server validates against `PARAFLOW_ADMIN_PASSWORD` and falls back to `default_fallback_change_me` if unset.
* **File Restrictions:** The server rejects `.exe` and `.sh` uploads.
* **Encryption Key:** The encryption key must be provided in `PARAFLOW_ENCRYPTION_KEY` (64 hex characters). This key is required by both client and server.

---
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Commit Task 3**

```bash
git add client/src/main.rs README.md
git commit -m "feat: load encryption key from env and update docs"
```
