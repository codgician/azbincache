use crate::compress::Compression;
use crate::narinfo;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct PathMeta {
    pub store_path: String,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub ca: Option<String>,
    pub sigs: Vec<String>,
}

/// Runtime closure of `paths`: every store path reachable via references,
/// including the inputs themselves (`nix-store --query --requisites`).
pub async fn closure(paths: &[String]) -> Result<Vec<String>> {
    let mut args = vec!["--query".to_string(), "--requisites".to_string()];
    args.extend(paths.iter().cloned());
    let out = run("nix-store", &args).await?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

pub async fn query_meta(store_path: &str) -> Result<PathMeta> {
    let nar_hash = run(
        "nix-store",
        &["--query".into(), "--hash".into(), store_path.into()],
    )
    .await?
    .trim()
    .to_string();
    let nar_hash = narinfo::normalize_nar_hash(&nar_hash)?;

    let size_raw = run(
        "nix-store",
        &["--query".into(), "--size".into(), store_path.into()],
    )
    .await?;
    let nar_size: u64 = size_raw.trim().parse().context("parsing nar size")?;

    let refs_raw = run(
        "nix-store",
        &["--query".into(), "--references".into(), store_path.into()],
    )
    .await?;
    let references: Vec<String> = refs_raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let deriver_raw = run(
        "nix-store",
        &["--query".into(), "--deriver".into(), store_path.into()],
    )
    .await?;
    let deriver = match deriver_raw.trim() {
        "" | "unknown-deriver" => None,
        d => Some(d.to_string()),
    };

    let sigs = query_sigs(store_path).await.unwrap_or_default();

    Ok(PathMeta {
        store_path: store_path.to_string(),
        nar_hash,
        nar_size,
        references,
        deriver,
        ca: None,
        sigs,
    })
}

async fn query_sigs(store_path: &str) -> Result<Vec<String>> {
    let out = run(
        "nix",
        &[
            "path-info".into(),
            "--json".into(),
            "--sigs".into(),
            store_path.into(),
        ],
    )
    .await?;
    let parsed: serde_json::Value = serde_json::from_str(&out)?;
    let mut sigs = Vec::new();
    if let Some(obj) = parsed.as_object() {
        for (_k, v) in obj {
            if let Some(arr) = v.get("signatures").and_then(|s| s.as_array()) {
                for s in arr {
                    if let Some(s) = s.as_str() {
                        sigs.push(s.to_string());
                    }
                }
            }
        }
    }
    Ok(sigs)
}

pub struct CompressedNar {
    pub file: tempfile::NamedTempFile,
    pub file_hash: String,
    pub file_size: u64,
    pub nar_hash: String,
    pub nar_size: u64,
}

const STREAM_CHUNK: usize = 256 * 1024;

/// Stream `nix store dump-path` through the chosen compressor into a temp file
/// with bounded memory: the raw NAR is hashed on the fly (`NarHash`) while being
/// compressed, then the compressed temp file is hashed (`FileHash`). Peak
/// resident memory is the chunk buffer plus the encoder window, independent of
/// NAR size.
pub async fn dump_and_compress(
    store_path: &str,
    compression: Compression,
) -> Result<CompressedNar> {
    let mut child = Command::new("nix")
        .args(["store", "dump-path", store_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning nix store dump-path")?;
    let mut stdout = child.stdout.take().context("capturing dump-path stdout")?;

    let tmp = tempfile::NamedTempFile::new().context("creating temp NAR file")?;
    let (nar_hash, nar_size) = {
        let out = tmp.reopen().context("reopening temp NAR file")?;
        let mut encoder = compression.encoder(out).context("init NAR encoder")?;
        let mut nar_hasher = Sha256::new();
        let mut nar_size: u64 = 0;
        let mut buf = vec![0u8; STREAM_CHUNK];
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            nar_hasher.update(&buf[..n]);
            nar_size += n as u64;
            encoder.write_all(&buf[..n]).context("writing NAR input")?;
        }
        encoder.finish().context("finalizing compressed stream")?;
        let nar_hash = format!(
            "sha256:{}",
            crate::nixbase32::encode(&nar_hasher.finalize())
        );
        (nar_hash, nar_size)
    };

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("nix store dump-path failed for {store_path}");
    }

    let (file_hash, file_size) = hash_file(tmp.path())?;

    Ok(CompressedNar {
        file: tmp,
        file_hash,
        file_size,
        nar_hash,
        nar_size,
    })
}

fn hash_file(path: &std::path::Path) -> Result<(String, u64)> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).context("opening compressed NAR for hashing")?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut buf = vec![0u8; STREAM_CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let hash = format!("sha256:{}", crate::nixbase32::encode(&hasher.finalize()));
    Ok((hash, size))
}

async fn run(program: &str, args: &[String]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_file_matches_independent_digest_and_size() {
        let payload = vec![7u8; 300 * 1024];
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &payload).unwrap();

        let (hash, size) = hash_file(tmp.path()).unwrap();

        let expected = format!(
            "sha256:{}",
            crate::nixbase32::encode(&Sha256::digest(&payload))
        );
        assert_eq!(hash, expected);
        assert_eq!(size, payload.len() as u64);
    }

    #[test]
    fn zstd_temp_file_roundtrips_and_hash_is_over_compressed_bytes() {
        let raw = vec![42u8; 512 * 1024];
        let tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let mut enc = zstd::stream::Encoder::new(tmp.reopen().unwrap(), 3).unwrap();
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
        }

        let decompressed = zstd::decode_all(std::fs::File::open(tmp.path()).unwrap()).unwrap();
        assert_eq!(decompressed, raw);

        let (file_hash, file_size) = hash_file(tmp.path()).unwrap();
        let compressed = std::fs::read(tmp.path()).unwrap();
        assert_eq!(
            file_hash,
            format!(
                "sha256:{}",
                crate::nixbase32::encode(&Sha256::digest(&compressed))
            )
        );
        assert_eq!(file_size, compressed.len() as u64);
        assert!(file_size < raw.len() as u64);
    }
}
