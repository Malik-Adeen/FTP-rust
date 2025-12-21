use std::fs::{self, File};
use std::io::{self};

pub fn create_upload_dir(upload_id: &str) -> io::Result<()> {
    let path = format!("uploads/{}", upload_id);
    fs::create_dir_all(path)
}

pub fn save_chunk(upload_id: &str, chunk_index: u64, data: &[u8]) -> io::Result<()> {
    let path = format!("uploads/{}/chunk_{}", upload_id, chunk_index);
    fs::write(path, data)
}

pub fn merge_chunks(upload_id: &str, file_name: &str, total_chunks: u64) -> io::Result<()> {
    let temp_dir = format!("uploads/{}", upload_id);
    let output_path = format!("uploads/{}", file_name);

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

    fs::remove_dir_all(temp_dir)?;
    println!(">> Merge Complete. Saved to {}", output_path);
    Ok(())
}
