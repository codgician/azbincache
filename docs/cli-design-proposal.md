# azbincache CLI design proposal

Status: **IMPLEMENTED.** All five decisions below were accepted with the
recommended defaults and implemented: single validated `--compression-level`;
`--commit-time` required with `--commit`; `gc` refuses empty manifests unless
`--allow-empty`; combined `--upstream URL=KEYNAME`; xz presets > 6 rejected
unless `--allow-high-memory`. zstd/xz/none backends all stream to the temp file.
This doc is retained as the design record.

---

## 1. Compression backends

### Decision (from review)
- Implement **all three**: `zstd` (default), `xz`, `none`.
- **Per-algorithm validated level**; reject out-of-range with a clear error.
- **xz must stream** to the temp file with bounded memory (same as zstd), to
  hold the 1 vCPU / 512 MB budget.

### Why this matters
`--compression` is currently accepted but `push::run` hard-errors on anything
but `zstd`. Three places in `push_path` hardcode the algorithm:
- NAR key extension `.nar.zst`
- narinfo `Compression: zstd`
- the `dump_and_compress(path, level)` call (level only, no algorithm)

Clients (real `nix`) pick the decompressor from the narinfo `Compression:`
field and the file extension, so mixed-compression caches are valid as long as
each narinfo's `URL:`/`Compression:` agree with the uploaded blob. GC reads the
`URL:` field, so it keeps working unchanged.

### Proposed model

A single enum carrying its algorithm + validated level, produced from the CLI:

```
Compression::Zstd(level)   // level 1..=22,  default 3
Compression::Xz(preset)    // preset 0..=9,  default 6
Compression::None          // no level
```

- File extension + narinfo field derived from the algorithm:
  - zstd → `nar/<filehash>.nar.zst`, `Compression: zstd`
  - xz   → `nar/<filehash>.nar.xz`,  `Compression: xz`
  - none → `nar/<filehash>.nar`,     `Compression: none`
- `nix.rs` streaming pipeline gains an algorithm parameter. Each algorithm wraps
  the temp `File` in a streaming `Write` encoder:
  - zstd: `zstd::stream::Encoder::new(file, level)` (existing)
  - xz:   `xz2::write::XzEncoder::new(file, preset)` — `preset: u32`, then
    `write_all` / `finish() -> W`; identical shape to zstd's encoder.
  - none: write chunks straight to the file (still hashed on the fly)
  - `FileHash`/`FileSize` computed over the resulting temp file exactly as today.
  - Roundtrip test decompressors: `zstd::decode_all`, `xz2::read::XzDecoder`.

### CLI surface

```
--compression <zstd|xz|none>     default: zstd
--compression-level <n>          default + valid range depend on --compression
```

Validation: after parsing, map `(compression, compression-level)` to the enum
and **reject** out-of-range levels:
- zstd: `1..=22`
- xz:   `0..=9`
- none: `--compression-level` must NOT be set (error if explicitly provided)

Open question for you: should `--compression-level` stay a single flag whose
valid range shifts with `--compression` (simpler surface, range documented), or
become algorithm-specific? Recommendation: **single flag, validated against the
chosen algorithm** — fewer flags, clear error messages.

### Memory note (512 MB budget)
- zstd level 3: single-digit MB encoder state. Safe.
- xz encoder memory is dominated by dictionary size and grows steeply with
  preset:
  - preset 6 (xz default, dict 8 MiB): **~94 MB** encoder memory — safe.
  - preset 9 (dict 64 MiB): **~674 MB** — **exceeds the 512 MB budget.**
  - presets 0-2: ~3-17 MB.
  Decision: **default xz preset = 6**; validate/reject (or warn) on presets that
  risk the budget. The valid range stays `0..=9` but anything above ~6 is
  documented as memory-hungry and unsuitable for the small-runner target.

  NOTE: the ~94 MB / ~674 MB figures are from direct knowledge of liblzma; the
  librarian lookup meant to confirm them expired before retrieval. Worth a
  one-line empirical check at implementation time, but the design (default 6,
  cap high presets) holds regardless of the exact numbers.
- none: no compressor state.

### Build input
- `xz2` links liblzma via `lzma-sys`. On Nix add `pkgs.xz` (provides liblzma)
  to the package `buildInputs` and the devShell. (`lzma-sys` can vendor+build
  liblzma, but using the Nix-provided lib is cleaner.) The flake build fails
  fast if the lib is missing, so this is verified at build time.

---

## 2. CLI correctness fixes (from full-sweep review)

Severity-ordered. Each is a place the interface currently lets a user do
something silently wrong.

### 2a. `--commit-time` defaulting to 0 (latent bug)
`push --commit X` without `--commit-time` records epoch 0 in the manifest. GC
ranks commits by `commit_time`, so a 0 timestamp makes that commit sort as
*oldest* → collected first regardless of real recency.
**Proposed:** make `--commit-time` **required when `--commit` is set** (clap
`requires`), OR default to "now". Recommendation: required — explicit beats a
silent wrong default in CI.

### 2b. `gc` with zero manifests = mass deletion (dangerous)
If a cache was populated by pushes without `--commit`, there are no manifests,
the keep-set is empty, and `gc` plans to delete everything.
**Proposed:** `gc` **refuses with a clear error when it finds zero manifests**
unless `--allow-empty` (or similar) is passed. Protects against wiping a cache
that simply wasn't using commit tracking.

### 2c. `--upstream` / `--upstream-key` positional index pairing (fragile)
Keys are paired to upstreams by list index. Mismatched counts/order silently
misalign, and an extra key is dropped.
**Proposed (options):**
- (A) Combined value: `--upstream <URL>=<KEYNAME>` (one flag, can't misalign).
- (B) Keep two flags but **error if counts differ** and warn on drop.
Recommendation: (A) for new clarity, but (B) is the smaller change. Your call.

### 2d. `--compression xz|none` erroring (fixed by section 1)
Resolved once backends land.

### 2e. `--compression-level: i32` unvalidated (fixed by section 1)
Resolved by per-algorithm range validation.

---

## 3. Conventions (informational — current design is defensible)

- **Target as `--to` flag**: matches `nix copy --to <store-uri>`, which is the
  closest analog (URL-addressed, not a named registry cache like cachix/attic).
  Recommendation: **keep `--to`**.
- **Paths positional**: matches cachix/attic/nix copy. ✓
- **Env naming** `AZBINCACHE_SAS_URL` / `_SIGNING_KEY` / `_LOG`: follows the
  conventional `<TOOL>_<THING>` shape (cf. `CACHIX_AUTH_TOKEN`). ✓
- **Count-based retention** `gc --keep-commits N`: matches the original
  requirement ("keep most recent N commits"). attic uses duration-based
  (`--retention-period 7d`); noting only as a possible future addition.

---

## 4. Decisions needed before implementation

1. `--compression-level`: single flag with algorithm-dependent range (rec) vs
   per-algorithm flags?
2. `--commit-time`: required-with-`--commit` (rec) vs default-to-now?
3. `gc` zero-manifest guard: refuse-unless-`--allow-empty` (rec) — agree?
4. Upstream pairing: combined `URL=KEY` (A) vs validated two-flag (B)?
5. Scope: implement after sign-off, or proposal-only for now?
