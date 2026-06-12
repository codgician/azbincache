use crate::manifest::{self, Manifest};
use crate::storage::StorageBackend;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct CacheStatus {
    pub backend_kind: String,
    pub endpoint: String,
    pub cache_info: Option<String>,
    pub narinfo_count: usize,
    pub nar_count: usize,
    pub commits: Vec<CommitSummary>,
}

pub struct CommitSummary {
    pub commit: String,
    pub commit_time: i64,
    pub hosts: Vec<String>,
    pub path_count: usize,
}

/// Strip a SAS signature (and any query string) from a URL so it is safe to
/// print in diagnostics. A SAS `sig=` is a bearer credential.
pub fn redact_url(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<redacted>"),
        None => url.to_string(),
    }
}

/// Render a Unix timestamp as an approximate age relative to `now_secs`
/// (e.g. "3d ago", "5m ago"). Dependency-free; for human diagnostics only.
pub fn humanize_age(commit_time: i64, now_secs: i64) -> String {
    let delta = now_secs - commit_time;
    if delta < 0 {
        return "in the future".to_string();
    }
    let (value, unit) = match delta {
        s if s < 60 => (s, "s"),
        s if s < 3600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3600, "h"),
        s => (s / 86_400, "d"),
    };
    format!("{value}{unit} ago")
}

pub fn summarize_commits(manifests: &[Manifest]) -> Vec<CommitSummary> {
    use std::collections::BTreeMap;
    let mut by_commit: BTreeMap<&str, (i64, BTreeSet<String>, usize)> = BTreeMap::new();
    for m in manifests {
        let entry = by_commit
            .entry(&m.commit)
            .or_insert((m.commit_time, BTreeSet::new(), 0));
        entry.0 = entry.0.max(m.commit_time);
        entry.1.insert(m.host.clone());
        entry.2 += m.closure.len();
    }
    let mut commits: Vec<CommitSummary> = by_commit
        .into_iter()
        .map(|(commit, (commit_time, hosts, path_count))| CommitSummary {
            commit: commit.to_string(),
            commit_time,
            hosts: hosts.into_iter().collect(),
            path_count,
        })
        .collect();
    commits.sort_by(|a, b| {
        b.commit_time
            .cmp(&a.commit_time)
            .then(a.commit.cmp(&b.commit))
    });
    commits
}

pub fn render_report(status: &CacheStatus, now_secs: i64) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "azbincache doctor");
    let _ = writeln!(s, "  backend:   {}", status.backend_kind);
    let _ = writeln!(s, "  endpoint:  {}", status.endpoint);
    let _ = writeln!(s, "  connection: OK");

    match &status.cache_info {
        Some(body) => {
            let _ = writeln!(s, "  nix-cache-info: present");
            for line in body.lines() {
                let _ = writeln!(s, "    {line}");
            }
        }
        None => {
            let _ = writeln!(
                s,
                "  nix-cache-info: MISSING (run `azbincache info --to ...`)"
            );
        }
    }

    let _ = writeln!(
        s,
        "  objects:   {} narinfo, {} nar",
        status.narinfo_count, status.nar_count
    );

    if status.commits.is_empty() {
        let _ = writeln!(
            s,
            "  commits:   none (pushes ran without --commit; gc has no retention data)"
        );
    } else {
        let _ = writeln!(s, "  commits:   {}", status.commits.len());
        for c in &status.commits {
            let hosts = c.hosts.join(", ");
            let _ = writeln!(
                s,
                "    {} [{}]  {} paths, hosts: {}",
                short_commit(&c.commit),
                humanize_age(c.commit_time, now_secs),
                c.path_count,
                hosts
            );
        }
    }
    s
}

fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

pub async fn gather(backend: &dyn StorageBackend, endpoint: &str) -> Result<CacheStatus> {
    let cache_info = backend
        .get("nix-cache-info")
        .await
        .context("connecting to storage (GET nix-cache-info)")?
        .map(|b| String::from_utf8_lossy(&b).trim_end().to_string());

    let root = backend.list("").await.context("listing cache objects")?;
    let narinfo_count = root.iter().filter(|k| k.ends_with(".narinfo")).count();
    let nar_count = backend
        .list("nar/")
        .await
        .context("listing nar/ objects")?
        .iter()
        .filter(|k| k.starts_with("nar/") && !k.ends_with('/'))
        .count();

    let manifests = manifest::load_all(backend)
        .await
        .context("loading commit manifests")?;
    let commits = summarize_commits(&manifests);

    Ok(CacheStatus {
        backend_kind: backend.kind().to_string(),
        endpoint: redact_url(endpoint),
        cache_info,
        narinfo_count,
        nar_count,
        commits,
    })
}

pub async fn run(backend: &dyn StorageBackend, endpoint: &str) -> Result<String> {
    let status = gather(backend, endpoint).await?;
    Ok(render_report(&status, now_unix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(commit: &str, time: i64, host: &str, paths: usize) -> Manifest {
        Manifest::new(
            commit.into(),
            time,
            host.into(),
            (0..paths).map(|i| format!("p{i}")).collect(),
        )
    }

    #[test]
    fn redacts_sas_signature() {
        assert_eq!(
            redact_url("https://acct.blob.core.windows.net/web?sv=2021&sig=secret"),
            "https://acct.blob.core.windows.net/web?<redacted>"
        );
        assert_eq!(
            redact_url("https://acct.blob.core.windows.net/web"),
            "https://acct.blob.core.windows.net/web"
        );
    }

    #[test]
    fn humanizes_age_units() {
        assert_eq!(humanize_age(1000, 1030), "30s ago");
        assert_eq!(humanize_age(1000, 1000 + 120), "2m ago");
        assert_eq!(humanize_age(1000, 1000 + 7200), "2h ago");
        assert_eq!(humanize_age(1000, 1000 + 3 * 86_400), "3d ago");
        assert_eq!(humanize_age(2000, 1000), "in the future");
    }

    #[test]
    fn summarize_merges_hosts_and_orders_by_recency() {
        let ms = vec![
            m("old", 100, "fischl", 2),
            m("new", 200, "linux", 3),
            m("new", 200, "darwin", 1),
        ];
        let out = summarize_commits(&ms);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].commit, "new");
        assert_eq!(out[0].hosts, vec!["darwin", "linux"]);
        assert_eq!(out[0].path_count, 4);
        assert_eq!(out[1].commit, "old");
    }

    #[test]
    fn report_flags_missing_cache_info_and_no_commits() {
        let status = CacheStatus {
            backend_kind: "HTTP (WebDAV)".into(),
            endpoint: "http://h/c".into(),
            cache_info: None,
            narinfo_count: 0,
            nar_count: 0,
            commits: vec![],
        };
        let out = render_report(&status, 0);
        assert!(out.contains("nix-cache-info: MISSING"));
        assert!(out.contains("commits:   none"));
    }

    #[test]
    fn report_lists_commits_with_age() {
        let status = CacheStatus {
            backend_kind: "Azure Blob Storage".into(),
            endpoint: "https://a/web?<redacted>".into(),
            cache_info: Some("StoreDir: /nix/store".into()),
            narinfo_count: 5,
            nar_count: 4,
            commits: summarize_commits(&[m("abcdef0123456789", 100, "linux", 5)]),
        };
        let out = render_report(&status, 100 + 86_400);
        assert!(out.contains("abcdef012345"));
        assert!(out.contains("1d ago"));
        assert!(out.contains("5 narinfo, 4 nar"));
    }
}
