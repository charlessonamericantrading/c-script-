use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Lockfile {
    pub version: u32,
    pub entries: BTreeMap<String, LockEntry>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

/// Computa un digest SHA-256 determinista en texto hex para un contenido fuente.
pub fn hash_source(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let k: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let bit_len = (bytes.len() as u64) * 8;
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut h_val = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h_val.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h_val = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(h_val);
    }

    format!(
        "{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]
    )
}

pub fn generate_lockfile(touched_files: &[PathBuf], project_root: &Path) -> Lockfile {
    let mut lock = Lockfile::new();
    let canon_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    for path in touched_files {
        let rel_path = path
            .strip_prefix(&canon_root)
            .or_else(|_| path.strip_prefix(project_root))
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.file_name().map(PathBuf::from).unwrap_or_else(|| path.clone()));

        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        if let Ok(source) = fs::read_to_string(path) {
            let hash = hash_source(&source);
            lock.entries.insert(
                rel_str.clone(),
                LockEntry {
                    path: rel_str,
                    hash,
                },
            );
        }
    }
    lock
}

pub fn verify_lockfile(lockfile: &Lockfile, project_root: &Path) -> Result<(), Vec<String>> {
    let mut mismatches = Vec::new();
    let canon_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    for (rel_path, entry) in &lockfile.entries {
        let file_path = canon_root.join(rel_path);
        if let Ok(source) = fs::read_to_string(&file_path) {
            let current_hash = hash_source(&source);
            if current_hash != entry.hash {
                mismatches.push(format!(
                    "El archivo '{}' ha cambiado desde la última generación del lockfile (esperado: {}, encontrado: {})",
                    rel_path, entry.hash, current_hash
                ));
            }
        } else {
            mismatches.push(format!("No se pudo leer el archivo '{}' declarado en link.lock", rel_path));
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches)
    }
}


pub fn write_lockfile(lockfile: &Lockfile, path: &Path) -> io::Result<()> {
    let json = serde_json::to_string_pretty(lockfile)?;
    fs::write(path, json)
}

pub fn read_lockfile(path: &Path) -> io::Result<Lockfile> {
    let content = fs::read_to_string(path)?;
    let lockfile: Lockfile = serde_json::from_str(&content)?;
    Ok(lockfile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hash_empty_string() {
        let hash = hash_source("");
        assert_eq!(hash, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn test_sha256_hash_hello_world() {
        let hash = hash_source("hello world");
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_lockfile_generation_and_roundtrip() {
        let dir = std::env::temp_dir().join("c_script_lockfile_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let f1 = dir.join("main.link");
        fs::write(&f1, "type User = { id: Int }").unwrap();

        let lock = generate_lockfile(std::slice::from_ref(&f1), &dir);
        assert_eq!(lock.entries.len(), 1);
        assert!(lock.entries.contains_key("main.link"));

        let lock_path = dir.join("link.lock");
        write_lockfile(&lock, &lock_path).unwrap();

        let read_back = read_lockfile(&lock_path).unwrap();
        assert_eq!(lock, read_back);

        let _ = fs::remove_dir_all(&dir);
    }

}
