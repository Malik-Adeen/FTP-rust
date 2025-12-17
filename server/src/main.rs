use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

// This function runs inside the new thread.
// It handles ONE specific client and then exits.
fn handle_client(mut stream: TcpStream) {
    let mut buffer = [0; 512];

    // We loop here to keep the conversation going with this specific client
    loop {
        match stream.read(&mut buffer) {
            Ok(size) => {
                // If size is 0, the client disconnected politely.
                if size == 0 {
                    return;
                }

                let received = String::from_utf8_lossy(&buffer[..size]);
                println!("Received: {}", received);

                // Send a response back
                if let Err(e) = stream.write_all(b"Server ACK") {
                    println!("Failed to send response: {}", e);
                    return;
                }
            }
            Err(_) => {
                // Error (like sudden disconnect)
                println!("Client disconnected with error.");
                return;
            }
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    println!("Multi-threaded Server listening on port 7878...");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("New connection: {}", stream.peer_addr().unwrap());

                // SPAWN THREAD
                // 'move' forces the thread to take ownership of 'stream'.
                // This allows the main loop to let go of it and listen for the next person.
                thread::spawn(move || {
                    handle_client(stream);
                });
            }
            Err(e) => {
                println!("Connection failed: {}", e);
            }
        }
    }
}
