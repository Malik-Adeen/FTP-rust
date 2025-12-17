use shared::Message;
use std::io::Write;
use std::net::TcpStream; // <--- Import our new library

fn main() {
    match TcpStream::connect("127.0.0.1:7878") {
        Ok(mut stream) => {
            println!("Connected!");

            // 1. Create a structured message using our Enum
            let command = Message::Hello {
                client_id: "ArchLinuxUser".to_string(),
            };

            // 2. Convert it to a JSON String
            let json_data = serde_json::to_string(&command).unwrap();

            // 3. Send it!
            stream.write_all(json_data.as_bytes()).unwrap();
            println!("Sent: {}", json_data);
        }
        Err(e) => println!("Failed: {}", e),
    }
}
