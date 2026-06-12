use crate::storage::StorageBackend;
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "manifests/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u32,
    pub commit: String,
    pub commit_time: i64,
    pub host: String,
    pub closure: Vec<String>,
}

impl Manifest {
    pub fn new(commit: String, commit_time: i64, host: String, closure: Vec<String>) -> Self {
        Self {
            schema: 1,
            commit,
            commit_time,
            host,
            closure,
        }
    }

    pub fn key(&self) -> String {
        let safe_host = self.host.replace('/', "_");
        format!("{PREFIX}{}__{}.json", self.commit, safe_host)
    }
}

pub async fn write(backend: &dyn StorageBackend, manifest: &Manifest) -> Result<()> {
    let body = serde_json::to_vec_pretty(manifest)?;
    backend
        .put_bytes(&manifest.key(), body, "application/json")
        .await
}

pub async fn load_all(backend: &dyn StorageBackend) -> Result<Vec<Manifest>> {
    let keys = backend.list(PREFIX).await?;
    let mut manifests = Vec::new();
    for key in keys {
        if !key.ends_with(".json") {
            continue;
        }
        if let Some(bytes) = backend.get(&key).await? {
            match serde_json::from_slice::<Manifest>(&bytes) {
                Ok(m) => manifests.push(m),
                Err(e) => tracing::warn!("skipping unparsable manifest {key}: {e}"),
            }
        }
    }
    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_uses_commit_and_sanitized_host() {
        let m = Manifest::new("abc123".into(), 100, "fischl".into(), vec![]);
        assert_eq!(m.key(), "manifests/abc123__fischl.json");
    }

    #[test]
    fn roundtrips_through_json() {
        let m = Manifest::new(
            "deadbeef".into(),
            1733865600,
            "focalors".into(),
            vec!["aaaa".into(), "bbbb".into()],
        );
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
    }
}
