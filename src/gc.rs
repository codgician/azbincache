use crate::manifest::{self, Manifest};
use crate::storage::StorageBackend;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub struct GcPlan {
    pub manifest_count: usize,
    pub kept_commits: Vec<String>,
    pub live_hashes: HashSet<String>,
    pub narinfos_to_delete: Vec<String>,
    pub nars_to_delete: Vec<String>,
    pub manifests_to_delete: Vec<String>,
}

pub fn plan_keep_set(manifests: &[Manifest], keep: usize) -> (Vec<String>, HashSet<String>) {
    let mut by_commit: HashMap<&str, i64> = HashMap::new();
    for m in manifests {
        let e = by_commit.entry(&m.commit).or_insert(m.commit_time);
        *e = (*e).max(m.commit_time);
    }

    let mut commits: Vec<(&str, i64)> = by_commit.into_iter().collect();
    commits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(a.0)));

    let kept: HashSet<&str> = commits.iter().take(keep).map(|(c, _)| *c).collect();

    let mut live = HashSet::new();
    for m in manifests {
        if kept.contains(m.commit.as_str()) {
            for h in &m.closure {
                live.insert(h.clone());
            }
        }
    }

    let kept_list: Vec<String> = commits
        .iter()
        .take(keep)
        .map(|(c, _)| c.to_string())
        .collect();
    (kept_list, live)
}

pub async fn plan(backend: &dyn StorageBackend, keep: usize) -> Result<GcPlan> {
    let manifests = manifest::load_all(backend).await?;
    let (kept_commits, live_hashes) = plan_keep_set(&manifests, keep);

    let mut live_nars = HashSet::new();
    for hash in &live_hashes {
        let key = format!("{hash}.narinfo");
        if let Some(bytes) = backend.get(&key).await? {
            if let Some(url) = parse_url_field(&String::from_utf8_lossy(&bytes)) {
                live_nars.insert(url);
            }
        }
    }

    let mut narinfos_to_delete = Vec::new();
    for key in backend.list("").await? {
        if let Some(hash) = key.strip_suffix(".narinfo") {
            if !live_hashes.contains(hash) {
                narinfos_to_delete.push(key.clone());
            }
        }
    }

    let mut nars_to_delete = Vec::new();
    for key in backend.list("nar/").await? {
        if key.starts_with("nar/") && !live_nars.contains(&key) {
            nars_to_delete.push(key);
        }
    }

    let kept_set: HashSet<&str> = kept_commits.iter().map(String::as_str).collect();
    let mut manifests_to_delete = Vec::new();
    for m in &manifests {
        if !kept_set.contains(m.commit.as_str()) {
            manifests_to_delete.push(m.key());
        }
    }

    Ok(GcPlan {
        manifest_count: manifests.len(),
        kept_commits,
        live_hashes,
        narinfos_to_delete,
        nars_to_delete,
        manifests_to_delete,
    })
}

pub async fn execute(backend: &dyn StorageBackend, plan: &GcPlan) -> Result<()> {
    for key in plan
        .narinfos_to_delete
        .iter()
        .chain(&plan.nars_to_delete)
        .chain(&plan.manifests_to_delete)
    {
        backend.delete(key).await?;
        tracing::info!("deleted {key}");
    }
    Ok(())
}

fn parse_url_field(narinfo_text: &str) -> Option<String> {
    narinfo_text
        .lines()
        .find_map(|l| l.strip_prefix("URL: "))
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(commit: &str, time: i64, closure: &[&str]) -> Manifest {
        Manifest::new(
            commit.into(),
            time,
            "h".into(),
            closure.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    #[test]
    fn keeps_newest_n_commits_by_time() {
        let ms = vec![
            m("c1", 100, &["a"]),
            m("c2", 200, &["b"]),
            m("c3", 300, &["c"]),
            m("c4", 400, &["d"]),
        ];
        let (kept, live) = plan_keep_set(&ms, 2);
        assert_eq!(kept, vec!["c4".to_string(), "c3".to_string()]);
        assert_eq!(live, HashSet::from(["c".into(), "d".into()]));
    }

    #[test]
    fn shared_deps_of_kept_commit_survive() {
        let ms = vec![
            m("old", 100, &["shared", "old_only"]),
            m("new", 200, &["shared", "new_only"]),
        ];
        let (_, live) = plan_keep_set(&ms, 1);
        assert!(live.contains("shared"));
        assert!(live.contains("new_only"));
        assert!(!live.contains("old_only"));
    }

    #[test]
    fn multiple_hosts_same_commit_merge_closures() {
        let ms = vec![
            m("c1", 100, &["linux_path"]),
            Manifest::new(
                "c1".into(),
                100,
                "darwin".into(),
                vec!["darwin_path".into()],
            ),
        ];
        let (kept, live) = plan_keep_set(&ms, 1);
        assert_eq!(kept, vec!["c1".to_string()]);
        assert!(live.contains("linux_path"));
        assert!(live.contains("darwin_path"));
    }

    #[test]
    fn keep_zero_makes_everything_dead() {
        let ms = vec![m("c1", 100, &["a"])];
        let (kept, live) = plan_keep_set(&ms, 0);
        assert!(kept.is_empty());
        assert!(live.is_empty());
    }

    #[test]
    fn parses_url_field() {
        let text = "StorePath: /nix/store/x\nURL: nar/abc.nar.zst\nCompression: zstd\n";
        assert_eq!(parse_url_field(text), Some("nar/abc.nar.zst".to_string()));
    }
}
