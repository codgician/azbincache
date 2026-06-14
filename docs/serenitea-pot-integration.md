# Integrating azbincache into serenitea-pot CI

This is a copy-paste guide to replace **Cachix** in
`codgician/serenitea-pot` with a self-hosted Azure-backed cache. azbincache
itself cannot edit that repo; apply these steps there.

## 1. One-time Azure setup

```bash
# Resource group + storage account (cheapest: Standard_LRS, Hot)
az group create -n nixcache-rg -l eastus
az storage account create -n <ACCOUNT> -g nixcache-rg \
  --sku Standard_LRS --kind StorageV2 --access-tier Hot

# Enable the static website endpoint ($web container, anonymous GET)
az storage blob service-properties update --account-name <ACCOUNT> \
  --static-website --index-document index.html

# Read the public read endpoint clients will use:
az storage account show -n <ACCOUNT> --query primaryEndpoints.web -o tsv
# -> https://<ACCOUNT>.z##.web.core.windows.net/

# A container-scoped SAS for CI writes (rwdl on $web), expiring e.g. 1 year out:
az storage container generate-sas --account-name <ACCOUNT> \
  --name '$web' --permissions rwdl --expiry 2027-01-01 --https-only -o tsv
```

Build the cache signing key once and keep the secret safe:

```bash
nix-store --generate-binary-cache-key serenitea-pot-1 ./secret ./public
cat ./public   # -> serenitea-pot-1:<base64>   (goes into nixConfig)
```

## 2. GitHub secrets (in serenitea-pot)

| Secret | Value |
| --- | --- |
| `AZBINCACHE_SAS_URL` | `https://<ACCOUNT>.blob.core.windows.net/$web?<SAS>` |
| `AZBINCACHE_SIGNING_KEY` | contents of `./secret` (`serenitea-pot-1:...`) |

## 3. Workflow edits (`.github/workflows/build.yml`)

In `build-linux` and `build-darwin`, **after** the `Build` step, add:

```yaml
      - name: Push to azbincache
        run: |
          nix run github:codgician/azbincache -- push \
            --to "$AZBINCACHE_SAS_URL" \
            --upstream https://cache.nixos.org=cache.nixos.org-1 \
            --upstream https://nix-community.cachix.org=nix-community.cachix.org-1 \
            --commit "$GITHUB_SHA" \
            --commit-time "$(git show -s --format=%ct HEAD)" \
            --host "${{ matrix.host.name }}" \
            "$(readlink -f result || nix path-info .#nixosConfigurations.${{ matrix.host.name }}.config.system.build.toplevel)"
        env:
          AZBINCACHE_SAS_URL: ${{ secrets.AZBINCACHE_SAS_URL }}
          AZBINCACHE_SIGNING_KEY: ${{ secrets.AZBINCACHE_SIGNING_KEY }}
```

### Alternative: keyless OIDC (no SAS secret)

Microsoft recommends OpenID Connect over long-lived secrets. Instead of a SAS,
federate GitHub's OIDC token to an Entra identity and grant it the
**`Storage Blob Data Contributor`** role on the `$web` container:

```bash
# Assign the data-plane role to your federated app/managed identity:
az role assignment create --assignee <CLIENT_ID> \
  --role "Storage Blob Data Contributor" \
  --scope "$(az storage account show -n <ACCOUNT> --query id -o tsv)/blobServices/default/containers/\$web"
```

Store `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID` as secrets,
then:

```yaml
    permissions:
      id-token: write          # let GitHub mint the OIDC token
      contents: read
    steps:
      # ...build...
      - uses: azure/login@v2
        with:
          client-id: ${{ secrets.AZURE_CLIENT_ID }}
          tenant-id: ${{ secrets.AZURE_TENANT_ID }}
          subscription-id: ${{ secrets.AZURE_SUBSCRIPTION_ID }}
      - name: Push to azbincache
        run: |
          nix run github:codgician/azbincache -- push \
            --to "https://<ACCOUNT>.blob.core.windows.net/\$web" \
            --auth oidc \
            --upstream https://cache.nixos.org=cache.nixos.org-1 \
            --commit "$GITHUB_SHA" --commit-time "$(git show -s --format=%ct HEAD)" \
            --host "${{ matrix.host.name }}" \
            "$(readlink -f result)"
        env:
          AZBINCACHE_SIGNING_KEY: ${{ secrets.AZBINCACHE_SIGNING_KEY }}
```

`azure/login@v2` exports `AZURE_*` / `AZURE_FEDERATED_TOKEN_FILE`, which
`--auth oidc` consumes automatically. A **service principal** with a client
secret works the same way with `--auth service-principal` (set
`AZURE_CLIENT_SECRET`). Note the public read endpoint stays
`https://<ACCOUNT>.z##.web.core.windows.net/`; only the *write* URL differs
between SAS and Entra.

> `cache.nixos.org` is treated as permanent (signature-only skip);
> `nix-community.cachix.org` is treated as ephemeral (a live HEAD is required
> before skipping). During validation you may keep the existing
> `cachix/cachix-action` steps in parallel; remove them once a machine
> substitutes correctly from Azure.

Add a terminal GC job (runs once, after all builds, only on `main`):

```yaml
  cleanup:
    needs: [build-linux, build-darwin]
    if: ${{ always() && github.ref == 'refs/heads/main' }}
    runs-on: ubuntu-latest
    steps:
      - uses: cachix/install-nix-action@v31
        with:
          extra_nix_config: experimental-features = nix-command flakes
      - name: GC old commits
        run: |
          nix run github:codgician/azbincache -- gc \
            --to "$AZBINCACHE_SAS_URL" --keep-commits 3
        env:
          AZBINCACHE_SAS_URL: ${{ secrets.AZBINCACHE_SAS_URL }}
```

## 4. Consumer config (each machine / flake `nixConfig`)

```nix
nix.settings = {
  extra-substituters = [ "https://<ACCOUNT>.z##.web.core.windows.net/" ];
  extra-trusted-public-keys = [ "serenitea-pot-1:<base64-from-./public>" ];
};
```

## 5. Validate

1. Run the workflow on a branch; confirm `azbincache push` uploads and the
   `cleanup` job keeps the last 3 commits.
2. On a machine with the substituter configured, build a host config and verify
   logs show fetches from `*.web.core.windows.net`.
3. Once validated, delete the `cachix/cachix-action` steps and the
   `CACHIX_*` secrets.
