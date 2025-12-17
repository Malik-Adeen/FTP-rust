use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use shared::Message;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const WORKER_COUNT: usize = 4;

#[derive(Parser)]
#[command(name = "ParaFlow Client")]
#[command(version = "1.0")]
#[command(about = "High-performance parallel file uploader", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload a file to the server
    Upload {
        /// The file to upload
        #[arg(short, long)]
        file: PathBuf,

        /// Server IP address (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Server Port (default: 7878)
        #[arg(short, long, default_value_t = 7878)]
        port: u16,

        /// Number of parallel threads (default: 4)
        #[arg(short, long, default_value_t = 4)]
        threads: usize,
    },
}

// ... (keep read_chunk, send_message, read_message, connect_and_auth exactly as they are) ...
fn read_chunk(filename: &str, chunk_index: u64) -> Vec<u8> {
    let mut file = File::open(filename).expect("File not found");
    file.seek(SeekFrom::Start(chunk_index * CHUNK_SIZE))
        .expect("Seek failed");
    let mut buffer = Vec::new();
    let _ = file.take(CHUNK_SIZE).read_to_end(&mut buffer);
    buffer
}

fn send_message(stream: &mut TcpStream, msg: &Message) {
    let json = serde_json::to_string(msg).unwrap();
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
}

fn read_message(stream: &mut TcpStream) -> Message {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut json_buf = vec![0u8; len];
    stream.read_exact(&mut json_buf).unwrap();
    let text = String::from_utf8_lossy(&json_buf);
    serde_json::from_str(&text).unwrap()
}

fn connect_and_auth(address: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).expect("Failed to connect");
    send_message(
        &mut stream,
        &Message::Hello {
            client_id: "Worker".to_string(),
        },
    );
    read_message(&mut stream);
    stream
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Upload {
            file,
            host,
            port,
            threads,
        } => {
            let filename = file.to_str().expect("Invalid filename");

            if !file.exists() {
                eprintln!("Error: File '{}' not found!", filename);
                return;
            }

            let file_size = std::fs::metadata(file).unwrap().len();
            let chunk_size = 4 * 1024 * 1024;
            let total_chunks = (file_size + chunk_size - 1) / chunk_size;
            let worker_count = *threads;

            // 1. Construct the address string
            let server_addr = format!("{}:{}", host, port);

            println!(
                "🚀 Connecting to {} with {} threads...",
                server_addr, worker_count
            );
            println!(
                "📂 Uploading: {} ({:.2} MB)",
                filename,
                file_size as f64 / 1024.0 / 1024.0
            );

            // VARIABLE TO STORE THE ID
            let mut current_upload_id = String::new();

            // ---------------------------------------------------------
            // FIX #1: Pass address to Setup Connection
            // ---------------------------------------------------------
            {
                println!("Initializing upload with server...");
                // Pass &server_addr here
                let mut setup_stream = connect_and_auth(&server_addr); // <--- FIX 1

                send_message(
                    &mut setup_stream,
                    &Message::InitUpload {
                        file_name: filename.to_string(),
                        total_size: file_size,
                    },
                );

                let response = read_message(&mut setup_stream);
                if let Message::InitAck { upload_id, .. } = response {
                    println!("Server assigned Upload ID: {}", upload_id);
                    current_upload_id = upload_id;
                } else {
                    panic!("Server did not send InitAck!");
                }
            }

            let upload_id_arc = Arc::new(current_upload_id.clone());
            let job_queue: Vec<u64> = (0..total_chunks).collect();
            let queue_ptr = Arc::new(Mutex::new(job_queue));
            let mut handles = vec![];

            for worker_id in 0..worker_count {
                let queue_ref = Arc::clone(&queue_ptr);
                let id_ref = Arc::clone(&upload_id_arc);
                let fname = filename.to_string();

                // ---------------------------------------------------------
                // FIX #2: Clone the address for the thread
                // ---------------------------------------------------------
                let addr_for_thread = server_addr.clone(); // <--- FIX 2 (Create a copy for this thread)

                let handle = thread::spawn(move || {
                    // Use the copy inside the thread
                    let mut stream = connect_and_auth(&addr_for_thread); // <--- FIX 2 (Use it)

                    loop {
                        let chunk_index = {
                            let mut queue = queue_ref.lock().unwrap();
                            match queue.pop() {
                                Some(idx) => idx,
                                None => break,
                            }
                        };

                        let mut attempts = 0;
                        loop {
                            attempts += 1;
                            let chunk_data = read_chunk(&fname, chunk_index);

                            let mut hasher = Sha256::new();
                            hasher.update(&chunk_data);
                            let hash_string = hex::encode(hasher.finalize());

                            send_message(
                                &mut stream,
                                &Message::ChunkMeta {
                                    upload_id: id_ref.to_string(),
                                    chunk_index,
                                    size: chunk_data.len(),
                                    hash: hash_string,
                                },
                            );

                            stream.write_all(&chunk_data).unwrap();

                            let response = read_message(&mut stream);
                            match response {
                                Message::ChunkAck { .. } => break,
                                Message::ChunkNack { .. } => {
                                    println!("Worker {} Retry...", worker_id)
                                }
                                _ => {}
                            }
                        }
                    }
                });
                handles.push(handle);
            }

            for handle in handles {
                handle.join().unwrap();
            }

            // ---------------------------------------------------------
            // FIX #3: Pass address to Complete Connection
            // ---------------------------------------------------------
            let mut stream = connect_and_auth(&server_addr); // <--- FIX 3

            send_message(
                &mut stream,
                &Message::Complete {
                    upload_id: current_upload_id,
                    file_name: filename.to_string(),
                },
            );
            println!("Done.");
        }
    }
}
