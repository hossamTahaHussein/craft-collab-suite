//! `cargo xtask brand`: no ArtCraft marks in this modified version.
//!
//! The ArtCraft brand terms require a modified version to drop the ArtCraft name, wordmark and
//! logos. They may stay only where the licences require them: LICENSE*, NOTICE, ATTRIBUTION.md, and
//! credit lines (copyright, "based on", "not endorsed", trademark notices). Developer notes
//! (Markdown under `docs/`), spelling lists and this check itself name the original project
//! factually and are not scanned, but any file *named* after the mark is (logos). Translation
//! catalogs (`i18n/`) are shared data whose unused rows never reach the screen, so they are not
//! scanned; a catalog row shown on screen is caught where the code names its source string.

use std::path::{Path, PathBuf};

const MARK: &str = "artcraft";

/// Directories never scanned (build output, VCS, local tool state, planning notes, catalogs).
const SKIP_DIRS: &[&str] =
    &[".git", "target", "node_modules", "dist", ".superpowers", ".remember", ".claude", ".impeccable", ".playwright-mcp", "corpus"];

/// Files the licences require to name the original authors.
fn allowed_file(name: &str) -> bool {
    name.starts_with("LICENSE") || name == "NOTICE" || name == "ATTRIBUTION.md"
}

/// Paths whose text names the original project factually: this check, its usage text, spelling
/// lists.
fn factual(rel: &Path) -> bool {
    rel == Path::new("xtask/src/brand.rs") || rel == Path::new("xtask/src/main.rs") || rel.components().any(|c| c.as_os_str() == "spelling")
}

/// Markdown developer notes under a `docs/` folder.
fn developer_note(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e == "md") && rel.components().any(|c| c.as_os_str() == "docs")
}

/// A line that credits the original authors, as the licences and brand terms ask.
fn credit_line(line: &str) -> bool {
    ["Copyright", "Based on", "endorsed", "trademark"].iter().any(|w| line.contains(w))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    /// 0 for a file whose name is the mark.
    pub line: usize,
}

pub fn scan(root: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            // The shared translation catalogs at the top (`i18n/`), not an app's own i18n code.
            let skipped = rel.starts_with("docs/superpowers") || rel == Path::new("i18n");
            if !SKIP_DIRS.contains(&name.as_str()) && !skipped {
                walk(root, &path, out);
            }
            continue;
        }
        if !kind.is_file() || allowed_file(&name) {
            continue;
        }
        if name.to_lowercase().contains(MARK) {
            out.push(Finding { path: rel.clone(), line: 0 });
        }
        if factual(&rel) || developer_note(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(MARK) && !credit_line(line) {
                out.push(Finding { path: rel.clone(), line: i + 1 });
            }
        }
    }
}

pub fn run(root: &Path) -> Result<(), String> {
    let found = scan(root);
    if found.is_empty() {
        println!("brand: no ArtCraft marks");
        return Ok(());
    }
    for f in &found {
        println!("{}:{}: ArtCraft mark", f.path.display(), f.line);
    }
    Err(format!("{} ArtCraft mark(s) outside LICENSE, NOTICE, ATTRIBUTION.md and credit lines", found.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-brand-{}-{}", std::process::id(), files.len() * 31 + files[0].0.len()));
        let _ = std::fs::remove_dir_all(&dir);
        for (p, body) in files {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    #[test]
    fn flags_mark_in_ui_string() {
        let dir = tree(&[("src/a.rs", "fn about() -> &'static str {\n    \"Made by ArtCraft\"\n}\n")]);
        assert_eq!(scan(&dir), [Finding { path: "src/a.rs".into(), line: 2 }]);
    }

    #[test]
    fn allows_notice_and_attribution() {
        let dir = tree(&[("NOTICE", "Copyright ArtCraft Team\n"), ("ATTRIBUTION.md", "ArtCraft\n"), ("LICENSE-MIT", "ArtCraft\n")]);
        assert_eq!(scan(&dir), []);
    }

    #[test]
    fn flags_logo_file() {
        let dir = tree(&[("docs/brand/artcraft-logo.png", "\u{89}PNG")]);
        assert_eq!(scan(&dir), [Finding { path: "docs/brand/artcraft-logo.png".into(), line: 0 }]);
    }

    #[test]
    fn top_level_catalogs_are_skipped_but_an_apps_i18n_code_is_not() {
        let dir =
            tree(&[("i18n/locales/nl/common.tsv", "ArtCraft\tArtCraft\n"), ("app/src/i18n/nl.rs", "(\"Join the ArtCraft Discord\", \"x\"),\n")]);
        assert_eq!(scan(&dir), [Finding { path: "app/src/i18n/nl.rs".into(), line: 1 }]);
    }

    #[test]
    fn credit_lines_pass_in_code_too() {
        let dir = tree(&[("src/about.rs", "label(\"Based on WordCraft by the ArtCraft team.\");\nlabel(\"Copyright (c) 2026 ArtCraft Team\");\n")]);
        assert_eq!(scan(&dir), []);
    }

    #[test]
    fn developer_notes_and_this_check_are_not_scanned_but_logo_names_are() {
        let dir = tree(&[
            ("docs/plan.md", "ArtCraft Native keeps an adapter\n"),
            ("assets/spelling/en-extra.txt", "artcraft\n"),
            ("xtask/src/brand.rs", "const MARK: &str = \"artcraft\";\n"),
            ("docs/brand/artcraft-logo.svg", "<svg/>"),
        ]);
        assert_eq!(scan(&dir), [Finding { path: "docs/brand/artcraft-logo.svg".into(), line: 0 }]);
    }

    #[test]
    fn readme_credit_lines_pass_but_other_readme_lines_do_not() {
        let dir = tree(&[("README.md", "Based on WordCraft by the ArtCraft team.\nJoin the ArtCraft Discord\n")]);
        assert_eq!(scan(&dir), [Finding { path: "README.md".into(), line: 2 }]);
    }
}
