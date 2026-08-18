//! Documentation suite: bilingual guide and README heading parity.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// Returns the sequence of Markdown heading levels (`1` for `#`, `2` for `##`, …).
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

/// Returns `##` section titles in document order (without the `## ` prefix).
fn h2_titles(markdown: &str) -> Vec<&str> {
    markdown
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .collect()
}

fn assert_pair_matching_headings(en_path: &PathBuf, pt_path: &PathBuf, label: &str) {
    assert!(
        en_path.is_file(),
        "missing English {label}: {}",
        en_path.display()
    );
    assert!(
        pt_path.is_file(),
        "missing Portuguese {label}: {}",
        pt_path.display()
    );
    let en = fs::read_to_string(en_path).expect("read en");
    let pt = fs::read_to_string(pt_path).expect("read pt");
    let en_levels = heading_levels(&en);
    let pt_levels = heading_levels(&pt);
    assert!(
        !en_levels.is_empty(),
        "{label} must contain at least one heading"
    );
    assert_eq!(
        en_levels, pt_levels,
        "heading hierarchy mismatch for {label}"
    );
}

#[test]
fn docs_guides_and_pt_translations_exist_and_have_matching_headings() {
    let docs = repo_root().join("docs");
    const PAIRS: &[(&str, &str)] = &[
        ("architecture.md", "architecture-PT.md"),
        ("c-ffi-guide.md", "c-ffi-guide-PT.md"),
        ("language-reference.md", "language-reference-PT.md"),
    ];
    for (en, pt) in PAIRS {
        assert_pair_matching_headings(&docs.join(en), &docs.join(pt), en);
    }
}

#[test]
fn readme_and_readme_pt_have_identical_heading_hierarchy() {
    let root = repo_root();
    assert_pair_matching_headings(
        &root.join("README.md"),
        &root.join("README-PT.md"),
        "README",
    );
}

#[test]
fn root_readme_and_readme_pt_exist_and_match_readme_skeleton_section_order() {
    let root = repo_root();
    let en_path = root.join("README.md");
    let pt_path = root.join("README-PT.md");
    let en = fs::read_to_string(&en_path).expect("README.md");
    let pt = fs::read_to_string(&pt_path).expect("README-PT.md");

    assert!(en.starts_with("# Elara\n"), "README.md title");
    assert!(pt.starts_with("# Elara\n"), "README-PT.md title");

    let en_h2 = h2_titles(&en);
    let pt_h2 = h2_titles(&pt);
    assert_eq!(
        en_h2,
        [
            "Highlights",
            "Overview",
            "Prerequisites",
            "Installation",
            "Quick Start",
            "Usage",
            "Project Layout",
            "Development",
            "License",
        ]
    );
    assert_eq!(
        pt_h2,
        [
            "Destaques",
            "Visão Geral",
            "Pré-requisitos",
            "Instalação",
            "Início Rápido",
            "Uso",
            "Estrutura do Projeto",
            "Desenvolvimento",
            "Licença",
        ]
    );

    for text in [&en, &pt] {
        assert!(
            text.contains("./demos/run_demos.sh"),
            "Usage must include ./demos/run_demos.sh"
        );
        for target in [
            "make build",
            "make test",
            "make quality",
            "make install",
            "make install-system",
            "make uninstall",
            "make uninstall-system",
        ] {
            assert!(text.contains(target), "Development must document {target}");
        }
        for dir in ["elara/", "demos/", "docs/", "bin/", ".make/"] {
            assert!(text.contains(dir), "Project Layout must document {dir}");
        }
        // Highlights: plain bullets, no trailing periods on highlight lines.
        let after_hl = text
            .split_once("## Highlights")
            .or_else(|| text.split_once("## Destaques"))
            .map(|(_, rest)| rest)
            .expect("Highlights section");
        let highlights_body = after_hl.split("\n## ").next().expect("highlights body");
        let bullets: Vec<&str> = highlights_body
            .lines()
            .filter(|l| l.starts_with("- "))
            .collect();
        assert!(
            (8..=10).contains(&bullets.len()),
            "Highlights must have 8–10 bullets, found {}",
            bullets.len()
        );
        for b in &bullets {
            assert!(
                !b.trim_end().ends_with('.'),
                "Highlight bullets must not end with a period: {b}"
            );
            assert!(
                !b.contains("**"),
                "Highlight bullets must not use bold-label prefixes: {b}"
            );
        }
    }
}
