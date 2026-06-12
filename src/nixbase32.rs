// Bit-packed nixbase32 (Nix's own alphabet/ordering). The `as u8` truncations
// below are intentional: values are masked (`& 0xff`) or provably < 32, so no
// information is lost. These casts are the natural expression of the algorithm.
#![allow(clippy::cast_possible_truncation)]

const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

pub fn encode(bytes: &[u8]) -> String {
    let len = encoded_len(bytes.len());
    let mut out = String::with_capacity(len);

    for n in (0..len).rev() {
        let b = n * 5;
        let i = b / 8;
        let j = b % 8;
        let mut c = u16::from(bytes[i]) >> j;
        if i + 1 < bytes.len() {
            c |= u16::from(bytes[i + 1]) << (8 - j);
        }
        out.push(ALPHABET[(c & 0x1f) as usize] as char);
    }

    out
}

pub fn decode(s: &str) -> Result<Vec<u8>, DecodeError> {
    let input = s.as_bytes();
    let out_len = decoded_len(input.len());
    let mut out = vec![0u8; out_len];

    for (n, &ch) in input.iter().rev().enumerate() {
        let digit = u16::from(value_of(ch).ok_or(DecodeError::InvalidChar(ch as char))?);
        let b = n * 5;
        let i = b / 8;
        let j = b % 8;

        out[i] |= ((digit << j) & 0xff) as u8;
        let carry = (digit >> (8 - j)) as u8;
        if i + 1 < out_len {
            out[i + 1] |= carry;
        } else if carry != 0 {
            return Err(DecodeError::NonZeroPadding);
        }
    }

    Ok(out)
}

pub const fn encoded_len(byte_len: usize) -> usize {
    if byte_len == 0 {
        0
    } else {
        (byte_len * 8 - 1) / 5 + 1
    }
}

pub const fn decoded_len(char_len: usize) -> usize {
    char_len * 5 / 8
}

fn value_of(ch: u8) -> Option<u8> {
    ALPHABET.iter().position(|&c| c == ch).map(|p| p as u8)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("invalid nixbase32 character: {0:?}")]
    InvalidChar(char),
    #[error("non-zero padding bits in nixbase32 input")]
    NonZeroPadding,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn empty_sha256_matches_nix_hash() {
        let digest =
            hex_to_bytes("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let expected = "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73";
        assert_eq!(encode(&digest), expected);
        assert_eq!(decode(expected).unwrap(), digest);
    }

    #[test]
    fn all_zeros_sha256_matches_nix_hash() {
        let digest = [0u8; 32];
        let expected = "0000000000000000000000000000000000000000000000000000";
        assert_eq!(encode(&digest), expected);
        assert_eq!(encode(&digest).len(), 52);
        assert_eq!(encoded_len(32), 52);
        assert_eq!(decode(expected).unwrap(), digest.to_vec());
    }

    #[test]
    fn rejects_excluded_letters() {
        for bad in ['e', 'o', 't', 'u'] {
            assert!(
                matches!(
                    decode(&bad.to_string().repeat(52)),
                    Err(DecodeError::InvalidChar(_))
                ),
                "char {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn decodes_real_store_hash() {
        let hash = "zi2bj2hlavv8q743li2s9diqbcpmrf9b";
        let decoded = decode(hash).unwrap();
        assert_eq!(decoded.len(), 20);
        assert_eq!(encode(&decoded), hash);
    }
}
