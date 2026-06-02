mod auth;
mod handler;
mod rate_limit;
mod storage;

use clap::Parser;
use dotenvy;
use handler::UploadRegistry;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConnection;
use rustls::StreamOwned;
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

fn build_tls_config() -> Arc<ServerConfig> {
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("Failed to generate TLS certificate");

    let cert_der = CertificateDer::from(cert.der().to_owned());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("Invalid TLS configuration");

    Arc::new(config)
}

fn main() {
    dotenvy::dotenv().ok();
    check_required_env_vars();

    let args = Cli::parse();
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = TcpListener::bind(&addr).expect("Could not bind to port");

    let tls_config = build_tls_config();
    let registry: Arc<UploadRegistry> = Arc::new(Mutex::new(HashMap::new()));
    let rate_limiter = rate_limit::new_auth_limiter();

    println!("Server listening on {} (TLS) ...", addr);

    for stream in listener.incoming() {
        if let Ok(tcp_stream) = stream {
            let peer_addr = match tcp_stream.peer_addr() {
                Ok(a) => a,
                Err(_) => continue,
            };
            let tls_conn = match ServerConnection::new(Arc::clone(&tls_config)) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("TLS init error: {}", e);
                    continue;
                }
            };
            let tls_stream = StreamOwned::new(tls_conn, tcp_stream);
            let reg = Arc::clone(&registry);
            let rl = Arc::clone(&rate_limiter);
            thread::spawn(move || {
                if let Err(e) = handler::handle_client(tls_stream, peer_addr, reg, rl) {
                    eprintln!("Connection error: {}", e);
                }
            });
        }
    }
}
