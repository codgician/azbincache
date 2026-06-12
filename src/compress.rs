use anyhow::{bail, Result};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Zstd(i32),
    Xz(u32),
    None,
}

impl Compression {
    pub fn nar_extension(self) -> &'static str {
        match self {
            Compression::Zstd(_) => "nar.zst",
            Compression::Xz(_) => "nar.xz",
            Compression::None => "nar",
        }
    }

    pub fn narinfo_name(self) -> &'static str {
        match self {
            Compression::Zstd(_) => "zstd",
            Compression::Xz(_) => "xz",
            Compression::None => "none",
        }
    }

    pub fn encoder<W: Write>(self, writer: W) -> Result<NarEncoder<W>> {
        Ok(match self {
            Compression::Zstd(level) => {
                NarEncoder::Zstd(zstd::stream::Encoder::new(writer, level)?)
            }
            Compression::Xz(preset) => NarEncoder::Xz(xz2::write::XzEncoder::new(writer, preset)),
            Compression::None => NarEncoder::Plain(writer),
        })
    }
}

pub enum NarEncoder<W: Write> {
    Zstd(zstd::stream::Encoder<'static, W>),
    Xz(xz2::write::XzEncoder<W>),
    Plain(W),
}

impl<W: Write> NarEncoder<W> {
    pub fn finish(self) -> Result<W> {
        Ok(match self {
            NarEncoder::Zstd(e) => e.finish()?,
            NarEncoder::Xz(e) => e.finish()?,
            NarEncoder::Plain(w) => w,
        })
    }
}

impl<W: Write> Write for NarEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            NarEncoder::Zstd(e) => e.write(buf),
            NarEncoder::Xz(e) => e.write(buf),
            NarEncoder::Plain(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            NarEncoder::Zstd(e) => e.flush(),
            NarEncoder::Xz(e) => e.flush(),
            NarEncoder::Plain(w) => w.flush(),
        }
    }
}

const ZSTD_MIN: i32 = 1;
const ZSTD_MAX: i32 = 22;
const XZ_MAX: u32 = 9;
const XZ_SAFE_MAX: u32 = 6;

/// Resolve `(algorithm, optional level)` into a validated `Compression`,
/// rejecting out-of-range levels and (without `allow_high_memory`) xz presets
/// that would exceed the small-runner memory budget.
pub fn resolve(algo: Algo, level: Option<i64>, allow_high_memory: bool) -> Result<Compression> {
    match algo {
        Algo::Zstd => {
            let l = level.unwrap_or(3);
            let level = i32::try_from(l)
                .ok()
                .filter(|v| (ZSTD_MIN..=ZSTD_MAX).contains(v))
                .ok_or_else(|| {
                    anyhow::anyhow!("zstd level must be {ZSTD_MIN}..={ZSTD_MAX}, got {l}")
                })?;
            Ok(Compression::Zstd(level))
        }
        Algo::Xz => {
            let l = level.unwrap_or(6);
            let preset = u32::try_from(l)
                .ok()
                .filter(|v| *v <= XZ_MAX)
                .ok_or_else(|| anyhow::anyhow!("xz preset must be 0..={XZ_MAX}, got {l}"))?;
            if preset > XZ_SAFE_MAX && !allow_high_memory {
                bail!(
                    "xz preset {preset} needs a large encoder dictionary (preset 9 ~ 674 MB) and \
                     may exceed the target memory budget; pass --allow-high-memory to override"
                );
            }
            Ok(Compression::Xz(preset))
        }
        Algo::None => {
            if level.is_some() {
                bail!("--compression-level is not valid with --compression none");
            }
            Ok(Compression::None)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    Zstd,
    Xz,
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn extension_and_narinfo_name_per_algo() {
        assert_eq!(Compression::Zstd(3).nar_extension(), "nar.zst");
        assert_eq!(Compression::Xz(6).nar_extension(), "nar.xz");
        assert_eq!(Compression::None.nar_extension(), "nar");
        assert_eq!(Compression::Zstd(3).narinfo_name(), "zstd");
        assert_eq!(Compression::Xz(6).narinfo_name(), "xz");
        assert_eq!(Compression::None.narinfo_name(), "none");
    }

    #[test]
    fn resolve_defaults() {
        assert_eq!(
            resolve(Algo::Zstd, None, false).unwrap(),
            Compression::Zstd(3)
        );
        assert_eq!(resolve(Algo::Xz, None, false).unwrap(), Compression::Xz(6));
        assert_eq!(resolve(Algo::None, None, false).unwrap(), Compression::None);
    }

    #[test]
    fn resolve_rejects_out_of_range() {
        assert!(resolve(Algo::Zstd, Some(0), false).is_err());
        assert!(resolve(Algo::Zstd, Some(23), false).is_err());
        assert!(resolve(Algo::Xz, Some(-1), false).is_err());
        assert!(resolve(Algo::Xz, Some(10), false).is_err());
    }

    #[test]
    fn resolve_none_rejects_level() {
        assert!(resolve(Algo::None, Some(3), false).is_err());
        assert!(resolve(Algo::None, None, false).is_ok());
    }

    #[test]
    fn resolve_guards_high_xz_preset() {
        assert!(resolve(Algo::Xz, Some(7), false).is_err());
        assert_eq!(
            resolve(Algo::Xz, Some(7), true).unwrap(),
            Compression::Xz(7)
        );
        assert_eq!(
            resolve(Algo::Xz, Some(6), false).unwrap(),
            Compression::Xz(6)
        );
    }

    fn roundtrip(c: Compression) -> Vec<u8> {
        let raw = vec![7u8; 200 * 1024];
        let mut buf = Vec::new();
        let mut enc = c.encoder(&mut buf).unwrap();
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap();
        buf
    }

    #[test]
    fn zstd_roundtrips() {
        let raw = vec![7u8; 200 * 1024];
        let compressed = roundtrip(Compression::Zstd(3));
        let back = zstd::decode_all(compressed.as_slice()).unwrap();
        assert_eq!(back, raw);
    }

    #[test]
    fn xz_roundtrips() {
        let raw = vec![7u8; 200 * 1024];
        let compressed = roundtrip(Compression::Xz(6));
        let mut back = Vec::new();
        xz2::read::XzDecoder::new(compressed.as_slice())
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, raw);
    }

    #[test]
    fn none_is_passthrough() {
        let raw = vec![7u8; 200 * 1024];
        let out = roundtrip(Compression::None);
        assert_eq!(out, raw);
    }
}
