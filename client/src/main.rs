use argon2::{Algorithm, Argon2, Params, Version};
use clap::{Parser, Subcommand};
use hex;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
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

#[derive(Debug)]
// SAFETY: Disables certificate identity verification. Connection is still TLS-encrypted.
// For production use, replace with cert pinning against the server's self-signed cert.
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

const BANNER: &str = r#"
 ______                    _______ __
|   __ \.---.-.----.---.-.|    ___|  |.-----.--.--.--.
|    __/|  _  |   _|  _  ||    ___|  ||  _  |  |  |  |
|___|   |___._|__| |___._||___|   |__||_____|________|
"#;

fn connect_and_auth(
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
        let params = Params::new(19456, 2, 1, Some(32)).expect("valid argon2 params");
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
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
                    eprintln!("Missing encryption key: {}", err);
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

            let current_upload_id;
            {
                let mut setup_stream = match connect_and_auth(&server_addr, secret) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Connection Failed: {}", e);
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
                        eprintln!("Upload Rejected: {}", text);
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
                                        .set_message(format!("Chunk #{} Retry...", chunk_index));
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
            match read_message(&mut stream).unwrap() {
                Message::CompleteAck => println!("Upload complete. File reassembled on server."),
                Message::ErrorMessage { text } => {
                    eprintln!("Server error during merge: {}", text);
                    std::process::exit(1);
                }
                _ => eprintln!("Unexpected server response after Complete."),
            }
        }
    }
}
