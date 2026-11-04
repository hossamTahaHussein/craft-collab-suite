//! Text the standard fonts can't set: right-to-left scripts (Arabic, Hebrew) and scripts that
//! need shaping (joining, ligatures, marks). Such runs are put in display order (the Unicode
//! bidirectional algorithm), shaped with a bundled or installed face that covers them
//! (`wordcraft_fonts`, the fonts Craft Docs shapes with, in the cell's weight and slant), and
//! drawn glyph by glyph in that face, embedded with only the glyphs drawn (its own font out of a
//! collection). Each shaped run is marked with its text (`/ActualText`), so readers copy and
//! search it as written, Arabic vowels included; the face's `/ToUnicode` map is a fallback.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use wordcraft_fonts::{FontDb, FontFace};

/// A face embedded in the document and the glyphs drawn in it.
pub(crate) struct Embedded {
    pub(crate) face: Arc<FontFace>,
    /// Glyph id → (advance in 1/1000 em, the text it was first drawn for).
    pub(crate) glyphs: BTreeMap<u16, (u16, String)>,
}

/// The faces a document's shaped text uses, shared by its pages.
#[derive(Default)]
pub(crate) struct Registry {
    pub(crate) faces: Vec<Embedded>,
}

impl Registry {
    fn slot(&mut self, face: &Arc<FontFace>) -> usize {
        if let Some(i) = self.faces.iter().position(|e| e.face.id() == face.id()) {
            return i;
        }
        self.faces.push(Embedded { face: face.clone(), glyphs: BTreeMap::new() });
        self.faces.len() - 1
    }
}

/// One glyph of a shaped run, in points from the run's start along its baseline.
pub(crate) struct Placed {
    pub(crate) gid: u16,
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// Starts a cluster whose text the glyphs' own map doesn't give (a mark on its letter, a
    /// ligature, a glyph mapped to other text before): drawn in an `/ActualText` span of it.
    pub(crate) span: Option<String>,
    /// The cluster's last glyph (a span ends after it).
    pub(crate) ends: bool,
}

/// A run of text in display order: in the standard fonts, or shaped.
pub(crate) enum Piece {
    Plain(String),
    /// Glyphs in display order (left to right).
    Shaped {
        face: Arc<FontFace>,
        glyphs: Vec<(u16, i32, i32, i32, usize)>,
        text: String,
        upem: f32,
    },
}

impl Piece {
    /// Its width in points at `size` (`plain` measures a standard-font run).
    pub(crate) fn width(&self, size: f32, plain: impl Fn(&str) -> f32) -> f32 {
        match self {
            Piece::Plain(t) => plain(t),
            Piece::Shaped { glyphs, upem, .. } => glyphs.iter().map(|g| g.1 as f32).sum::<f32>() * size / upem,
        }
    }
}

/// A character the standard fonts can't set that isn't Chinese, Japanese or Korean (those keep
/// the predefined CID font).
fn complex(c: char) -> bool {
    let cp = c as u32;
    !c.is_control()
        && super::win_ansi(c) == b'?'
        && c != '?'
        && !matches!(cp, 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// Whether `text` needs shaping or display reordering.
pub(crate) fn needs(text: &str) -> bool {
    text.chars().any(|c| complex(c) || wordcraft_fonts::is_rtl(c))
}

/// `text` as runs in display order: a right-to-left run or one with characters the standard
/// fonts lack is shaped, in a `bold` or `italic` face when the text is; the rest stays plain.
/// Without a face for a run, it stays plain.
pub(crate) fn pieces(text: &str, bold: bool, italic: bool) -> Vec<Piece> {
    if !needs(text) {
        return vec![Piece::Plain(text.to_string())];
    }
    let bidi = unicode_bidi::BidiInfo::new(text, None);
    let mut out = Vec::new();
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let piece = &text[run.clone()];
            let rtl = levels.get(run.start).is_some_and(|l| l.is_rtl());
            if !rtl && !piece.chars().any(complex) {
                out.push(Piece::Plain(piece.to_string()));
                continue;
            }
            let chars: Vec<char> = piece.chars().filter(|c| !c.is_whitespace()).collect();
            let Some(face) = FontDb::global().fallback_for_run(&chars, 0, bold, italic) else {
                out.push(Piece::Plain(piece.to_string()));
                continue;
            };
            let shaped = wordcraft_fonts::shape_run(&face, piece, &[], |c| c, rtl);
            let mut glyphs: Vec<(u16, i32, i32, i32, usize)> =
                shaped.iter().filter_map(|g| u16::try_from(g.gid).ok().map(|gid| (gid, g.x_advance, g.x_offset, g.y_offset, g.cluster))).collect();
            if rtl {
                // Logical clusters (each cluster's glyphs already in display order) → display order.
                let mut groups: Vec<Vec<(u16, i32, i32, i32, usize)>> = Vec::new();
                for g in glyphs {
                    match groups.last_mut() {
                        Some(last) if last.first().is_some_and(|f| f.4 == g.4) => last.push(g),
                        _ => groups.push(vec![g]),
                    }
                }
                groups.reverse();
                glyphs = groups.into_iter().flatten().collect();
            }
            let upem = face.units_per_em().max(1.0) as f32;
            out.push(Piece::Shaped { face, glyphs, text: piece.to_string(), upem });
        }
    }
    out
}

/// Record the glyphs of a shaped run in `reg` and lay them out at `size`: the face's slot and
/// each glyph's position from the run's start.
pub(crate) fn place(
    reg: &mut Registry,
    face: &Arc<FontFace>,
    glyphs: &[(u16, i32, i32, i32, usize)],
    text: &str,
    upem: f32,
    size: f32,
) -> (usize, Vec<Placed>) {
    let slot = reg.slot(face);
    // Each glyph stands for the text of its cluster (its first glyph does; marks join it).
    let mut starts: Vec<usize> = glyphs.iter().map(|g| g.4).collect();
    starts.sort_unstable();
    starts.dedup();
    let cluster_text = |c: usize| {
        let end = starts.iter().copied().find(|s| *s > c).unwrap_or(text.len());
        text.get(c..end).unwrap_or_default().to_string()
    };
    let scale = size / upem;
    let mut pen = 0.0f32;
    let mut out = Vec::with_capacity(glyphs.len());
    let e = &mut reg.faces[slot];
    let mut i = 0;
    // Cluster by cluster, in display order.
    while i < glyphs.len() {
        let cluster = glyphs[i].4;
        let end = glyphs[i..].iter().position(|g| g.4 != cluster).map_or(glyphs.len(), |k| i + k);
        let text = cluster_text(cluster);
        let mut own = end - i == 1;
        for (k, &(gid, adv, dx, dy, _)) in glyphs[i..end].iter().enumerate() {
            let width = (face.advance(u32::from(gid)) * 1000.0 / f64::from(upem)).round().clamp(0.0, 65_535.0) as u16;
            // The first text a glyph is drawn for is its map entry; other text needs a span.
            let mapped = e.glyphs.entry(gid).or_insert_with(|| (width, if k == 0 { text.clone() } else { String::new() }));
            if k == 0 && mapped.1 != text {
                own = false;
            }
            out.push(Placed { gid, x: pen + dx as f32 * scale, y: dy as f32 * scale, span: None, ends: false });
            pen += adv as f32 * scale;
        }
        if !own && let Some(first) = out.len().checked_sub(end - i).and_then(|f| out.get_mut(f)) {
            first.span = Some(text);
            if let Some(last) = out.last_mut() {
                last.ends = true;
            }
        }
        i = end;
    }
    (slot, out)
}

/// `text` as a PDF UTF-16BE hex string with its byte order mark (for `/ActualText`).
pub(crate) fn utf16_hex(text: &str) -> String {
    let mut s = String::from("<FEFF");
    for u in text.encode_utf16() {
        let _ = write!(s, "{u:04X}");
    }
    s.push('>');
    s
}

/// Whether the face has TrueType outlines (else CFF).
fn truetype_face(e: &Embedded) -> bool {
    e.face.skrifa().is_some_and(|f| f.table_data(skrifa::raw::types::Tag::new(b"glyf")).is_some())
}

/// How many PDF object ids [`objects`] takes for one face.
pub(crate) const OBJECTS_PER_FACE: u32 = 6;

/// The face's own font, with only the glyphs drawn in it (and glyph 0), for embedding: TrueType
/// gets the map from the glyph ids drawn (the CIDs) to the subset's (a `/CIDToGIDMap` stream); a
/// CFF font keeps every glyph up to the last drawn, so its ids stay. `None` when the font can't
/// be subset (embedded as it is).
fn subset(e: &Embedded, truetype: bool) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let last = e.glyphs.keys().next_back().copied().unwrap_or(0);
    let mut remap = subsetter::GlyphRemapper::new();
    remap.remap(0);
    if truetype {
        for gid in e.glyphs.keys() {
            remap.remap(*gid);
        }
    } else {
        for gid in 0..=last {
            remap.remap(gid);
        }
    }
    let bytes = subsetter::subset(e.face.data(), e.face.index(), &remap).ok()?;
    let map = truetype.then(|| (0..=last).flat_map(|cid| remap.get(cid).unwrap_or(0).to_be_bytes()).collect());
    Some((bytes, map))
}

/// The PDF objects for embedded face `e` named `/E{slot+1}`: the Type 0 font (at `first_id`), its
/// CID font, descriptor, font file, ToUnicode map and CIDToGIDMap (the five ids after it).
pub(crate) fn objects(e: &Embedded, slot: usize, first_id: u32) -> Vec<(u32, Vec<u8>)> {
    let (font, cid, desc, file, cmap) = (first_id, first_id + 1, first_id + 2, first_id + 3, first_id + 4);
    let embedded = subset(e, truetype_face(e));
    // A subset's name carries a six-letter tag (PDF 9.6.4).
    let tag: String = (0..6).map(|i| char::from(b'A' + ((slot / 26usize.pow(i)) % 26) as u8)).collect();
    let name = if embedded.is_some() { format!("{tag}+CraftShaped{}", slot + 1) } else { format!("CraftShaped{}", slot + 1) };
    let data = e.face.data();
    let truetype = truetype_face(e);
    let upem = e.face.units_per_em().max(1.0);
    let (asc, desc_m) = e.face.vertical_metrics();
    let em = |v: f64| (v * 1000.0 / upem).round() as i64;
    let mut widths = String::new();
    for (gid, (w, _)) in &e.glyphs {
        let _ = write!(widths, "{gid} [{w}] ");
    }
    let mut out = Vec::new();
    out.push((
        font,
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {cmap} 0 R >>")
            .into_bytes(),
    ));
    let map_ref = match &embedded {
        Some((_, Some(_))) => format!(" /CIDToGIDMap {} 0 R", cmap + 1),
        _ => " /CIDToGIDMap /Identity".to_string(),
    };
    let (subtype, map) = if truetype { ("CIDFontType2", map_ref.as_str()) } else { ("CIDFontType0", "") };
    out.push((
        cid,
        format!(
            "<< /Type /Font /Subtype /{subtype} /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc} 0 R /DW 1000 /W [{widths}]{map} >>"
        )
        .into_bytes(),
    ));
    let file_key = if truetype { "FontFile2" } else { "FontFile3" };
    out.push((
        desc,
        format!(
            "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [-1000 {} 2000 {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /{file_key} {file} 0 R >>",
            em(desc_m.min(0.0)).min(-200),
            em(asc).max(800),
            em(asc),
            em(desc_m),
            em(asc)
        )
        .into_bytes(),
    ));
    let sub = if truetype { String::new() } else { " /Subtype /OpenType".to_string() };
    let bytes: &[u8] = embedded.as_ref().map_or(data, |(b, _)| b.as_slice());
    let mut o = format!("<< /Length {} /Length1 {}{sub} >>\nstream\n", bytes.len(), bytes.len()).into_bytes();
    o.extend_from_slice(bytes);
    o.extend_from_slice(b"\nendstream");
    out.push((file, o));
    // TrueType: the glyph ids drawn stay the CIDs, mapped to the subset's own glyph ids.
    // (The id is the face's either way: an empty object when the map is the identity.)
    match &embedded {
        Some((_, Some(map))) => {
            let mut o = format!("<< /Length {} >>\nstream\n", map.len()).into_bytes();
            o.extend_from_slice(map);
            o.extend_from_slice(b"\nendstream");
            out.push((cmap + 1, o));
        }
        _ => out.push((cmap + 1, b"null".to_vec())),
    }
    let mut cm = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let mapped: Vec<(&u16, &String)> = e.glyphs.iter().filter(|(_, (_, t))| !t.is_empty()).map(|(g, (_, t))| (g, t)).collect();
    for chunk in mapped.chunks(100) {
        let _ = writeln!(cm, "{} beginbfchar", chunk.len());
        for (gid, t) in chunk {
            let hex: String = t.encode_utf16().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cm, "<{gid:04X}> <{hex}>");
        }
        cm.push_str("endbfchar\n");
    }
    cm.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut o = format!("<< /Length {} >>\nstream\n", cm.len()).into_bytes();
    o.extend_from_slice(cm.as_bytes());
    o.extend_from_slice(b"endstream");
    out.push((cmap, o));
    out
}
