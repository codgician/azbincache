pub fn fingerprint(
    store_path: &str,
    nar_hash: &str,
    nar_size: u64,
    references: &[String],
) -> String {
    let mut sorted: Vec<&String> = references.iter().collect();
    sorted.sort();
    let refs = sorted
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!("1;{store_path};{nar_hash};{nar_size};{refs}")
}

pub fn references_to_absolute(store_dir: &str, short_refs: &[String]) -> Vec<String> {
    short_refs
        .iter()
        .map(|r| {
            if r.starts_with('/') {
                r.clone()
            } else {
                format!("{store_dir}/{r}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_references_yields_trailing_semicolon() {
        let fp = fingerprint("/nix/store/aaaa-foo", "sha256:1xxxx", 1234, &[]);
        assert_eq!(fp, "1;/nix/store/aaaa-foo;sha256:1xxxx;1234;");
        assert!(fp.ends_with(';'));
    }

    #[test]
    fn references_are_absolute_and_comma_separated() {
        let refs = references_to_absolute(
            "/nix/store",
            &[
                "57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61".to_string(),
                "zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3".to_string(),
            ],
        );
        let fp = fingerprint(
            "/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3",
            "sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw",
            279624,
            &refs,
        );
        assert_eq!(
            fp,
            "1;/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3;\
sha256:0vwm6sr61cx6hydqlx3phhg1a0830k61dbfwqlkhilkpcbppjmdw;279624;\
/nix/store/57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61,\
/nix/store/zi2bj2hlavv8q743li2s9diqbcpmrf9b-hello-2.12.3"
        );
    }
    #[test]
    fn already_absolute_refs_are_preserved() {
        let refs = references_to_absolute("/nix/store", &["/nix/store/aaaa-bar".to_string()]);
        assert_eq!(refs, vec!["/nix/store/aaaa-bar".to_string()]);
    }

    #[test]
    fn references_are_sorted_lexicographically() {
        let fp = fingerprint(
            "/nix/store/6qa0-libidn2",
            "sha256:xxx",
            10,
            &[
                "/nix/store/bf6w-libunistring".to_string(),
                "/nix/store/6qa0-libidn2".to_string(),
            ],
        );
        assert_eq!(
            fp,
            "1;/nix/store/6qa0-libidn2;sha256:xxx;10;\
/nix/store/6qa0-libidn2,/nix/store/bf6w-libunistring"
        );
    }
}
