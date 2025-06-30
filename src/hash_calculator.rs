use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use md5::{Md5, Digest as Md5Digest};
use sha1::{Sha1, Digest as Sha1Digest};

pub fn calc_md5<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut md5 = Md5::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 { break; }
        md5.update(&buffer[..n]);
    }
    Ok(format!("{:x}", md5.finalize()))
}

pub fn calc_sha1<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut sha1 = Sha1::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 { break; }
        sha1.update(&buffer[..n]);
    }
    Ok(format!("{:x}", sha1.finalize()))
}

pub fn calc_md5_sha1<P: AsRef<Path>>(path: P) -> std::io::Result<(String, String)> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 { break; }
        md5.update(&buffer[..n]);
        sha1.update(&buffer[..n]);
    }
    Ok((format!("{:x}", md5.finalize()), format!("{:x}", sha1.finalize())))
}