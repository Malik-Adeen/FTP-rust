use rustls::ClientConfig;
use rustls::ClientConnection;
use rustls::StreamOwned;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest, Sha256};
use shared::{Message, ParaFlowError, encryption, load_encryption_key, read_message, send_message};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub threads: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7878,
            secret: "secret123".to_string(),
            threads: 4,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Started {
        file_id: usize,
        upload_id: String,
        total_chunks: u64,
        total_size: u64,
    },
    WorkerProgress {
        file_id: usize,
        worker_id: usize,
        chunk_index: u64,
        percent: u8,
    },
    ChunkDone {
        file_id: usize,
        worker_id: usize,
        chunk_index: u64,
        bytes_done: u64,
    },
    Done {
        file_id: usize,
    },
    Failed {
        file_id: usize,
        error: String,
    },
}

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &ServerName,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _msg: &[u8],
        _cert: &CertificateDer,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _msg: &[u8],
        _cert: &CertificateDer,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn connect_and_auth(
    address: &str,
    password: &str,
) -> Result<StreamOwned<ClientConnection, std::net::TcpStream>, ParaFlowError> {
    let tcp = std::net::TcpStream::connect(address)?;
    let host = address.split(':').next().unwrap_or("localhost").to_string();

    let client_config = Arc::new(
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth(),
    );

    let server_name: ServerName<'static> = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        ServerName::IpAddress(ip.into())
    } else {
        ServerName::try_from(host.clone())
            .map_err(|_| ParaFlowError::ProtocolError("Invalid server name".into()))?
            .to_owned()
    };

    let conn = ClientConnection::new(client_config, server_name)
        .map_err(|e| ParaFlowError::ProtocolError(e.to_string()))?;

    let mut stream = StreamOwned::new(conn, tcp);

    send_message(
        &mut stream,
        &Message::LoginRequest {
            client_id: "admin".to_string(),
        },
    )?;

    if let Message::LoginChallenge { salt } = read_message(&mut stream)? {
        let params = argon2::Params::new(19456, 2, 1, Some(32)).expect("valid argon2 params");
        let argon2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut output = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt.as_bytes(), &mut output)
            .expect("Argon2 failed");
        let answer = hex::encode(output);

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

pub fn read_chunk(filename: &str, chunk_index: u64) -> Vec<u8> {
    let mut file = File::open(filename).expect("File not found");
    let chunk_size = 4 * 1024 * 1024;
    file.seek(SeekFrom::Start(chunk_index * chunk_size))
        .unwrap();
    let mut buffer = Vec::new();
    let _ = file.take(chunk_size).read_to_end(&mut buffer);
    buffer
}

pub fn upload_file(
    file_id: usize,
    path: PathBuf,
    config: Config,
    progress_tx: std::sync::mpsc::Sender<ProgressEvent>,
) -> Result<(), String> {
    let encryption_key = match load_encryption_key() {
        Ok(key) => key,
        Err(err) => {
            let err_msg = format!("Missing/Invalid encryption key: {}", err);
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            return Err(err_msg);
        }
    };

    let filename = path.to_str().ok_or_else(|| {
        let err_msg = "Invalid filename path".to_string();
        let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
        err_msg
    })?;

    if !path.exists() {
        let err_msg = format!("File not found: {}", filename);
        let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
        return Err(err_msg);
    }

    let file_size = std::fs::metadata(&path).map_err(|e| {
        let err_msg = format!("Metadata error: {}", e);
        let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
        err_msg
    })?.len();

    let chunk_size = 4 * 1024 * 1024;
    let total_chunks = (file_size + chunk_size - 1) / chunk_size;
    let server_addr = format!("{}:{}", config.host, config.port);

    // Initial stream for Handshake and InitUpload
    let current_upload_id = {
        let mut setup_stream = match connect_and_auth(&server_addr, &config.secret) {
            Ok(s) => s,
            Err(e) => {
                let err_msg = format!("Connection Failed: {}", e);
                let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
                return Err(err_msg);
            }
        };

        if let Err(e) = send_message(
            &mut setup_stream,
            &Message::InitUpload {
                file_name: filename.to_string(),
                total_size: file_size,
            },
        ) {
            let err_msg = format!("InitUpload failed: {}", e);
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            return Err(err_msg);
        }

        match read_message(&mut setup_stream) {
            Ok(Message::InitAck { upload_id, .. }) => upload_id,
            Ok(Message::ErrorMessage { text }) => {
                let err_msg = format!("Upload Rejected: {}", text);
                let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
                return Err(err_msg);
            }
            Err(e) => {
                let err_msg = format!("InitUpload read response failed: {}", e);
                let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
                return Err(err_msg);
            }
            _ => {
                let err_msg = "Server sent unexpected message".to_string();
                let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
                return Err(err_msg);
            }
        }
    };

    // Send Started event
    let _ = progress_tx.send(ProgressEvent::Started {
        file_id,
        upload_id: current_upload_id.clone(),
        total_chunks,
        total_size: file_size,
    });

    let upload_id_arc = Arc::new(current_upload_id.clone());
    let secret_arc = Arc::new(config.secret.clone());
    let job_queue = Arc::new(Mutex::new((0..total_chunks).collect::<Vec<u64>>()));
    
    // Track worker errors in atomic bool/mutex
    let worker_error = Arc::new(Mutex::new(None));
    let mut handles = vec![];

    for worker_id in 0..config.threads {
        let queue = Arc::clone(&job_queue);
        let id = Arc::clone(&upload_id_arc);
        let pass = Arc::clone(&secret_arc);
        let addr = server_addr.clone();
        let fname = filename.to_string();
        let key = encryption_key;
        let tx = progress_tx.clone();
        let w_err = Arc::clone(&worker_error);

        handles.push(thread::spawn(move || {
            let mut stream = match connect_and_auth(&addr, &pass) {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = format!("Worker {} failed to authenticate: {}", worker_id, e);
                    let mut lock = w_err.lock().unwrap();
                    if lock.is_none() {
                        *lock = Some(err_msg.clone());
                    }
                    let _ = tx.send(ProgressEvent::Failed { file_id, error: err_msg });
                    return;
                }
            };

            loop {
                // If another worker encountered a fatal error, exit
                if w_err.lock().unwrap().is_some() {
                    break;
                }

                let chunk_index = {
                    let mut q = queue.lock().unwrap();
                    match q.pop() {
                        Some(i) => i,
                        None => break,
                    }
                };

                loop {
                    // Send initial worker progress of 0%
                    let _ = tx.send(ProgressEvent::WorkerProgress {
                        file_id,
                        worker_id,
                        chunk_index,
                        percent: 0,
                    });

                    let chunk_data = read_chunk(&fname, chunk_index);
                    let size_u64 = chunk_data.len() as u64;

                    let encrypted_chunk = match encryption::encrypt_chunk(&chunk_data, &key) {
                        Ok(c) => c,
                        Err(e) => {
                            let err_msg = format!("Encryption failed: {}", e);
                            let mut lock = w_err.lock().unwrap();
                            if lock.is_none() {
                                *lock = Some(err_msg.clone());
                            }
                            let _ = tx.send(ProgressEvent::Failed { file_id, error: err_msg });
                            return;
                        }
                    };

                    let mut hasher = Sha256::new();
                    hasher.update(&encrypted_chunk);
                    let hash = hex::encode(hasher.finalize());

                    if let Err(e) = send_message(
                        &mut stream,
                        &Message::ChunkMeta {
                            upload_id: id.to_string(),
                            chunk_index,
                            size: encrypted_chunk.len(),
                            hash,
                        },
                    ) {
                        let err_msg = format!("Send chunk meta failed: {}", e);
                        let mut lock = w_err.lock().unwrap();
                        if lock.is_none() {
                            *lock = Some(err_msg.clone());
                        }
                        let _ = tx.send(ProgressEvent::Failed { file_id, error: err_msg });
                        return;
                    }

                    // Write chunk in smaller pieces to show progress
                    let mut written = 0;
                    let chunk_len = encrypted_chunk.len();
                    let mut write_err = false;

                    while written < chunk_len {
                        if w_err.lock().unwrap().is_some() {
                            write_err = true;
                            break;
                        }
                        let to_write = std::cmp::min(64 * 1024, chunk_len - written);
                        if let Err(e) = stream.write_all(&encrypted_chunk[written..written+to_write]) {
                            let err_msg = format!("Write chunk data failed: {}", e);
                            let mut lock = w_err.lock().unwrap();
                            if lock.is_none() {
                                *lock = Some(err_msg.clone());
                            }
                            let _ = tx.send(ProgressEvent::Failed { file_id, error: err_msg });
                            write_err = true;
                            break;
                        }
                        written += to_write;
                        
                        let percent = ((written as f64 / chunk_len as f64) * 100.0) as u8;
                        let _ = tx.send(ProgressEvent::WorkerProgress {
                            file_id,
                            worker_id,
                            chunk_index,
                            percent,
                        });
                    }

                    if write_err {
                        return;
                    }

                    match read_message(&mut stream) {
                        Ok(Message::ChunkAck { .. }) => {
                            let _ = tx.send(ProgressEvent::ChunkDone {
                                file_id,
                                worker_id,
                                chunk_index,
                                bytes_done: size_u64,
                            });
                            break;
                        }
                        Ok(Message::ChunkNack { .. }) => {
                            thread::sleep(Duration::from_millis(500));
                        }
                        Err(e) => {
                            let err_msg = format!("Read chunk ack failed: {}", e);
                            let mut lock = w_err.lock().unwrap();
                            if lock.is_none() {
                                *lock = Some(err_msg.clone());
                            }
                            let _ = tx.send(ProgressEvent::Failed { file_id, error: err_msg });
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    // Check if any worker set a fatal error
    if let Some(err_msg) = worker_error.lock().unwrap().clone() {
        return Err(err_msg);
    }

    // Final merge connection
    let mut merge_stream = match connect_and_auth(&server_addr, &config.secret) {
        Ok(s) => s,
        Err(e) => {
            let err_msg = format!("Final connection for merge failed: {}", e);
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            return Err(err_msg);
        }
    };

    if let Err(e) = send_message(
        &mut merge_stream,
        &Message::Complete {
            upload_id: current_upload_id,
            file_name: filename.to_string(),
            total_chunks,
        },
    ) {
        let err_msg = format!("Complete message failed: {}", e);
        let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
        return Err(err_msg);
    }

    match read_message(&mut merge_stream) {
        Ok(Message::CompleteAck) => {
            let _ = progress_tx.send(ProgressEvent::Done { file_id });
            Ok(())
        }
        Ok(Message::ErrorMessage { text }) => {
            let err_msg = format!("Server merge failed: {}", text);
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            Err(err_msg)
        }
        Err(e) => {
            let err_msg = format!("Read CompleteAck failed: {}", e);
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            Err(err_msg)
        }
        _ => {
            let err_msg = "Unexpected response after Complete".to_string();
            let _ = progress_tx.send(ProgressEvent::Failed { file_id, error: err_msg.clone() });
            Err(err_msg)
        }
    }
}
