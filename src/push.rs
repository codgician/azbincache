use crate::compress::Compression;
use crate::fingerprint;
use crate::narinfo::{self, NarInfo};
use crate::nix;
use crate::sign::NixSigningKey;
use crate::storage::StorageBackend;
use anyhow::{Context, Result};

const CONTENT_TYPE_NARINFO: &str = "text/x-nix-narinfo";
const CONTENT_TYPE_NAR: &str = "application/x-nix-nar";

pub struct PushOptions {
    pub store_dir: String,
    pub priority: u32,
    pub compression: Compression,
}

impl Default for PushOptions {
    fn default() -> Self {
        Self {
            store_dir: "/nix/store".to_string(),
            priority: 41,
            compression: Compression::Zstd(3),
        }
    }
}

pub async fn push_path(
    backend: &dyn StorageBackend,
    key: &NixSigningKey,
    store_path: &str,
    opts: &PushOptions,
) -> Result<bool> {
    let store_hash = narinfo::store_path_hash(store_path);
    let narinfo_key = format!("{store_hash}.narinfo");

    if backend.exists(&narinfo_key).await? {
        return Ok(false);
    }

    let meta = nix::query_meta(store_path)
        .await
        .with_context(|| format!("querying metadata for {store_path}"))?;

    let nar = nix::dump_and_compress(store_path, opts.compression)
        .await
        .with_context(|| format!("dumping NAR for {store_path}"))?;

    if nar.nar_hash != meta.nar_hash {
        anyhow::bail!(
            "NarHash mismatch for {store_path}: dump={} meta={}",
            nar.nar_hash,
            meta.nar_hash
        );
    }

    let file_hash_bare = nar
        .file_hash
        .strip_prefix("sha256:")
        .unwrap_or(&nar.file_hash);
    let nar_key = format!("nar/{file_hash_bare}.{}", opts.compression.nar_extension());

    backend
        .put_file(&nar_key, nar.file.path(), CONTENT_TYPE_NAR)
        .await
        .with_context(|| format!("uploading {nar_key}"))?;

    let abs_refs = fingerprint::references_to_absolute(&opts.store_dir, &meta.references);
    let fp = fingerprint::fingerprint(store_path, &meta.nar_hash, meta.nar_size, &abs_refs);
    let our_sig = key.sign_to_field(fp.as_bytes());

    let mut sigs = meta.sigs.clone();
    if !sigs.contains(&our_sig) {
        sigs.push(our_sig);
    }

    let short_refs = meta
        .references
        .iter()
        .map(|r| narinfo::short_reference(r).to_string())
        .collect();
    let deriver = meta
        .deriver
        .as_deref()
        .map(|d| narinfo::short_reference(d).to_string());

    let info = NarInfo {
        store_path: store_path.to_string(),
        url: nar_key,
        compression: opts.compression.narinfo_name().to_string(),
        file_hash: nar.file_hash,
        file_size: nar.file_size,
        nar_hash: meta.nar_hash,
        nar_size: meta.nar_size,
        references: short_refs,
        deriver,
        ca: meta.ca,
        sigs,
    };

    backend
        .put_bytes(
            &narinfo_key,
            info.render().into_bytes(),
            CONTENT_TYPE_NARINFO,
        )
        .await
        .with_context(|| format!("uploading {narinfo_key}"))?;

    Ok(true)
}
