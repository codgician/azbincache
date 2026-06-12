#![allow(clippy::unwrap_used, clippy::expect_used)]

use azbincache::storage::{AzureBackend, StorageBackend};
use std::process::{Child, Command};
use std::time::Duration;

const ACCOUNT: &str = "devstoreaccount1";
const KEY_B64: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

struct Azurite {
    child: Child,
    dir: tempfile::TempDir,
}

impl Drop for Azurite {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = &self.dir;
    }
}

fn start_azurite(port: u16) -> Option<Azurite> {
    let dir = tempfile::tempdir().ok()?;
    let child = Command::new("azurite-blob")
        .args([
            "--silent",
            "--skipApiVersionCheck",
            "--location",
            dir.path().to_str().unwrap(),
            "--blobPort",
            &port.to_string(),
        ])
        .spawn()
        .ok()?;
    std::thread::sleep(Duration::from_secs(4));
    Some(Azurite { child, dir })
}

fn account_sas(port: u16) -> String {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let key = base64::engine::general_purpose::STANDARD
        .decode(KEY_B64)
        .unwrap();

    let signed_permissions = "rwdlac";
    let signed_services = "b";
    let signed_resource_types = "sco";
    let signed_start = "";
    let signed_expiry = "2030-01-01T00:00:00Z";
    let signed_ip = "";
    let signed_protocol = "http,https";
    let signed_version = "2021-08-06";

    let string_to_sign = format!(
        "{ACCOUNT}\n{signed_permissions}\n{signed_services}\n{signed_resource_types}\n{signed_start}\n{signed_expiry}\n{signed_ip}\n{signed_protocol}\n{signed_version}\n\n"
    );

    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(string_to_sign.as_bytes());
    let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();

    let _ = port;
    format!(
        "sv={}&ss={}&srt={}&sp={}&se={}&spr={}&sig={}",
        signed_version,
        signed_services,
        signed_resource_types,
        signed_permissions,
        enc(signed_expiry),
        enc(signed_protocol),
        enc(&sig),
    )
}

async fn create_container(port: u16, sas: &str, container: &str) -> bool {
    let url = format!("http://127.0.0.1:{port}/{ACCOUNT}/{container}?restype=container&{sas}");
    reqwest::Client::new()
        .put(&url)
        .header("content-length", "0")
        .header("x-ms-version", "2021-08-06")
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

#[tokio::test]
async fn azure_backend_put_get_list_delete_against_azurite() {
    let port = 10010;
    let Some(_azurite) = start_azurite(port) else {
        eprintln!("azurite not available; skipping");
        return;
    };

    let sas = account_sas(port);
    let container = "nixcache";
    if !create_container(port, &sas, container).await {
        eprintln!("could not create container (auth/SAS issue); skipping");
        return;
    }

    let container_url = format!("http://127.0.0.1:{port}/{ACCOUNT}/{container}?{sas}");
    let backend = AzureBackend::from_sas_url(&container_url).expect("backend");

    backend
        .put_bytes(
            "nix-cache-info",
            b"StoreDir: /nix/store\n".to_vec(),
            "text/plain",
        )
        .await
        .expect("put");

    assert!(backend.exists("nix-cache-info").await.expect("exists"));
    assert!(!backend.exists("missing.narinfo").await.expect("exists"));

    let got = backend.get("nix-cache-info").await.expect("get");
    assert_eq!(got.as_deref(), Some(&b"StoreDir: /nix/store\n"[..]));

    let nar = tempfile::NamedTempFile::new().expect("temp nar");
    std::fs::write(nar.path(), [1u8, 2, 3]).expect("write nar");
    backend
        .put_file("nar/abc.nar.zst", nar.path(), "application/x-nix-nar")
        .await
        .expect("put nar");
    let listed = backend.list("nar/").await.expect("list");
    assert!(listed.contains(&"nar/abc.nar.zst".to_string()));

    let status = azbincache::doctor::gather(&backend, &container_url)
        .await
        .expect("doctor gather");
    assert_eq!(status.backend_kind, "Azure Blob Storage");
    assert_eq!(status.nar_count, 1);
    assert!(status.cache_info.is_some());
    assert!(
        !status.endpoint.contains("sig="),
        "endpoint must be redacted"
    );

    backend.delete("nar/abc.nar.zst").await.expect("delete");
    assert!(!backend.exists("nar/abc.nar.zst").await.expect("exists"));
}
