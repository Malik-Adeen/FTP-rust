use std::fs::{self, File};
use std::io::{self};

pub fn create_upload_dir(session_id: &str, upload_id: &str) -> io::Result<()> {
    let path = format!("uploads/{}/{}", session_id, upload_id);
    fs::create_dir_all(path)
}

pub fn save_chunk(
    session_id: &str,
    upload_id: &str,
    chunk_index: u64,
    data: &[u8],
) -> io::Result<()> {
    let path = format!("uploads/{}/{}/chunk_{}", session_id, upload_id, chunk_index);
    fs::write(path, data)
}

pub fn merge_chunks(
    session_id: &str,
    upload_id: &str,
    file_name: &str,
    total_chunks: u64,
) -> io::Result<()> {
    let temp_dir = format!("uploads/{}/{}", session_id, upload_id);
    let output_path = format!("uploads/{}/{}", session_id, file_name);

    println!(
        ">> Merging {} chunks from {} into {}...",
        total_chunks, temp_dir, output_path
    );

    let mut output_file = File::create(&output_path)?;

    for i in 0..total_chunks {
        let chunk_path = format!("{}/chunk_{}", temp_dir, i);
        let mut chunk_file = File::open(&chunk_path)?;
        std::io::copy(&mut chunk_file, &mut output_file)?;
    }

    fs::remove_dir_all(&temp_dir)?;
    let session_dir = format!("uploads/{}", session_id);
    if fs::read_dir(&session_dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
    {
        fs::remove_dir_all(&session_dir)?;
    }
    println!(">> Merge Complete. Saved to {}", output_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::path::Path;
    use uuid::Uuid;

    fn with_temp_dir<F: FnOnce()>(f: F) {
        let base = std::env::temp_dir().join(format!("paraflow_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&base).unwrap();
        f();
        std::env::set_current_dir(prev).unwrap();
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn creates_session_scoped_upload_dir() {
        with_temp_dir(|| {
            create_upload_dir("s1", "u1").unwrap();
            assert!(Path::new("uploads/s1/u1").exists());
        });
    }

    #[test]
    fn saves_and_merges_chunks_in_session_dir() {
        with_temp_dir(|| {
            create_upload_dir("s1", "u1").unwrap();
            save_chunk("s1", "u1", 0, b"hello").unwrap();
            save_chunk("s1", "u1", 1, b"world").unwrap();

            merge_chunks("s1", "u1", "out.bin", 2).unwrap();

            let mut merged = Vec::new();
            File::open("uploads/s1/out.bin")
                .unwrap()
                .read_to_end(&mut merged)
                .unwrap();
            assert_eq!(merged, b"helloworld");
            assert!(!Path::new("uploads/s1/u1").exists());
        });
    }
}
