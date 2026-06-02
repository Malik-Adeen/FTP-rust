mod auth;
mod handler;
mod rate_limit;
mod storage;

use clap::Parser;
use dotenvy;
use handler::UploadRegistry;
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Parser)]
struct Cli {
    #[arg(short, long, default_value_t = 7878)]
    port: u16,
}

fn check_required_env_vars() {
    let mut missing = Vec::new();
    if std::env::var("PARAFLOW_ENCRYPTION_KEY").is_err() {
        missing.push("PARAFLOW_ENCRYPTION_KEY");
    }
    if std::env::var("PARAFLOW_ADMIN_PASSWORD").is_err() {
        missing.push("PARAFLOW_ADMIN_PASSWORD");
    }
    if !missing.is_empty() {
        eprintln!("Fatal: required environment variables not set: {}", missing.join(", "));
        std::process::exit(1);
    }
}

fn main() {
    dotenvy::dotenv().ok();
    check_required_env_vars();

    let args = Cli::parse();
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = TcpListener::bind(&addr).expect("Could not bind to port");

    let registry: Arc<UploadRegistry> = Arc::new(Mutex::new(HashMap::new()));
    let rate_limiter = rate_limit::new_auth_limiter();

    println!("Server listening on {} ...", addr);

    for stream in listener.incoming() {
        if let Ok(s) = stream {
            let peer_addr = match s.peer_addr() {
                Ok(a) => a,
                Err(_) => continue,
            };
            let reg = Arc::clone(&registry);
            let rl = Arc::clone(&rate_limiter);
            thread::spawn(move || {
                if let Err(e) = handler::handle_client(s, peer_addr, reg, rl) {
                    eprintln!("Connection error: {}", e);
                }
            });
        }
    }
}
