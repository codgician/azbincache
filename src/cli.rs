use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "azbincache", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[arg(long, global = true, env = "AZBINCACHE_LOG", default_value = "info")]
    pub log: String,
}

impl Cli {
    pub fn init_tracing(&self) {
        let filter = tracing_subscriber::EnvFilter::try_new(&self.log)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Push(PushArgs),
    Gc(GcArgs),
    Info(InfoArgs),
    Pubkey(PubkeyArgs),
    Doctor(DoctorArgs),
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[arg(long, env = "AZBINCACHE_SAS_URL")]
    pub to: String,

    #[command(flatten)]
    pub auth: AuthArgs,
}

#[derive(Args, Debug)]
pub struct PubkeyArgs {
    #[arg(
        long,
        env = "AZBINCACHE_SIGNING_KEY",
        conflicts_with = "signing_key_file"
    )]
    pub signing_key_env: Option<String>,

    #[arg(long, conflicts_with = "signing_key_env")]
    pub signing_key_file: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct PushArgs {
    #[arg(long, env = "AZBINCACHE_SAS_URL")]
    pub to: String,

    #[command(flatten)]
    pub auth: AuthArgs,

    #[arg(
        long,
        env = "AZBINCACHE_SIGNING_KEY",
        conflicts_with = "signing_key_file"
    )]
    pub signing_key_env: Option<String>,

    #[arg(long, conflicts_with = "signing_key_env")]
    pub signing_key_file: Option<std::path::PathBuf>,

    #[arg(long, value_delimiter = ',')]
    pub upstream: Vec<String>,

    #[arg(long, default_value = "zstd")]
    pub compression: Algo,

    #[arg(long)]
    pub compression_level: Option<i64>,

    #[arg(long)]
    pub allow_high_memory: bool,

    #[arg(long, requires = "commit_time")]
    pub commit: Option<String>,

    #[arg(long, requires = "commit")]
    pub commit_time: Option<i64>,

    #[arg(long)]
    pub host: Option<String>,

    #[arg(long)]
    pub no_closure: bool,

    #[arg(long)]
    pub no_upstream_skip: bool,

    #[arg(required = true)]
    pub paths: Vec<String>,
}

#[derive(Args, Debug)]
pub struct GcArgs {
    #[arg(long, env = "AZBINCACHE_SAS_URL")]
    pub to: String,

    #[command(flatten)]
    pub auth: AuthArgs,

    #[arg(long, default_value_t = 3)]
    pub keep_commits: usize,

    #[arg(long)]
    pub allow_empty: bool,

    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct InfoArgs {
    #[arg(long, env = "AZBINCACHE_SAS_URL")]
    pub to: String,

    #[command(flatten)]
    pub auth: AuthArgs,

    #[arg(long, default_value = "/nix/store")]
    pub store_dir: String,

    #[arg(long, default_value_t = 41)]
    pub priority: u32,
}

#[derive(Args, Debug)]
pub struct AuthArgs {
    #[arg(long, value_enum, default_value = "auto", env = "AZBINCACHE_AUTH")]
    pub auth: AuthMode,

    #[arg(long, env = "AZURE_TENANT_ID")]
    pub azure_tenant_id: Option<String>,

    #[arg(long, env = "AZURE_CLIENT_ID")]
    pub azure_client_id: Option<String>,

    #[arg(long, env = "AZURE_CLIENT_SECRET")]
    pub azure_client_secret: Option<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// Pick SAS when the URL carries `sig=`, else anonymous HTTP.
    Auto,
    /// SAS token embedded in the `--to` URL.
    Sas,
    /// GitHub Actions OIDC / workload identity federation.
    Oidc,
    /// Entra service principal (needs azure-tenant-id/client-id/client-secret).
    ServicePrincipal,
}

impl AuthArgs {
    pub fn resolve(&self) -> anyhow::Result<Option<crate::storage::AzureAuth>> {
        use crate::storage::AzureAuth;
        match self.auth {
            AuthMode::Auto => Ok(None),
            AuthMode::Sas => Ok(Some(AzureAuth::Sas)),
            AuthMode::Oidc => Ok(Some(AzureAuth::Oidc)),
            AuthMode::ServicePrincipal => {
                let tenant_id = self.azure_tenant_id.clone().ok_or_else(|| {
                    anyhow::anyhow!("service-principal auth requires --azure-tenant-id")
                })?;
                let client_id = self.azure_client_id.clone().ok_or_else(|| {
                    anyhow::anyhow!("service-principal auth requires --azure-client-id")
                })?;
                let client_secret = self.azure_client_secret.clone().ok_or_else(|| {
                    anyhow::anyhow!("service-principal auth requires --azure-client-secret")
                })?;
                Ok(Some(AzureAuth::ServicePrincipal {
                    tenant_id,
                    client_id,
                    client_secret,
                }))
            }
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Zstd,
    Xz,
    None,
}

impl From<Algo> for crate::compress::Algo {
    fn from(a: Algo) -> Self {
        match a {
            Algo::Zstd => crate::compress::Algo::Zstd,
            Algo::Xz => crate::compress::Algo::Xz,
            Algo::None => crate::compress::Algo::None,
        }
    }
}

fn load_signing_key(
    env_val: Option<&String>,
    file: Option<&std::path::PathBuf>,
) -> anyhow::Result<crate::sign::NixSigningKey> {
    let raw = match (env_val, file) {
        (Some(v), _) => v.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)?,
        (None, None) => {
            anyhow::bail!("a signing key is required (--signing-key-env or --signing-key-file)")
        }
    };
    Ok(crate::sign::NixSigningKey::parse(raw.trim())?)
}

pub mod push {
    use super::{load_signing_key, PushArgs};
    use crate::compress;
    use crate::dedup::{should_upload, Deduper, Upstream};
    use crate::nix;
    use crate::push::{push_path, PushOptions};
    use crate::storage;
    use anyhow::Result;

    pub async fn run(args: PushArgs) -> Result<()> {
        let compression = compress::resolve(
            args.compression.into(),
            args.compression_level,
            args.allow_high_memory,
        )?;
        let key = load_signing_key(
            args.signing_key_env.as_ref(),
            args.signing_key_file.as_ref(),
        )?;
        let backend = storage::from_url_with_auth(&args.to, args.auth.resolve()?)?;
        let opts = PushOptions {
            compression,
            ..PushOptions::default()
        };
        let upstreams = parse_upstreams(&args.upstream)?;
        let deduper = Deduper::new(upstreams, args.no_upstream_skip)?;

        crate::cache_info::ensure(backend.as_ref(), &opts.store_dir, opts.priority).await?;

        let to_consider = if args.no_closure {
            args.paths.clone()
        } else {
            nix::closure(&args.paths).await?
        };

        let mut uploaded = 0usize;
        let mut skipped = 0usize;
        let mut kept_in_cache: Vec<String> = Vec::new();
        for path in &to_consider {
            let meta = nix::query_meta(path).await?;
            if !should_upload(backend.as_ref(), &deduper, path, &meta.sigs).await? {
                skipped += 1;
                if backend
                    .exists(&format!(
                        "{}.narinfo",
                        crate::narinfo::store_path_hash(path)
                    ))
                    .await?
                {
                    kept_in_cache.push(crate::narinfo::store_path_hash(path).to_string());
                }
                tracing::debug!("skipped {path}");
                continue;
            }
            if push_path(backend.as_ref(), &key, path, &opts).await? {
                uploaded += 1;
                kept_in_cache.push(crate::narinfo::store_path_hash(path).to_string());
                tracing::info!("uploaded {path}");
            } else {
                skipped += 1;
                kept_in_cache.push(crate::narinfo::store_path_hash(path).to_string());
            }
        }

        if let Some(commit) = &args.commit {
            let m = crate::manifest::Manifest::new(
                commit.clone(),
                args.commit_time.unwrap_or(0),
                args.host.clone().unwrap_or_else(|| "default".to_string()),
                kept_in_cache,
            );
            crate::manifest::write(backend.as_ref(), &m).await?;
            tracing::info!("wrote manifest {}", m.key());
        }

        tracing::info!(
            "push complete: {uploaded} uploaded, {skipped} skipped (of {})",
            to_consider.len()
        );
        Ok(())
    }

    fn parse_upstreams(specs: &[String]) -> Result<Vec<Upstream>> {
        let permanent_hosts = ["cache.nixos.org"];
        specs
            .iter()
            .map(|spec| {
                let (url, key_name) = match spec.split_once('=') {
                    Some((u, k)) if !k.is_empty() => (u.to_string(), Some(k.to_string())),
                    Some((_, _)) => {
                        anyhow::bail!("upstream spec '{spec}' has an empty key after '='")
                    }
                    None => (spec.clone(), None),
                };
                let permanent = permanent_hosts.iter().any(|h| url.contains(h));
                Ok(Upstream {
                    url,
                    key_name,
                    permanent,
                })
            })
            .collect()
    }
}

pub mod gc {
    use super::GcArgs;
    use crate::storage;
    use anyhow::Result;

    pub async fn run(args: GcArgs) -> Result<()> {
        let backend = storage::from_url_with_auth(&args.to, args.auth.resolve()?)?;
        let plan = crate::gc::plan(backend.as_ref(), args.keep_commits).await?;

        if plan.manifest_count == 0 && !args.allow_empty {
            anyhow::bail!(
                "refusing to gc: no commit manifests found. A cache pushed without --commit \
                 has no retention data, so gc would delete everything. Pass --allow-empty to \
                 proceed anyway."
            );
        }

        tracing::info!(
            "gc plan: keep {} commits, {} live paths; delete {} narinfo + {} nar + {} manifests",
            plan.kept_commits.len(),
            plan.live_hashes.len(),
            plan.narinfos_to_delete.len(),
            plan.nars_to_delete.len(),
            plan.manifests_to_delete.len(),
        );

        if args.dry_run {
            for k in plan
                .narinfos_to_delete
                .iter()
                .chain(&plan.nars_to_delete)
                .chain(&plan.manifests_to_delete)
            {
                println!("would delete {k}");
            }
            return Ok(());
        }

        crate::gc::execute(backend.as_ref(), &plan).await?;
        Ok(())
    }
}

pub mod info {
    use super::InfoArgs;
    use crate::storage;
    use anyhow::Result;

    pub async fn run(args: InfoArgs) -> Result<()> {
        let backend = storage::from_url_with_auth(&args.to, args.auth.resolve()?)?;
        crate::cache_info::ensure(backend.as_ref(), &args.store_dir, args.priority).await?;
        tracing::info!("nix-cache-info ensured at {}", args.to);
        Ok(())
    }
}

pub mod pubkey {
    use super::{load_signing_key, PubkeyArgs};
    use anyhow::Result;

    pub fn run(args: PubkeyArgs) -> Result<()> {
        let PubkeyArgs {
            signing_key_env,
            signing_key_file,
        } = args;
        let key = load_signing_key(signing_key_env.as_ref(), signing_key_file.as_ref())?;
        tracing::debug!("derived public key for {}", key.name());
        println!("{}", key.public_key_field());
        Ok(())
    }
}

pub mod doctor {
    use super::DoctorArgs;
    use crate::storage;
    use anyhow::{Context, Result};

    pub async fn run(args: DoctorArgs) -> Result<()> {
        let auth = args.auth.resolve()?;
        let backend = storage::from_url_with_auth(&args.to, auth)
            .context("constructing storage backend (check --to URL and --auth)")?;

        match crate::doctor::run(backend.as_ref(), &args.to).await {
            Ok(report) => {
                print!("{report}");
                Ok(())
            }
            Err(e) => Err(e.context(hint(&args))),
        }
    }

    fn hint(args: &DoctorArgs) -> String {
        let mode = format!("{:?}", args.auth.auth);
        format!(
            "could not reach the cache. Checklist: (1) is --to correct and the container reachable? \
             (2) auth mode is {mode}: for SAS the URL needs a valid '?...sig=' with rwl permissions; \
             for OIDC/service-principal the identity needs the 'Storage Blob Data Contributor' role \
             on the container (a missing role returns 403); (3) network/DNS to the endpoint"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::AzureAuth;

    fn auth_args(mode: AuthMode, sp: Option<(&str, &str, &str)>) -> AuthArgs {
        AuthArgs {
            auth: mode,
            azure_tenant_id: sp.map(|(t, _, _)| t.to_string()),
            azure_client_id: sp.map(|(_, c, _)| c.to_string()),
            azure_client_secret: sp.map(|(_, _, s)| s.to_string()),
        }
    }

    #[test]
    fn auto_resolves_to_none() {
        assert!(auth_args(AuthMode::Auto, None).resolve().unwrap().is_none());
    }

    #[test]
    fn sas_and_oidc_map_directly() {
        assert!(matches!(
            auth_args(AuthMode::Sas, None).resolve().unwrap(),
            Some(AzureAuth::Sas)
        ));
        assert!(matches!(
            auth_args(AuthMode::Oidc, None).resolve().unwrap(),
            Some(AzureAuth::Oidc)
        ));
    }

    #[test]
    fn service_principal_requires_all_three_fields() {
        assert!(auth_args(AuthMode::ServicePrincipal, None)
            .resolve()
            .is_err());
        let ok = auth_args(AuthMode::ServicePrincipal, Some(("t", "c", "s")))
            .resolve()
            .unwrap();
        assert!(matches!(
            ok,
            Some(AzureAuth::ServicePrincipal { tenant_id, client_id, client_secret })
                if tenant_id == "t" && client_id == "c" && client_secret == "s"
        ));
    }
}
