//! The page's JavaScript API (`gridcraft:call`, `gridcraft:result` and `gridcraft:event`, see
//! `host.rs`), as Craft Docs has it: calls run here against the app, and a [`Watcher`] turns
//! what changed since the last frame into events. No browser APIs here, so it is tested natively.

use serde_json::{Value, json};

use gridcraft_ui_egui::SheetApp;

/// The formats `getDocument` and the relay's HTTP API write (ODS is read, not written).
pub const FORMATS: [&str; 6] = ["xlsx", "csv", "tsv", "json", "html", "pdf"];

/// What a call answers: a JSON result, and for `getDocument` the file (name and bytes).
#[derive(Debug)]
pub struct Reply {
    pub result: Value,
    pub file: Option<(String, Vec<u8>)>,
}

/// Run API method `method`: `run` (any command), `getText`, `getRange`, `setRange`,
/// `getSelection`, `activeSheet`, `setReadOnly` or `getDocument`. While co-editing the edits sync
/// like typed ones.
pub fn call(app: &mut SheetApp, method: &str, params: &Value) -> Result<Reply, String> {
    if !params.is_object() {
        return Err(format!("{method}: params must be an object"));
    }
    let text = |key: &str| params.get(key).and_then(Value::as_str).ok_or_else(|| format!("`{key}` required"));
    let sheet = params.get("sheet").cloned().unwrap_or(Value::Null);
    let result = match method {
        "run" => app.run(text("command")?, params.get("params").cloned().unwrap_or_else(|| json!({})))?,
        "getText" => json!({"text": workbook_text(app)?}),
        "getRange" => {
            let mut p = json!({"range": text("range")?, "formatted": params.get("formatted").and_then(Value::as_bool).unwrap_or(false)});
            if !sheet.is_null() {
                p["sheet"] = sheet;
            }
            app.run("sheet.read", p)?
        }
        "setRange" => {
            let values = params.get("values").filter(|v| v.is_array()).cloned().ok_or("`values` required: rows of values")?;
            let mut p = json!({"range": text("range")?, "values": values});
            if !sheet.is_null() {
                p["sheet"] = sheet;
            }
            app.run("range.setValues", p)?
        }
        "getSelection" => selection(app).ok_or("no workbook is open")?,
        "activeSheet" => {
            let d = app.session.active().ok_or("no workbook is open")?;
            let i = d.wb.active_sheet();
            json!({"index": i, "name": d.wb.sheets.get(i).map(|s| s.name.clone()).unwrap_or_default()})
        }
        // The page's read only is a lock: the person can't leave it themselves.
        "setReadOnly" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            app.session.set_read_only(on, on);
            json!({"readOnly": on})
        }
        "getDocument" => {
            let format = params.get("format").and_then(Value::as_str).unwrap_or("xlsx").to_ascii_lowercase();
            let (name, bytes) = document_file(app, &format)?;
            return Ok(Reply { result: json!({"name": name}), file: Some((name, bytes)) });
        }
        other => return Err(format!("unknown method `{other}`")),
    };
    Ok(Reply { result, file: None })
}

/// The workbook as a file of `format` (one of [`FORMATS`]): its name and bytes. A CSV, TSV or
/// PDF is the active sheet, as Save As writes them.
pub fn document_file(app: &mut SheetApp, format: &str) -> Result<(String, Vec<u8>), String> {
    if !FORMATS.contains(&format) {
        return Err(format!("unsupported format `{format}` (one of {})", FORMATS.join(", ")));
    }
    let d = app.session.active().ok_or("no workbook is open")?;
    let title = d.display_title();
    let stem = std::path::Path::new(&title).file_stem().map_or_else(|| title.clone(), |s| s.to_string_lossy().to_string());
    let name = format!("{stem}.{format}");
    if format == "pdf" {
        let out = app.session.execute("file.exportPdf", json!({})).map_err(|e| e.to_string())?;
        let data = out.get("base64").and_then(Value::as_str).ok_or("the PDF wasn't made")?;
        let bytes = gridcraft_engine::io::base64_decode(data).ok_or("the PDF wasn't made")?;
        return Ok((name, bytes));
    }
    let bytes = gridcraft_engine::io::save_bytes(&d.wb, &name).map_err(|e| e.to_string())?;
    Ok((name, bytes))
}

/// Every sheet as text for the relay's `text` endpoint and `getText`: each sheet's name on a
/// line of its own, then its cells as CSV (as displayed), then a blank line.
pub fn workbook_text(app: &SheetApp) -> Result<String, String> {
    let d = app.session.active().ok_or("no workbook is open")?;
    let mut out = String::new();
    for sheet in &d.wb.sheets {
        out.push_str(&sheet.name);
        out.push('\n');
        if let Some(used) = sheet.used_range() {
            for row in used.start.row..=used.end.row {
                let cells: Vec<String> = (used.start.col..=used.end.col)
                    .map(|col| csv_field(&gridcraft_engine::display::cell_text(&d.wb, sheet, gridcraft_engine::core::CellRef { row, col })))
                    .collect();
                out.push_str(&cells.join(","));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// Run command `cmd` for the relay's HTTP API as an edit by `author`: while co-editing it syncs
/// to everyone as `author`'s edit, and it isn't this person's to undo.
pub fn run_as(app: &mut SheetApp, cmd: &str, params: Value, author: &str) -> Result<Value, String> {
    if !params.is_object() {
        return Err(format!("{cmd}: params must be an object"));
    }
    app.session.as_api_author(author, |s| s.execute(cmd, params)).map_err(|e| e.to_string())
}

/// Where the selection is: its sheet, active cell and ranges (A1).
fn selection(app: &SheetApp) -> Option<Value> {
    let d = app.session.active()?;
    let sheet = d.wb.active()?.name.clone();
    let ranges: Vec<String> = d.selection.ranges.iter().map(|r| r.a1()).collect();
    Some(json!({"sheet": sheet, "active": d.selection.active.a1(), "ranges": ranges}))
}

/// A person's colour (an index into the palette) as CSS (`#rrggbb`, the light palette).
pub fn peer_css_color(index: u8) -> String {
    let [r, g, b] = gridcraft_ui_egui::grid::peers::PEER_COLORS[usize::from(index) % 12];
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// What the page has been told, to tell it what changed.
#[derive(Default)]
pub struct Watcher {
    seen: bool,
    /// The workbook the page was told about: another one (New, Open, joining a shared one) is
    /// recorded afresh, not reported as edits.
    document: u64,
    rev: u64,
    sel: Option<Value>,
    people: Vec<(String, u8)>,
}

impl Watcher {
    /// The events since the last call, at most one of each: `change` when the workbook's revision
    /// moved (`rev`: the shared revision when co-editing, else the workbook's; its author the last
    /// of `edited_by`, else this person), `selectionchange` and `presence` (the others here). The
    /// first call only records.
    pub fn events(&mut self, app: &SheetApp, rev: u64, edited_by: &[String], people: &[(String, u8)]) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        let document = app.session.active().map_or(0, |d| d.uid);
        let sel = selection(app);
        if self.seen && document == self.document {
            if rev != self.rev {
                let author = edited_by.last().cloned().unwrap_or_else(|| app.session.prefs.user_name.clone());
                out.push(("change", json!({"rev": rev, "author": author})));
            }
            if sel != self.sel
                && let Some(s) = &sel
            {
                out.push(("selectionchange", s.clone()));
            }
            if people != self.people.as_slice() {
                let list: Vec<Value> = people.iter().map(|(name, color)| json!({"name": name, "color": peer_css_color(*color)})).collect();
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

/// The workbook's own revision, for [`Watcher::events`] when not co-editing.
pub fn local_rev(app: &SheetApp) -> u64 {
    app.session.active().map_or(0, |d| d.revision)
}

#[cfg(test)]
mod tests {
    use gridcraft_ui_egui::{Services, SheetApp};
    use serde_json::{Value, json};

    use super::{Watcher, call, document_file, local_rev, run_as, workbook_text};

    fn app() -> SheetApp {
        let mut s = gridcraft_engine::Session::new();
        s.new_workbook();
        s.prefs.user_name = "Me".into();
        SheetApp::new(s, Services::default())
    }

    fn ok(app: &mut SheetApp, method: &str, params: Value) -> Value {
        call(app, method, &params).unwrap_or_else(|e| panic!("{method}: {e}")).result
    }

    #[test]
    fn ranges_are_written_and_read_back_and_the_selection_and_sheet_are_told() {
        let mut a = app();
        ok(&mut a, "setRange", json!({"range": "A1", "values": [["Name", "Total"], ["Ann", 3]]}));
        let read = ok(&mut a, "getRange", json!({"range": "A1:B2"}));
        assert!(read.to_string().contains("Ann") && read.to_string().contains('3'), "{read}");
        ok(&mut a, "run", json!({"command": "edit.goTo", "params": {"reference": "B2"}}));
        assert_eq!(ok(&mut a, "getSelection", json!({}))["active"], "B2");
        assert_eq!(ok(&mut a, "activeSheet", json!({})), json!({"index": 0, "name": "Sheet1"}));
        assert!(workbook_text(&a).unwrap().starts_with("Sheet1\nName,Total\nAnn,3\n"), "{:?}", workbook_text(&a));
        assert!(call(&mut a, "nope", &json!({})).unwrap_err().contains("unknown method"));
        assert!(call(&mut a, "getRange", &json!([])).unwrap_err().contains("must be an object"));
    }

    #[test]
    fn get_document_returns_bytes_in_each_format() {
        let mut a = app();
        ok(&mut a, "setRange", json!({"range": "A1", "values": [["Hi"]]}));
        let (name, bytes) = document_file(&mut a, "xlsx").unwrap();
        assert!(name.ends_with(".xlsx") && bytes.starts_with(b"PK"), "{name}");
        assert!(document_file(&mut a, "pdf").unwrap().1.starts_with(b"%PDF"));
        assert!(String::from_utf8_lossy(&document_file(&mut a, "csv").unwrap().1).contains("Hi"));
        for f in ["tsv", "json", "html"] {
            assert!(!document_file(&mut a, f).unwrap().1.is_empty(), "{f}");
        }
        assert!(document_file(&mut a, "exe").unwrap_err().contains("unsupported format"));
        let reply = call(&mut a, "getDocument", &json!({"format": "csv"})).unwrap();
        assert!(reply.file.is_some_and(|(n, _)| n.ends_with(".csv")));
    }

    #[test]
    fn the_relay_api_runs_as_its_author_and_the_page_hears_what_changed() {
        let mut a = app();
        a.session.set_collab(true);
        run_as(&mut a, "cell.set", json!({"cell": "A1", "input": "5"}), "Report Bot").unwrap();
        let out = a.session.take_outbox();
        assert_eq!((out[0].ctx.author.as_str(), out[0].ctx.api), ("Report Bot", true));
        assert!(run_as(&mut a, "cell.set", json!([]), "X").is_err());
        let mut w = Watcher::default();
        assert!(w.events(&a, local_rev(&a), &[], &[]).is_empty(), "the first call only records");
        ok(&mut a, "setRange", json!({"range": "B1", "values": [[1]]}));
        let ev = w.events(&a, local_rev(&a), &[], &[("Ann".into(), 1)]);
        let kinds: Vec<&str> = ev.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, ["change", "presence"], "{ev:?}");
        assert_eq!(ev[0].1["author"], "Me");
        assert_eq!(ev[1].1["people"][0]["color"], "#1e7bd6");
        ok(&mut a, "run", json!({"command": "edit.goTo", "params": {"reference": "C3"}}));
        let ev = w.events(&a, local_rev(&a), &[], &[("Ann".into(), 1)]);
        assert_eq!(ev.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["selectionchange"]);
        assert_eq!(ev[0].1["active"], "C3");
    }
}
