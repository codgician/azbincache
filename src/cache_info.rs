use crate::storage::StorageBackend;
use anyhow::Result;

pub fn render(store_dir: &str, priority: u32) -> String {
    format!("StoreDir: {store_dir}\nWantMassQuery: 1\nPriority: {priority}\n")
}

pub async fn ensure(backend: &dyn StorageBackend, store_dir: &str, priority: u32) -> Result<()> {
    if !backend.exists("nix-cache-info").await? {
        backend
            .put_bytes(
                "nix-cache-info",
                render(store_dir, priority).into_bytes(),
                "text/x-nix-cache-info",
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_expected_fields() {
        assert_eq!(
            render("/nix/store", 41),
            "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 41\n"
        );
    }
}
