use super::StorageBackend;
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{Client, Method, StatusCode};

pub struct HttpBackend {
    base: String,
    client: Client,
}

impl HttpBackend {
    pub fn new(base_url: &str) -> Result<Self> {
        let base = base_url.trim_end_matches('/').to_string();
        let client = Client::builder()
            .user_agent(concat!("azbincache/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { base, client })
    }

    fn url(&self, key: &str) -> String {
        format!("{}/{}", self.base, key.trim_start_matches('/'))
    }
}

#[async_trait]
impl StorageBackend for HttpBackend {
    async fn exists(&self, key: &str) -> Result<bool> {
        let resp = self
            .client
            .head(self.url(key))
            .send()
            .await
            .with_context(|| format!("HEAD {key}"))?;
        match resp.status() {
            StatusCode::OK | StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            other => anyhow::bail!("unexpected status {other} for HEAD {key}"),
        }
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .client
            .get(self.url(key))
            .send()
            .await
            .with_context(|| format!("GET {key}"))?;
        match resp.status() {
            StatusCode::OK => Ok(Some(resp.bytes().await?.to_vec())),
            StatusCode::NOT_FOUND => Ok(None),
            other => anyhow::bail!("unexpected status {other} for GET {key}"),
        }
    }

    async fn put_bytes(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<()> {
        let resp = self
            .client
            .put(self.url(key))
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            .with_context(|| format!("PUT {key}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("unexpected status {} for PUT {key}", resp.status())
        }
    }

    async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<()> {
        let file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("opening {} for upload", path.display()))?;
        let len = file.metadata().await?.len();
        let stream = tokio_util::io::ReaderStream::new(file);
        let resp = self
            .client
            .put(self.url(key))
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .header(reqwest::header::CONTENT_LENGTH, len)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .with_context(|| format!("PUT (file) {key}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("unexpected status {} for PUT {key}", resp.status())
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let resp = self
            .client
            .delete(self.url(key))
            .send()
            .await
            .with_context(|| format!("DELETE {key}"))?;
        match resp.status() {
            s if s.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            other => anyhow::bail!("unexpected status {other} for DELETE {key}"),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let mut dirs = vec![prefix.trim_end_matches('/').to_string()];

        while let Some(dir) = dirs.pop() {
            let url = if dir.is_empty() {
                format!("{}/", self.base)
            } else {
                format!("{}/{}/", self.base, dir.trim_start_matches('/'))
            };
            let (entry_files, entry_dirs) = self.propfind_depth1(&url, &dir).await?;
            files.extend(entry_files);
            dirs.extend(entry_dirs);
        }

        Ok(files)
    }

    fn kind(&self) -> &'static str {
        "HTTP (WebDAV)"
    }
}

impl HttpBackend {
    async fn propfind_depth1(&self, url: &str, dir: &str) -> Result<(Vec<String>, Vec<String>)> {
        let method = Method::from_bytes(b"PROPFIND")?;
        let resp = self
            .client
            .request(method, url)
            .header("Depth", "1")
            .send()
            .await
            .with_context(|| format!("PROPFIND {url}"))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok((vec![], vec![]));
        }
        let body = resp.text().await?;
        let self_key = dir.trim_matches('/');
        Ok(parse_propfind_entries(&body, &self.base, self_key))
    }
}

fn base_path(base: &str) -> &str {
    base.find("://")
        .and_then(|i| base[i + 3..].find('/').map(|j| &base[i + 3 + j..]))
        .unwrap_or("")
        .trim_end_matches('/')
}

fn parse_propfind_entries(xml: &str, base: &str, self_key: &str) -> (Vec<String>, Vec<String>) {
    let bp = base_path(base);
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<D:href>").or_else(|| rest.find("<d:href>")) {
        let after = &rest[start + 8..];
        let Some(end) = after.find("</") else { break };
        let href = after[..end].trim();
        let is_dir = href.ends_with('/');
        let key = href
            .strip_prefix(bp)
            .unwrap_or(href)
            .trim_matches('/')
            .to_string();
        rest = &after[end..];

        if key == self_key {
            continue;
        }
        if is_dir {
            dirs.push(key);
        } else {
            files.push(key);
        }
    }
    (files, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_files_and_subdirs_excluding_self() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/cache/</D:href></D:response>
  <D:response><D:href>/cache/nix-cache-info</D:href></D:response>
  <D:response><D:href>/cache/nar/</D:href></D:response>
  <D:response><D:href>/cache/manifests/</D:href></D:response>
</D:multistatus>"#;
        let (files, dirs) = parse_propfind_entries(xml, "http://server/cache", "");
        assert_eq!(files, vec!["nix-cache-info"]);
        assert_eq!(dirs, vec!["nar", "manifests"]);
    }

    #[test]
    fn lists_files_inside_a_subdir() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/cache/manifests/</D:href></D:response>
  <D:response><D:href>/cache/manifests/commitA__h.json</D:href></D:response>
</D:multistatus>"#;
        let (files, dirs) = parse_propfind_entries(xml, "http://server/cache", "manifests");
        assert_eq!(files, vec!["manifests/commitA__h.json"]);
        assert!(dirs.is_empty());
    }
}
