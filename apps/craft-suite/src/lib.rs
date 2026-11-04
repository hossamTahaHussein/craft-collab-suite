//! Craft Collab Suite: one start window and one command line over the three editors. A file opens
//! in the editor its extension names; each editor runs as its own app (its binary sits next to
//! this one), as the suite's per-app builds are tested and released.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One of the suite's editors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    Docs,
    Sheets,
    Slides,
}

impl Editor {
    pub const ALL: [Editor; 3] = [Editor::Docs, Editor::Sheets, Editor::Slides];

    /// Its name in the suite.
    pub fn name(self) -> &'static str {
        match self {
            Editor::Docs => "Craft Docs",
            Editor::Sheets => "Craft Sheets",
            Editor::Slides => "Craft Slides",
        }
    }

    /// Its id on the command line and in the relay (`--app`).
    pub fn id(self) -> &'static str {
        match self {
            Editor::Docs => "docs",
            Editor::Sheets => "sheets",
            Editor::Slides => "slides",
        }
    }

    pub fn parse(id: &str) -> Option<Editor> {
        Editor::ALL.into_iter().find(|e| e.id().eq_ignore_ascii_case(id))
    }

    /// What it makes new: a document, a workbook, a presentation.
    pub fn new_label(self) -> &'static str {
        match self {
            Editor::Docs => "New document",
            Editor::Sheets => "New workbook",
            Editor::Slides => "New presentation",
        }
    }

    /// The extensions it opens.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Editor::Docs => &["docx", "doc", "odt", "rtf", "txt", "md", "html", "htm"],
            Editor::Sheets => &["xlsx", "xlsm", "xls", "ods", "csv", "tsv"],
            Editor::Slides => &["pptx", "ppt", "odp", "potx", "ppsx", "deckcraft"],
        }
    }

    /// Its desktop app's and its command line's binaries.
    pub fn binary(self, cli: bool) -> &'static str {
        match (self, cli) {
            (Editor::Docs, false) => "wordcraft",
            (Editor::Docs, true) => "wordcraft-cli",
            (Editor::Sheets, false) => "gridcraft",
            (Editor::Sheets, true) => "gridcraft-cli",
            (Editor::Slides, false) => "deckcraft",
            (Editor::Slides, true) => "deckcraft-cli",
        }
    }
}

/// The editor that opens `path`, by its extension.
pub fn editor_for(path: &Path) -> Option<Editor> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Editor::ALL.into_iter().find(|e| e.extensions().contains(&ext.as_str()))
}

/// Where binary `name` is: next to this program when it is there (an installed suite), else
/// found on the PATH.
pub fn sibling(name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(file))
}

/// The command that opens `editor`, with `file` when given (else a new, empty one).
pub fn open_command(editor: Editor, file: Option<&Path>) -> Command {
    let mut c = Command::new(sibling(editor.binary(false)));
    if let Some(f) = file {
        c.arg(f);
    }
    c
}

/// `craft-suite-cli` arguments as the editor's own command line takes them: `collab-mcp --app
/// sheets …` runs `gridcraft-cli collab-mcp …`; `--app` may come anywhere; without it, Craft Docs.
pub fn cli_command(args: &[OsString]) -> Result<Command, String> {
    let mut editor = Editor::Docs;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--app" {
            let id = it.next().and_then(|v| v.to_str()).ok_or("--app needs docs, sheets or slides")?;
            editor = Editor::parse(id).ok_or_else(|| format!("unknown app `{id}` (docs, sheets or slides)"))?;
        } else if let Some(id) = a.to_str().and_then(|s| s.strip_prefix("--app=")) {
            editor = Editor::parse(id).ok_or_else(|| format!("unknown app `{id}` (docs, sheets or slides)"))?;
        } else {
            rest.push(a.clone());
        }
    }
    let mut c = Command::new(sibling(editor.binary(true)));
    c.args(rest);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_open_in_the_editor_their_extension_names() {
        let at = |p: &str| editor_for(Path::new(p));
        assert_eq!(at("report.DOCX"), Some(Editor::Docs));
        assert_eq!(at("notes.odt"), Some(Editor::Docs));
        assert_eq!(at("budget.xlsx"), Some(Editor::Sheets));
        assert_eq!(at("data.csv"), Some(Editor::Sheets));
        assert_eq!(at("talk.pptx"), Some(Editor::Slides));
        assert_eq!(at("talk.odp"), Some(Editor::Slides));
        assert_eq!(at("photo.png"), None);
        assert_eq!(at("README"), None);
    }

    #[test]
    fn the_command_line_runs_the_apps_own() {
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        let c = cli_command(&args(&["collab-mcp", "--app", "sheets", "--relay", "ws://x"])).unwrap();
        assert!(c.get_program().to_string_lossy().contains("gridcraft-cli"), "{c:?}");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["collab-mcp", "--relay", "ws://x"]);
        let c = cli_command(&args(&["--app=slides", "convert", "a.pptx", "a.pdf"])).unwrap();
        assert!(c.get_program().to_string_lossy().contains("deckcraft-cli"));
        let c = cli_command(&args(&["text", "a.docx"])).unwrap();
        assert!(c.get_program().to_string_lossy().contains("wordcraft-cli"), "Craft Docs without --app");
        assert!(cli_command(&args(&["--app", "paint"])).unwrap_err().contains("unknown app"));
        assert!(cli_command(&args(&["--app"])).is_err());
    }

    #[test]
    fn a_new_file_or_an_opened_one_starts_its_editor() {
        let c = open_command(Editor::Slides, Some(Path::new("talk.pptx")));
        assert!(c.get_program().to_string_lossy().contains("deckcraft"));
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["talk.pptx"]);
        assert_eq!(open_command(Editor::Docs, None).get_args().count(), 0);
    }
}
