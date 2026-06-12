use data_encoding::BASE64;
use ed25519_dalek::{Signer, SigningKey, SECRET_KEY_LENGTH};

pub struct NixSigningKey {
    name: String,
    key: SigningKey,
}

impl NixSigningKey {
    pub fn parse(s: &str) -> Result<Self, KeyError> {
        let (name, payload) = split_named(s)?;
        let bytes = BASE64.decode(payload.as_bytes())?;
        if bytes.len() != SECRET_KEY_LENGTH + 32 {
            return Err(KeyError::WrongLength {
                expected: SECRET_KEY_LENGTH + 32,
                actual: bytes.len(),
            });
        }
        let seed = bytes
            .first_chunk::<SECRET_KEY_LENGTH>()
            .ok_or(KeyError::WrongLength {
                expected: SECRET_KEY_LENGTH + 32,
                actual: bytes.len(),
            })?;
        Ok(Self {
            name: name.to_string(),
            key: SigningKey::from_bytes(seed),
        })
    }

    pub fn sign_to_field(&self, message: &[u8]) -> String {
        let sig = self.key.sign(message);
        format!("{}:{}", self.name, BASE64.encode(&sig.to_bytes()))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn public_key_field(&self) -> String {
        format!(
            "{}:{}",
            self.name,
            BASE64.encode(self.key.verifying_key().as_bytes())
        )
    }
}

fn split_named(s: &str) -> Result<(&str, &str), KeyError> {
    let (name, payload) = s.trim().split_once(':').ok_or(KeyError::NoColon)?;
    if name.is_empty() {
        return Err(KeyError::EmptyName);
    }
    if payload.is_empty() {
        return Err(KeyError::EmptyPayload);
    }
    Ok((name, payload))
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("key string lacks a ':' separator")]
    NoColon,
    #[error("key name is empty")]
    EmptyName,
    #[error("key payload is empty")]
    EmptyPayload,
    #[error("base64 decode failed: {0}")]
    Base64(#[from] data_encoding::DecodeError),
    #[error("wrong secret key length: expected {expected}, got {actual}")]
    WrongLength { expected: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-key-1:ZvXN0BONPeNT/IFpW8mjDnetnzGkOiegm8DdXfPcylpqpxSGhIDyjBr8uAup/1fpSMWepPBIvQAqJ/xRCMMLjA==";
    const PUBLIC: &str = "test-key-1:aqcUhoSA8owa/LgLqf9X6UjFnqTwSL0AKif8UQjDC4w=";

    #[test]
    fn parses_real_nix_key_and_derives_pubkey() {
        let key = NixSigningKey::parse(SECRET).unwrap();
        assert_eq!(key.name(), "test-key-1");
        assert_eq!(key.public_key_field(), PUBLIC);
    }

    #[test]
    fn sign_field_has_name_prefix_and_valid_base64() {
        let key = NixSigningKey::parse(SECRET).unwrap();
        let field = key.sign_to_field(b"hello");
        let (name, payload) = field.split_once(':').unwrap();
        assert_eq!(name, "test-key-1");
        assert_eq!(BASE64.decode(payload.as_bytes()).unwrap().len(), 64);
    }

    #[test]
    fn golden_signature_matches_nix_store_sign() {
        let key = NixSigningKey::parse(SECRET).unwrap();
        let fingerprint = crate::fingerprint::fingerprint(
            "/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3",
            "sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw",
            279624,
            &[
                "/nix/store/57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61".to_string(),
                "/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3".to_string(),
            ],
        );
        let expected = "test-key-1:8AI5OJZqB6PopYKv7ICletATgq43D6KoMl0VFK15HCCQ4k/P4Ikg9S0k2AUdS+4XLdh9cm2n6nYnlbTrCiTOAA==";
        assert_eq!(key.sign_to_field(fingerprint.as_bytes()), expected);
    }

    #[test]
    fn golden_signature_with_unsorted_refs_matches_nix() {
        let key = NixSigningKey::parse(SECRET).unwrap();
        let fingerprint = crate::fingerprint::fingerprint(
            "/nix/store/6qa00czc79b3nb6ld0mdyacfp2p1k3jx-libidn2-2.3.8",
            "sha256:03qm43d81sfk4cpjj1laqjg4l0kcda6wrknpb517qfh59hq7q4ql",
            368176,
            &[
                "/nix/store/bf6wgamqnl3c91iamlb1branrfcwwy7x-libunistring-1.4.2".to_string(),
                "/nix/store/6qa00czc79b3nb6ld0mdyacfp2p1k3jx-libidn2-2.3.8".to_string(),
            ],
        );
        let expected = "test-key-1:+qZTGACkUguUAsymXaVMAkjVABcqacor0dmKy1ytsUnPCFunXFEC5VileuXEAKS6rurdCXJHOZQxzHpJwpjaAw==";
        assert_eq!(key.sign_to_field(fingerprint.as_bytes()), expected);
    }

    #[test]
    fn rejects_malformed_keys() {
        assert!(matches!(
            NixSigningKey::parse("nocolon"),
            Err(KeyError::NoColon)
        ));
        assert!(matches!(
            NixSigningKey::parse(":payload"),
            Err(KeyError::EmptyName)
        ));
        assert!(NixSigningKey::parse("name:not_base64!!").is_err());
    }
}
