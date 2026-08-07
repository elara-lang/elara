//! Documentation suite: bilingual guide heading parity.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// Returns the sequence of AT heading levels (`1` for `#`, `2` for `##`, …).
fn heading_levels(markdown: &str) -> Vec<usize> {
    markdown
        .lines()
        .filter_map(|line| {
            let bytes = line.as_bytes();
            if bytes.first() != Some(&b'#') {
                return None;
            }
            let mut n = 0usize;
            for b in bytes {
                if *b == b'#' {
                    n += 1;
                } else {
                    break;
                }
            }
            if (1..=6).contains(&n) && bytes.get(n) == Some(&b' ') {
                Some(n)
            } else {
                None
            }
        })
        .collect()
}

fn assert_pair_matching_headings(en_name: &str, pt_name: &str) {
    let docs = repo_root().join("docs");
    let en_path = docs.join(en_name);
    let pt_path = docs.join(pt_name);
    assert!(
        en_path.is_file(),
        "missing English guide {}",
        en_path.display()
    );
    assert!(
        pt_path.is_file(),
        "missing Portuguese guide {}",
        pt_path.display()
    );
    let en = fs::read_to_string(&en_path).expect("read en");
    let pt = fs::read_to_string(&pt_path).expect("read pt");
    let en_levels = heading_levels(&en);
    let pt_levels = heading_levels(&pt);
    assert!(
        !en_levels.is_empty(),
        "{en_name} must contain at least one heading"
    );
    assert_eq!(
        en_levels, pt_levels,
        "heading hierarchy mismatch between {en_name} and {pt_name}"
    );
}

#[test]
fn docs_guides_and_pt_translations_exist_and_have_matching_headings() {
    // architecture + c-ffi-guide (language-reference is added by a later docs task).
    const PAIRS: &[(&str, &str)] = &[
        ("architecture.md", "architecture-PT.md"),
        ("c-ffi-guide.md", "c-ffi-guide-PT.md"),
    ];
    for (en, pt) in PAIRS {
        assert_pair_matching_headings(en, pt);
    }
}
