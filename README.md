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

> **Warning:** The current implementation only hashes the encrypted chunk bytes and compares the client-provided hash; it does not verify plaintext integrity or perform server-driven re-queue logic.
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
