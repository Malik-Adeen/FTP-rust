use clap::{Parser, Subcommand};
use hex;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use shared::Message;
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

        // The secret password flag
        #[arg(long, default_value = "secret123")]
        secret: String,
    },
}

const BANNER: &str = r#"
 ███████████                                 ███████████ ████                         
░░███░░░░░███                               ░░███░░░░░░█░░███                         
 ░███     ░███  ██████    ████████   ██████  ░███   █ ░  ░███    ██████  █████ ███ █████
 ░██████████   ░░░░░███  ░░███░░███ ░░░░░███  ░███████    ░███   ███░░███░░███ ░███░░███ 
 ░███░░░░░░     ███████   ░███ ░░░   ███████  ░███░░░█    ░███  ░███ ░███ ░███ ░███ ░███ 
 ░███          ███░░███   ░███      ███░░███  ░███  ░     ░███  ░███ ░███ ░░███████████  
 █████        ░░████████  █████    ░░████████ █████       █████ ░░██████   ░░████░████   
░░░░░          ░░░░░░░░  ░░░░░      ░░░░░░░░ ░░░░░       ░░░░░   ░░░░░░     ░░░░ ░░░░    
"#;

// UPDATED: Now accepts 'password' argument
fn connect_and_auth(address: &str, password: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).expect("Failed to connect");

    // 1. Login Request
    send_message(
        &mut stream,
        &Message::LoginRequest {
            client_id: "admin".to_string(),
        },
    );

    // 2. Get Challenge
    let response = read_message(&mut stream);
    if let Message::LoginChallenge { salt } = response {
        // 3. Solve Puzzle
        let combined = format!("{}{}", password, salt);
        let mut hasher = Sha256::new();
        hasher.update(combined.as_bytes());
        let answer = hex::encode(hasher.finalize());

        // 4. Send Answer
        send_message(&mut stream, &Message::LoginAnswer { hash: answer });

        // 5. Check Result
        match read_message(&mut stream) {
            Message::Welcome { .. } => return stream, // Success!
            Message::ErrorMessage { text } => {
                eprintln!("❌ Login Failed: {}", text);
                std::process::exit(1); // Exit cleanly
            }
            _ => panic!("Protocol Error"),
        }
    } else {
        panic!("Protocol Error: Expected Challenge");
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
                .unwrap()
                .progress_chars("#>-"));
            pb_total.set_message("Total Progress");

            // println!("🚀 Connecting to {} (Auth Enabled)...", server_addr);

            // --- 1. SETUP PHASE ---
            let mut current_upload_id = String::new();
            {
                // <--- CHANGE 1: PASS SECRET HERE
                let mut setup_stream = connect_and_auth(&server_addr, secret);

                send_message(
                    &mut setup_stream,
                    &Message::InitUpload {
                        file_name: filename.to_string(),
                        total_size: file_size,
                    },
                );

                match read_message(&mut setup_stream) {
                    Message::InitAck { upload_id, .. } => {
                        println!("Authorized! Upload ID: {}", upload_id);
                        current_upload_id = upload_id;
                    }
                    Message::ErrorMessage { text } => {
                        eprintln!("❌ Upload Rejected: {}", text);
                        std::process::exit(1); // Exit cleanly
                    }
                    _ => panic!("Server sent unexpected message"),
                }
            }

            // --- 2. WORKER PHASE ---
            let upload_id_arc = Arc::new(current_upload_id.clone());

            // <--- CRITICAL: WRAP SECRET IN ARC FOR THREADS
            let secret_arc = Arc::new(secret.clone());

            let job_queue = Arc::new(Mutex::new((0..total_chunks).collect::<Vec<u64>>()));
            let mut handles = vec![];

            for worker_id in 0..*threads {
                let queue = Arc::clone(&job_queue);
                let id = Arc::clone(&upload_id_arc);

                // <--- CRITICAL: CLONE ARC FOR THIS SPECIFIC THREAD
                let pass = Arc::clone(&secret_arc);

                let addr = server_addr.clone();
                let fname = filename.to_string();

                let pb_worker = m.add(ProgressBar::new_spinner());
                pb_worker.set_style(
                    ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {msg}").unwrap(),
                );
                pb_worker.set_prefix(format!("Worker {}", worker_id));

                let pb_total_clone = pb_total.clone();

                handles.push(thread::spawn(move || {
                    // <--- CHANGE 2: PASS SECRET HERE
                    let mut stream = connect_and_auth(&addr, &pass);
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
                            let mut hasher = Sha256::new();
                            hasher.update(&chunk_data);
                            let hash = hex::encode(hasher.finalize());

                            send_message(
                                &mut stream,
                                &Message::ChunkMeta {
                                    upload_id: id.to_string(),
                                    chunk_index,
                                    size: chunk_data.len(),
                                    hash,
                                },
                            );
                            stream.write_all(&chunk_data).unwrap();

                            match read_message(&mut stream) {
                                Message::ChunkAck { .. } => {
                                    // println!(
                                    //     "Worker {} Chunk #{} Success.",
                                    //     worker_id, chunk_index
                                    // );
                                    pb_total_clone.inc(size_u64);
                                    break;
                                }
                                Message::ChunkNack { .. } => {
                                    pb_worker
                                        .set_message(format!("⚠️ Chunk #{} Retry...", chunk_index));
                                    thread::sleep(Duration::from_millis(500)); // Slow down retry slightly so we can see it
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

            // --- 3. COMPLETE PHASE ---
            // <--- CHANGE 3: PASS SECRET HERE
            let mut stream = connect_and_auth(&server_addr, secret);

            send_message(
                &mut stream,
                &Message::Complete {
                    upload_id: current_upload_id,
                    file_name: filename.to_string(),
                    total_chunks: total_chunks, // <--- SEND IT HERE
                },
            );
            println!("Done.");
        }
    }
}
