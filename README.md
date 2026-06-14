# azbincache

Publish a [Nix binary cache](https://nix.dev/manual/nix/latest/store/types/http-binary-cache-store)
to **Azure Blob Storage** (or any plain HTTP file host) from CI, with:

- **Static hosting, no compute.** The cache is just files served over anonymous
  HTTPS `GET`. On Azure, use the Static Website (`$web`) feature.
- **Upstream-aware dedup.** Store paths already available in a configurable list of
  upstream caches (e.g. `cache.nixos.org`) are not re-uploaded, keeping the cache small.
- **Commit-windowed garbage collection.** Retain only the closures of the most recent
  *N* commits; older, unreferenced paths are swept.

## How it works

A Nix binary cache is three kinds of files at the host root:

```
nix-cache-info                       StoreDir / WantMassQuery / Priority
<storehash>.narinfo                  per-path metadata (signed)
nar/<filehash>.nar.zst               compressed NAR payload
```

`azbincache push` resolves the closure of the given store paths, skips anything
available upstream, compresses each remaining NAR, signs the narinfo with your
ed25519 key (byte-compatible with `nix store sign`), and uploads. It records a
per-commit manifest so `azbincache gc` can keep the most recent *N* commits.

NARs are streamed `nix store dump-path → compress → temp file → upload` so peak
memory stays bounded (a chunk buffer plus the encoder window) regardless of path
size. The Azure backend uploads via staged blocks; the HTTP backend streams the
file body. This is designed to run on small CI runners (e.g. 1 vCPU / 512 MB),
so pushes are serial. Compression is `zstd` (default), `xz`, or `none`; the
default zstd level is 3 and xz preset defaults to 6 (xz presets above 6 need a
large dictionary and are rejected unless `--allow-high-memory`).

Clients consume it like any substituter:

```
extra-substituters = https://<account>.z##.web.core.windows.net/
extra-trusted-public-keys = your-cache-1:<pubkey>
```

## Usage

```
azbincache push \
  --to <write-URL or SAS>            # AZBINCACHE_SAS_URL
  --signing-key-file <path>          # or --signing-key-env AZBINCACHE_SIGNING_KEY
  --upstream https://cache.nixos.org=cache.nixos.org-1 \   # URL[=public-key-name]
  --commit "$GITHUB_SHA" --commit-time "$(git show -s --format=%ct)" \
  --host "$HOSTNAME" \
  --compression zstd                 # zstd (default) | xz | none
  --compression-level 3              # zstd 1-22 (def 3); xz 0-9 (def 6); omit for none
  ./result

azbincache gc --to <write-URL> --keep-commits 3   # refuses empty manifests unless --allow-empty

azbincache info --to <write-URL>     # write/refresh nix-cache-info

azbincache doctor --to <write-URL>   # diagnose connectivity + report cache status

azbincache pubkey --signing-key-file ./secret   # derive the public key to publish
```

## Authentication

The Azure backend supports three auth modes via `--auth` (env `AZBINCACHE_AUTH`):

- **`sas`** — a Shared Access Signature in the `--to` URL (`?...&sig=...`). No
  role assignment needed; the signature itself grants access.
- **`oidc`** — GitHub Actions OIDC / workload identity federation. No stored
  secret: `azure/login@v2` (with `permissions: id-token: write`) exports
  `AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_FEDERATED_TOKEN_FILE`, which
  azbincache consumes automatically.
- **`service-principal`** — Entra app credentials via `--azure-tenant-id`,
  `--azure-client-id`, `--azure-client-secret` (env `AZURE_TENANT_ID` etc.).

`auto` (default) uses SAS when the URL carries `sig=`, else anonymous HTTP for
non-Azure hosts. **OIDC and service-principal require the
`Storage Blob Data Contributor` RBAC role** on the container or account — an
Entra token without that data-plane role gets `403` even though sign-in
succeeded. With `--auth oidc|service-principal` the `--to` URL is the plain
container URL with no SAS query string.

Each `--upstream` is `URL[=public-key-name]`; the optional key name lets a
trusted upstream signature alone authorize a skip. Upstreams matching
`cache.nixos.org` are treated as **permanent** (a trusted upstream signature is
enough to skip). Other upstreams are **ephemeral**: a live `HEAD` of their
`<hash>.narinfo` is required before skipping, so a garbage-collected upstream
path is never assumed present. `--no-upstream-skip` disables all skipping and
forces a self-contained cache.

## Signing key

```
nix-store --generate-binary-cache-key your-cache-1 ./secret ./public
```

Store the secret in CI (e.g. a GitHub Actions secret consumed via
`--signing-key-env`); publish the public key in `trusted-public-keys`. You can
re-derive the public key from the secret at any time with `azbincache pubkey`.

## Use as a GitHub Action

A composite action wraps `azbincache push` for the common "build then push"
flow. Install Nix, build, then push the result:

```yaml
- uses: cachix/install-nix-action@v31
- run: nix build
- uses: codgician/azbincache@v1
  with:
    to: ${{ secrets.AZBINCACHE_SAS_URL }}
    signing-key: ${{ secrets.AZBINCACHE_SIGNING_KEY }}
    paths: result
    upstream: |
      https://cache.nixos.org=cache.nixos.org-1
```

`to` and `signing-key` are passed to the CLI as environment variables, so
secrets never appear in process arguments or logs. By default the action
records a commit manifest (from `github.sha`) so `azbincache gc` can prune by
commit later.

Key inputs: `to` (required), `signing-key`, `paths` (default `result`),
`auth` (`auto`/`sas`/`oidc`/`service-principal`), `azure-tenant-id` /
`azure-client-id` / `azure-client-secret`, `upstream`, `commit` /
`commit-time` / `host`, `compression` / `compression-level`, `skip-push`,
`extra-args`, and `version` (the azbincache flake ref to run). For keyless
OIDC and service-principal setups see
[`docs/examples/build-and-cache.yml`](docs/examples/build-and-cache.yml).

The action requires Nix on the runner (it shells out to
`nix run github:codgician/azbincache`); add an installer step first.

## Development

```
nix develop          # Rust toolchain (incl. rust-analyzer), nix, azurite, nginx
cargo test           # unit + Azurite integration tests
nix flake check      # clippy (deny warnings), rustfmt, and the NixOS VM end-to-end test
```

With [direnv](https://direnv.net/) the devShell loads automatically on `cd`
(`direnv allow`), so editors and LSP clients pick up the pinned `rust-analyzer`
from the flake toolchain — no host-level Rust install is required.

The NixOS VM test (`tests/nixos/end-to-end.nix`) treats Azure Blob as a plain HTTP
server (nginx + WebDAV), then validates push, signed substitution by real `nix`,
rejection of an untrusted key, and GC retention end-to-end.

## License

MIT
