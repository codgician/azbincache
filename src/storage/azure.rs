use super::StorageBackend;
use anyhow::{Context, Result};
use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_core::http::RequestContent;
use azure_identity::{ClientSecretCredential, WorkloadIdentityCredential};
use azure_storage_blob::models::{
    BlobClientUploadOptions, BlobContainerClientListBlobsOptions,
    BlockBlobClientCommitBlockListOptions, BlockLookupList,
};
use azure_storage_blob::{BlobClient, BlobContainerClient};
use futures::stream::StreamExt;
use std::io::Read as _;
use std::sync::Arc;
use url::Url;

const BLOCK_SIZE: usize = 8 * 1024 * 1024;

/// How the Azure backend proves its identity to the storage account.
#[derive(Debug, Clone)]
pub enum AzureAuth {
    /// Credential lives in the URL query string (`?...&sig=...`). No RBAC.
    Sas,
    /// GitHub Actions OIDC / Kubernetes workload identity federation. Reads
    /// `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_FEDERATED_TOKEN_FILE` from
    /// the environment (set by `azure/login@v2`). Requires the
    /// `Storage Blob Data Contributor` role on the container/account.
    Oidc,
    /// Entra service principal (client id + secret).
    ServicePrincipal {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
}

pub struct AzureBackend {
    container: BlobContainerClient,
    container_url: Url,
    credential: Option<Arc<dyn TokenCredential>>,
}

impl AzureBackend {
    pub fn from_sas_url(sas_url: &str) -> Result<Self> {
        Self::new(sas_url, &AzureAuth::Sas)
    }

    pub fn new(url: &str, auth: &AzureAuth) -> Result<Self> {
        let container_url = Url::parse(url).context("parsing container URL")?;
        let credential = build_credential(auth)?;
        let container = BlobContainerClient::new(container_url.clone(), credential.clone(), None)
            .context("constructing BlobContainerClient")?;
        Ok(Self {
            container,
            container_url,
            credential,
        })
    }

    fn blob_url(&self, key: &str) -> Result<Url> {
        let trimmed = key.trim_start_matches('/');
        let mut url = self.container_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| anyhow::anyhow!("container URL cannot be a base"))?;
            for part in trimmed.split('/') {
                segments.push(part);
            }
        }
        Ok(url)
    }

    fn blob_client(&self, key: &str) -> Result<BlobClient> {
        BlobClient::new(self.blob_url(key)?, self.credential.clone(), None)
            .with_context(|| format!("constructing BlobClient for {key}"))
    }
}

fn build_credential(auth: &AzureAuth) -> Result<Option<Arc<dyn TokenCredential>>> {
    match auth {
        AzureAuth::Sas => Ok(None),
        AzureAuth::Oidc => {
            let cred = WorkloadIdentityCredential::new(None)
                .context("constructing workload identity (OIDC) credential")?;
            Ok(Some(cred))
        }
        AzureAuth::ServicePrincipal {
            tenant_id,
            client_id,
            client_secret,
        } => {
            let cred = ClientSecretCredential::new(
                tenant_id,
                client_id.clone(),
                client_secret.clone().into(),
                None,
            )
            .context("constructing service principal credential")?;
            Ok(Some(cred))
        }
    }
}

#[async_trait]
impl StorageBackend for AzureBackend {
    async fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.blob_client(key)?.exists().await?)
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self.blob_client(key)?.download(None).await {
            Ok(resp) => Ok(Some(resp.body.collect().await?.to_vec())),
            Err(e) if e.http_status() == Some(azure_core::http::StatusCode::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn put_bytes(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<()> {
        let client = self.blob_client(key)?;
        let options = BlobClientUploadOptions {
            blob_content_type: Some(content_type.to_string()),
            ..Default::default()
        };
        let content = RequestContent::from(body);
        client
            .upload(content, Some(options))
            .await
            .with_context(|| format!("uploading {key}"))?;
        Ok(())
    }

    async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<()> {
        let block = self.blob_client(key)?.block_blob_client();
        let mut file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut block_ids: Vec<Vec<u8>> = Vec::new();
        let mut buf = vec![0u8; BLOCK_SIZE];
        let mut index: u64 = 0;
        loop {
            let mut filled = 0;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            let block_id = format!("{index:08}").into_bytes();
            let chunk = buf[..filled].to_vec();
            block
                .stage_block(&block_id, filled as u64, RequestContent::from(chunk), None)
                .await
                .with_context(|| format!("staging block {index} of {key}"))?;
            block_ids.push(block_id);
            index += 1;
        }

        let lookup = BlockLookupList {
            latest: Some(block_ids),
            ..Default::default()
        };
        let commit_options = BlockBlobClientCommitBlockListOptions {
            blob_content_type: Some(content_type.to_string()),
            ..Default::default()
        };
        block
            .commit_block_list(lookup.try_into()?, Some(commit_options))
            .await
            .with_context(|| format!("committing block list for {key}"))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        match self.blob_client(key)?.delete(None).await {
            Ok(_) => Ok(()),
            Err(e) if e.http_status() == Some(azure_core::http::StatusCode::NotFound) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let options = BlobContainerClientListBlobsOptions {
            prefix: Some(prefix.to_string()),
            ..Default::default()
        };
        let mut pages = self.container.list_blobs(Some(options))?.into_pages();
        let mut names = Vec::new();
        while let Some(page) = pages.next().await {
            let body = page?.into_model()?;
            for item in body.blob_items {
                if let Some(name) = item.name {
                    names.push(name);
                }
            }
        }
        Ok(names)
    }

    fn kind(&self) -> &'static str {
        "Azure Blob Storage"
    }
}
