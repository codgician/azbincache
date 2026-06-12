use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn exists(&self, key: &str) -> Result<bool>;

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Upload a small in-memory object (narinfo, manifest, nix-cache-info).
    async fn put_bytes(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<()>;

    /// Upload a file without buffering its whole contents in memory. Backends
    /// stream from disk (HTTP) or stage bounded blocks (Azure).
    async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<()>;

    async fn delete(&self, key: &str) -> Result<()>;

    async fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// Human-readable backend label for diagnostics (e.g. "Azure Blob", "HTTP").
    fn kind(&self) -> &'static str;
}

pub mod azure;
pub mod http;

pub use azure::{AzureAuth, AzureBackend};
pub use http::HttpBackend;

pub fn from_url(url: &str) -> Result<Box<dyn StorageBackend>> {
    from_url_with_auth(url, None)
}

pub fn from_url_with_auth(
    url: &str,
    azure_auth: Option<AzureAuth>,
) -> Result<Box<dyn StorageBackend>> {
    match azure_auth {
        Some(auth) => Ok(Box::new(AzureBackend::new(url, &auth)?)),
        None if is_azure_url(url) => Ok(Box::new(AzureBackend::new(url, &AzureAuth::Sas)?)),
        None => Ok(Box::new(HttpBackend::new(url)?)),
    }
}

fn is_azure_url(url: &str) -> bool {
    url.contains(".blob.core.windows.net")
        || url.contains("127.0.0.1:10000")
        || url.contains("sig=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_azure_urls() {
        assert!(is_azure_url(
            "https://acct.blob.core.windows.net/web?sv=2021&sig=abc"
        ));
        assert!(is_azure_url("https://h/c?sig=xyz"));
        assert!(!is_azure_url("http://127.0.0.1:8080"));
        assert!(!is_azure_url("https://cache.example.com/"));
    }
}
