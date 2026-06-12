use crate::nixbase32;
use data_encoding::BASE64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarInfo {
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub file_hash: String,
    pub file_size: u64,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub ca: Option<String>,
    pub sigs: Vec<String>,
}

impl NarInfo {
    pub fn store_hash(&self) -> &str {
        store_path_hash(&self.store_path)
    }

    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "StorePath: {}", self.store_path);
        let _ = writeln!(s, "URL: {}", self.url);
        let _ = writeln!(s, "Compression: {}", self.compression);
        let _ = writeln!(s, "FileHash: {}", self.file_hash);
        let _ = writeln!(s, "FileSize: {}", self.file_size);
        let _ = writeln!(s, "NarHash: {}", self.nar_hash);
        let _ = writeln!(s, "NarSize: {}", self.nar_size);
        let _ = writeln!(s, "References: {}", self.references.join(" "));
        if let Some(deriver) = &self.deriver {
            let _ = writeln!(s, "Deriver: {deriver}");
        }
        if let Some(ca) = &self.ca {
            let _ = writeln!(s, "CA: {ca}");
        }
        for sig in &self.sigs {
            let _ = writeln!(s, "Sig: {sig}");
        }
        s
    }
}

pub fn store_path_hash(store_path: &str) -> &str {
    let base = store_path.rsplit('/').next().unwrap_or(store_path);
    base.split('-').next().unwrap_or(base)
}

pub fn short_reference(store_path: &str) -> &str {
    store_path.rsplit('/').next().unwrap_or(store_path)
}

pub fn sri_to_nix_hash(sri: &str) -> Result<String, HashError> {
    let (algo, b64) = sri.split_once('-').ok_or(HashError::Malformed)?;
    if algo != "sha256" {
        return Err(HashError::UnsupportedAlgo(algo.to_string()));
    }
    let raw = BASE64.decode(b64.as_bytes())?;
    if raw.len() != 32 {
        return Err(HashError::WrongDigestLength(raw.len()));
    }
    Ok(format!("sha256:{}", nixbase32::encode(&raw)))
}

pub fn normalize_nar_hash(hash: &str) -> Result<String, HashError> {
    if let Some(rest) = hash.strip_prefix("sha256-") {
        sri_to_nix_hash(&format!("sha256-{rest}"))
    } else if hash.starts_with("sha256:") {
        Ok(hash.to_string())
    } else {
        Err(HashError::Malformed)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HashError {
    #[error("malformed hash string")]
    Malformed,
    #[error("unsupported hash algorithm: {0}")]
    UnsupportedAlgo(String),
    #[error("base64 decode error: {0}")]
    Base64(String),
    #[error("digest has wrong length: {0} bytes")]
    WrongDigestLength(usize),
}

impl From<data_encoding::DecodeError> for HashError {
    fn from(e: data_encoding::DecodeError) -> Self {
        HashError::Base64(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3".into(),
            url: "nar/1zzwzcsbpsghsbfjdw416dgmfankjs4chksx1cic1p1z8v6vr0s8.nar.xz".into(),
            compression: "xz".into(),
            file_hash: "sha256:1zzwzcsbpsghsbfjdw416dgmfankjs4chksx1cic1p1z8v6vr0s8".into(),
            file_size: 58480,
            nar_hash: "sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw".into(),
            nar_size: 279624,
            references: vec![
                "57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61".into(),
                "zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3".into(),
            ],
            deriver: Some("67mdzby3g0maqqp93xj03rc99nnrpdp9-hello-2.12.3.drv".into()),
            ca: None,
            sigs: vec![
                "cache.nixos.org-1:DwOHEUMyxq4aUrwzcZJrPPqqlImdA7042VJ+HWOnyjZeBMaKkSQxFS2vArnR7Okej9R8tRzEP9dgTEglZQN8Aw==".into(),
            ],
        }
    }

    #[test]
    fn renders_byte_identical_to_nix_copy_output() {
        let expected = "\
StorePath: /nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3
URL: nar/1zzwzcsbpsghsbfjdw416dgmfankjs4chksx1cic1p1z8v6vr0s8.nar.xz
Compression: xz
FileHash: sha256:1zzwzcsbpsghsbfjdw416dgmfankjs4chksx1cic1p1z8v6vr0s8
FileSize: 58480
NarHash: sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw
NarSize: 279624
References: 57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61 zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3
Deriver: 67mdzby3g0maqqp93xj03rc99nnrpdp9-hello-2.12.3.drv
Sig: cache.nixos.org-1:DwOHEUMyxq4aUrwzcZJrPPqqlImdA7042VJ+HWOnyjZeBMaKkSQxFS2vArnR7Okej9R8tRzEP9dgTEglZQN8Aw==
";
        assert_eq!(hello().render(), expected);
    }

    #[test]
    fn store_hash_extracted() {
        assert_eq!(hello().store_hash(), "zi2bj2hlavv8q743li2s9diqbcpmrf9b");
    }

    #[test]
    fn sri_converts_to_nix_base32_nar_hash() {
        assert_eq!(
            sri_to_nix_hash("sha256-vFV572J30ggnxdytFswEAwEVHoR3dIqbh6azYLI2lW8=").unwrap(),
            "sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw"
        );
    }

    #[test]
    fn normalize_accepts_both_forms() {
        let nix_form = "sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw";
        assert_eq!(normalize_nar_hash(nix_form).unwrap(), nix_form);
        assert_eq!(
            normalize_nar_hash("sha256-vFV572J30ggnxdytFswEAwEVHoR3dIqbh6azYLI2lW8=").unwrap(),
            nix_form
        );
    }

    #[test]
    fn no_references_renders_empty_field() {
        let mut ni = hello();
        ni.references = vec![];
        ni.deriver = None;
        ni.sigs = vec![];
        assert!(ni.render().contains("References: \n"));
    }
}
