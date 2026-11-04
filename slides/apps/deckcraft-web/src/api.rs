//! The page's JavaScript API (`deckcraft:call`, `deckcraft:result` and `deckcraft:event`, see
//! `host.rs`), as Craft Docs has it: calls run here against the app, and a [`Watcher`] turns
//! what changed since the last frame into events. No browser APIs here, so it is tested natively.

use serde_json::{Value, json};

use deckcraft_ui_egui::SlideApp;

/// The formats `getDocument` and the relay's HTTP API write (`png`: one slide, `slide` or the
/// one shown; `txt`: the outline).
pub const FORMATS: [&str; 6] = ["pptx", "deckcraft", "pdf", "png", "txt", "rtf"];

/// What a call answers: a JSON result, and for `getDocument` the file (name and bytes).
#[derive(Debug)]
pub struct Reply {
    pub result: Value,
    pub file: Option<(String, Vec<u8>)>,
}

/// Run API method `method`: `run` (any command), `getText` (the outline), `slides`,
/// `goToSlide`, `getSelection`, `setReadOnly` or `getDocument`. While co-editing the edits sync
/// like typed ones.
pub fn call(app: &mut SlideApp, method: &str, params: &Value) -> Result<Reply, String> {
    if !params.is_object() {
        return Err(format!("{method}: params must be an object"));
    }
    let result = match method {
        "run" => {
            let cmd = params.get("command").and_then(Value::as_str).ok_or("`command` required")?;
            app.run(cmd, params.get("params").cloned().unwrap_or_else(|| json!({})))?
        }
        "getText" => json!({"text": outline(app)?}),
        "slides" => slides(app).ok_or("no presentation is open")?,
        "goToSlide" => {
            let index = params.get("index").and_then(Value::as_u64).ok_or("`index` required: a slide number from 0")?;
            app.run("slide.go", json!({"index": index}))?
        }
        "getSelection" => selection(app).ok_or("no presentation is open")?,
        // The page's read only is a lock: the person can't leave it themselves.
        "setReadOnly" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            app.session.set_read_only(on, on);
            json!({"readOnly": on})
        }
        "getDocument" => {
            let format = params.get("format").and_then(Value::as_str).unwrap_or("pptx").to_ascii_lowercase();
            let slide = params.get("slide").and_then(Value::as_u64).map(|i| i as usize);
            let (name, bytes) = document_file(app, &format, slide)?;
            return Ok(Reply { result: json!({"name": name}), file: Some((name, bytes)) });
        }
        other => return Err(format!("unknown method `{other}`")),
    };
    Ok(Reply { result, file: None })
}

/// The presentation as a file of `format` (one of [`FORMATS`]): its name and bytes. A PNG is
/// slide `slide` (from 0), else the one shown.
pub fn document_file(app: &SlideApp, format: &str, slide: Option<usize>) -> Result<(String, Vec<u8>), String> {
    if !FORMATS.contains(&format) {
        return Err(format!("unsupported format `{format}` (one of {})", FORMATS.join(", ")));
    }
    let d = app.session.active().ok_or("no presentation is open")?;
    let title = d.title();
    let stem = std::path::Path::new(&title).file_stem().map_or_else(|| title.clone(), |s| s.to_string_lossy().to_string());
    let name = format!("{stem}.{format}");
    let bytes = match format {
        "png" => {
            let i = slide.unwrap_or(d.selection.slide);
            if i >= d.doc.slides.len() {
                return Err(format!("no slide {i}"));
            }
            deckcraft_engine::cmd::file::render_png(&d.doc, i, 2.0, false).0
        }
        f => deckcraft_engine::cmd::file::save_bytes(&d.doc, f).map_err(|e| e.to_string())?,
    };
    Ok((name, bytes))
}

/// The presentation's outline: each slide's title and text, for `getText` and the relay's `text`
/// endpoint.
pub fn outline(app: &SlideApp) -> Result<String, String> {
    let d = app.session.active().ok_or("no presentation is open")?;
    String::from_utf8(deckcraft_engine::cmd::file::save_bytes(&d.doc, "outline").map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Run command `cmd` for the relay's HTTP API as an edit by `author`: while co-editing it syncs
/// to everyone as `author`'s edit, and it isn't this person's to undo.
pub fn run_as(app: &mut SlideApp, cmd: &str, params: Value, author: &str) -> Result<Value, String> {
    if !params.is_object() {
        return Err(format!("{cmd}: params must be an object"));
    }
    app.session.as_api_author(author, |s| s.execute(cmd, &params)).map_err(|e| e.to_string())
}

/// The slides: index, id and title (empty when it has none).
fn slides(app: &SlideApp) -> Option<Value> {
    let d = app.session.active()?;
    let list: Vec<Value> = d.doc.slides.iter().enumerate().map(|(i, s)| json!({"index": i, "id": s.id, "title": s.title()})).collect();
    Some(json!({"slides": list, "current": d.selection.slide}))
}

/// Where the selection is: the slide shown, the shapes selected and the text being edited.
fn selection(app: &SlideApp) -> Option<Value> {
    let d = app.session.active()?;
    let sel = &d.selection;
    let text =
        sel.text.as_ref().map(|t| json!({"shape": t.shape, "anchor": [t.anchor.0, t.anchor.1], "caret": [t.caret.0, t.caret.1], "notes": t.notes}));
    Some(json!({"slide": sel.slide, "shapes": sel.shapes, "text": text}))
}

/// What the page has been told, to tell it what changed.
#[derive(Default)]
pub struct Watcher {
    seen: bool,
    /// The presentation the page was told about: another one (New, Open, joining a shared one)
    /// is recorded afresh, not reported as edits.
    document: u64,
    rev: u64,
    sel: Option<Value>,
    people: Vec<(String, u8)>,
}

impl Watcher {
    /// The events since the last call, at most one of each: `change` when the presentation's
    /// revision moved (`rev`: the shared revision when co-editing, else the presentation's; its
    /// author the last of `edited_by`, else this person), `selectionchange` and `presence` (the
    /// others here). The first call only records.
    pub fn events(&mut self, app: &SlideApp, rev: u64, edited_by: &[String], people: &[(String, u8)]) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        let document = app.session.active().map_or(0, |d| d.uid);
        let sel = selection(app);
        if self.seen && document == self.document {
            if rev != self.rev {
                let author = edited_by.last().cloned().unwrap_or_else(|| app.session.prefs.author.clone());
                out.push(("change", json!({"rev": rev, "author": author})));
            }
            if sel != self.sel
                && let Some(s) = &sel
            {
                out.push(("selectionchange", s.clone()));
            }
            if people != self.people.as_slice() {
                let list: Vec<Value> = people
                    .iter()
                    .map(|(name, color)| json!({"name": name, "color": deckcraft_ui_egui::canvas::peers::peer_css_color(*color)}))
                    .collect();
                out.push(("presence", json!({"people": list})));
            }
        }
        self.seen = true;
        self.document = document;
        self.rev = rev;
        self.sel = sel;
        self.people = people.to_vec();
        out
    }
}

/// The presentation's own revision, for [`Watcher::events`] when not co-editing.
pub fn local_rev(app: &SlideApp) -> u64 {
    app.session.active().map_or(0, |d| d.revision)
}

#[cfg(test)]
mod tests {
    use deckcraft_ui_egui::{Services, SlideApp};
    use serde_json::{Value, json};

    use super::{Watcher, call, document_file, local_rev, outline, run_as};

    fn app() -> SlideApp {
        let mut s = deckcraft_engine::Session::with_new();
        s.prefs.author = "Me".into();
        SlideApp::new(s, Services::default())
    }

    fn ok(app: &mut SlideApp, method: &str, params: Value) -> Value {
        call(app, method, &params).unwrap_or_else(|e| panic!("{method}: {e}")).result
    }

    #[test]
    fn slides_are_listed_gone_to_and_the_selection_is_told() {
        let mut a = app();
        ok(&mut a, "run", json!({"command": "slide.new"}));
        let list = ok(&mut a, "slides", json!({}));
        assert_eq!(list["slides"].as_array().map(Vec::len), Some(2), "{list}");
        assert_eq!(list["current"], 1);
        ok(&mut a, "goToSlide", json!({"index": 0}));
        assert_eq!(ok(&mut a, "getSelection", json!({}))["slide"], 0);
        assert!(call(&mut a, "goToSlide", &json!({})).unwrap_err().contains("index"));
        assert!(call(&mut a, "nope", &json!({})).unwrap_err().contains("unknown method"));
        assert!(outline(&a).is_ok());
    }

    #[test]
    fn get_document_returns_bytes_in_each_format() {
        let mut a = app();
        let file = |a: &SlideApp, f: &str| document_file(a, f, None);
        let (name, bytes) = file(&a, "pptx").unwrap();
        assert!(name.ends_with(".pptx") && bytes.starts_with(b"PK"), "{name}");
        assert!(file(&a, "pdf").unwrap().1.starts_with(b"%PDF"));
        assert!(file(&a, "png").unwrap().1.starts_with(b"\x89PNG"));
        assert!(document_file(&a, "png", Some(9)).unwrap_err().contains("no slide 9"));
        for f in ["deckcraft", "txt", "rtf"] {
            assert!(!file(&a, f).unwrap().1.is_empty(), "{f}");
        }
        assert!(file(&a, "exe").unwrap_err().contains("unsupported format"));
        let reply = call(&mut a, "getDocument", &json!({"format": "pdf"})).unwrap();
        assert!(reply.file.is_some_and(|(n, _)| n.ends_with(".pdf")));
    }

    #[test]
    fn the_relay_api_runs_as_its_author_and_the_page_hears_what_changed() {
        let mut a = app();
        a.session.set_collab(true);
        run_as(&mut a, "slide.new", json!({}), "Report Bot").unwrap();
        let out = a.session.take_outbox();
        assert_eq!((out[0].ctx.author.as_str(), out[0].ctx.api), ("Report Bot", true));
        assert!(run_as(&mut a, "slide.new", json!([]), "X").is_err());
        let mut w = Watcher::default();
        assert!(w.events(&a, local_rev(&a), &[], &[]).is_empty(), "the first call only records");
        ok(&mut a, "run", json!({"command": "slide.new"}));
        let ev = w.events(&a, local_rev(&a), &[], &[("Ann".into(), 1)]);
        let kinds: Vec<&str> = ev.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, ["change", "selectionchange", "presence"], "{ev:?}");
        assert_eq!(ev[0].1["author"], "Me");
        assert_eq!(ev[2].1["people"][0]["color"], "#1e7bd6");
    }
}
