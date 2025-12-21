mod auth;
mod handler;
mod storage;

use clap::Parser;
use dotenvy;
use std::net::TcpListener;
use std::thread;

#[derive(Parser)]
struct Cli {
    #[arg(short, long, default_value_t = 7878)]
    port: u16,
}

fn main() {
    dotenvy::dotenv().ok();
    let args = Cli::parse();
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = TcpListener::bind(&addr).expect("Could not bind to port");

    println!("🌍 Server listening on {} ...", addr);

    for stream in listener.incoming() {
        if let Ok(s) = stream {
            thread::spawn(|| {
                if let Err(e) = handler::handle_client(s) {
                    eprintln!("Connection error: {}", e);
                }
            });
        }
    }
}
