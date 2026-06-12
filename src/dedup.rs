use crate::narinfo;
use crate::storage::StorageBackend;
use anyhow::Result;
use reqwest::{Client, StatusCode};

#[derive(Debug, Clone)]
pub struct Upstream {
    pub url: String,
    pub key_name: Option<String>,
    pub permanent: bool,
}

impl Upstream {
    pub fn narinfo_url(&self, store_hash: &str) -> String {
        format!("{}/{}.narinfo", self.url.trim_end_matches('/'), store_hash)
    }
}

pub struct Deduper {
    upstreams: Vec<Upstream>,
    client: Client,
    no_upstream_skip: bool,
}

impl Deduper {
    pub fn new(upstreams: Vec<Upstream>, no_upstream_skip: bool) -> Result<Self> {
        Ok(Self {
            upstreams,
            client: Client::builder()
                .user_agent(concat!("azbincache/", env!("CARGO_PKG_VERSION")))
                .build()?,
            no_upstream_skip,
        })
    }

    pub fn signed_by_permanent_upstream(&self, sigs: &[String]) -> bool {
        if self.no_upstream_skip {
            return false;
        }
        self.upstreams.iter().any(|u| {
            u.permanent
                && u.key_name.as_ref().is_some_and(|name| {
                    sigs.iter()
                        .any(|sig| sig.split(':').next() == Some(name.as_str()))
                })
        })
    }

    pub async fn present_in_any_upstream(&self, store_hash: &str) -> bool {
        if self.no_upstream_skip {
            return false;
        }
        for u in &self.upstreams {
            if self.head_ok(&u.narinfo_url(store_hash)).await {
                return true;
            }
        }
        false
    }

    async fn head_ok(&self, url: &str) -> bool {
        matches!(
            self.client.head(url).send().await.map(|r| r.status()),
            Ok(StatusCode::OK)
        )
    }
}

pub async fn should_upload(
    backend: &dyn StorageBackend,
    deduper: &Deduper,
    store_path: &str,
    sigs: &[String],
) -> Result<bool> {
    let store_hash = narinfo::store_path_hash(store_path);

    if backend.exists(&format!("{store_hash}.narinfo")).await? {
        return Ok(false);
    }
    if deduper.signed_by_permanent_upstream(sigs) {
        return Ok(false);
    }
    if deduper.present_in_any_upstream(store_hash).await {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstreams() -> Vec<Upstream> {
        vec![
            Upstream {
                url: "https://cache.nixos.org".into(),
                key_name: Some("cache.nixos.org-1".into()),
                permanent: true,
            },
            Upstream {
                url: "https://nix-community.cachix.org".into(),
                key_name: Some("nix-community.cachix.org-1".into()),
                permanent: false,
            },
        ]
    }

    #[test]
    fn permanent_upstream_signature_triggers_skip() {
        let d = Deduper::new(upstreams(), false).unwrap();
        assert!(d.signed_by_permanent_upstream(&["cache.nixos.org-1:abc==".to_string()]));
    }

    #[test]
    fn ephemeral_upstream_signature_does_not_trigger_offline_skip() {
        let d = Deduper::new(upstreams(), false).unwrap();
        assert!(!d.signed_by_permanent_upstream(&["nix-community.cachix.org-1:abc==".to_string()]));
    }

    #[test]
    fn unknown_signature_does_not_trigger_skip() {
        let d = Deduper::new(upstreams(), false).unwrap();
        assert!(!d.signed_by_permanent_upstream(&["serenitea-pot-1:abc==".to_string()]));
    }

    #[test]
    fn no_upstream_skip_disables_signature_filter() {
        let d = Deduper::new(upstreams(), true).unwrap();
        assert!(!d.signed_by_permanent_upstream(&["cache.nixos.org-1:abc==".to_string()]));
    }

    #[test]
    fn narinfo_url_built_correctly() {
        let u = &upstreams()[0];
        assert_eq!(
            u.narinfo_url("zi2bj2hlavv8q743li2s9diqbcpmrf9b"),
            "https://cache.nixos.org/zi2bj2hlavv8q743li2s9diqbcpmrf9b.narinfo"
        );
    }
}
