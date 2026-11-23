//! A small reader for `.xlsx` workbooks: the cell values of the first worksheet as text rows.
//!
//! An XLSX file is a zip of XML parts. Only what a table of survey points needs is read: shared
//! strings, inline strings and numbers of the first sheet. Formulas are read as their last
//! calculated value, styles are ignored, and a file in the old `.xls` format is refused with a
//! message that says what to do instead.

use std::io::{Cursor, Read};

use crate::points::ParseError;

/// The most decompressed bytes read from any part: a zip bomb cannot exhaust memory.
const MAX_PART: u64 = 256 * 1024 * 1024;

fn err<T>(msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(msg.into()))
}

/// Rows of cells `(row number, cells)` of the first worksheet; empty rows are left out.
pub fn read_rows(bytes: &[u8]) -> Result<Vec<(usize, Vec<String>)>, ParseError> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return err("this is an old .xls file: save it as .xlsx or .csv in Excel and import that");
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| ParseError(format!("not a valid .xlsx file: {e}")))?;
    let shared = match read_part(&mut zip, "xl/sharedStrings.xml") {
        Some(xml) => shared_strings(&xml),
        None => Vec::new(),
    };
    let sheet_name = if zip.file_names().filter_map(Result::ok).any(|n| n == "xl/worksheets/sheet1.xml") {
        "xl/worksheets/sheet1.xml".to_string()
    } else {
        let mut names: Vec<String> = zip
            .file_names()
            .filter_map(Result::ok)
            .filter(|n| n.starts_with("xl/worksheets/") && n.ends_with(".xml"))
            .map(|n| n.to_string())
            .collect();
        names.sort();
        names.into_iter().next().ok_or_else(|| ParseError("the workbook has no worksheet".into()))?
    };
    let xml = read_part(&mut zip, &sheet_name).ok_or_else(|| ParseError(format!("can't read {sheet_name}")))?;
    Ok(sheet_rows(&xml, &shared))
}

fn read_part<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let file = zip.by_name(name).ok()?;
    let mut s = String::new();
    file.take(MAX_PART).read_to_string(&mut s).ok()?;
    Some(s)
}

/// One piece of XML: a start tag with its attributes text, an end tag, or character data.
enum Tok<'a> {
    Open { name: &'a str, attrs: &'a str, empty: bool },
    Close(&'a str),
    Text(&'a str),
}

fn tokens(xml: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while !rest.is_empty() {
        match rest.find('<') {
            None => {
                out.push(Tok::Text(rest));
                break;
            }
            Some(0) => {}
            Some(i) => {
                out.push(Tok::Text(&rest[..i]));
                rest = &rest[i..];
            }
        }
        // `rest` starts with '<'.
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map_or("", |e| &rest[e + 3..]);
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").unwrap_or(rest.len());
            out.push(Tok::Text(&rest[9..end]));
            rest = rest.get(end + 3..).unwrap_or("");
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            out.push(Tok::Close(name.trim()));
            continue;
        }
        let empty = inner.ends_with('/');
        let inner = inner.trim_end_matches('/');
        let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        out.push(Tok::Open { name, attrs, empty });
    }
    out
}

/// The value of attribute `key` in a start tag's attribute text.
fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(i) = rest.find(key) {
        let before_ok = i == 0 || rest[..i].ends_with(char::is_whitespace);
        let after = &rest[i + key.len()..];
        let after = after.trim_start();
        if before_ok && let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let body = &v[1..];
                return body.find(q).map(|e| &body[..e]);
            }
        }
        rest = &rest[i + key.len()..];
    }
    None
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn shared_strings(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_t = false;
    for t in tokens(xml) {
        match t {
            Tok::Open { name: "si", empty, .. } => {
                if empty {
                    out.push(String::new());
                } else {
                    cur = Some(String::new());
                }
            }
            Tok::Close("si") => out.extend(cur.take()),
            Tok::Open { name: "t", empty: false, .. } => in_t = true,
            Tok::Close("t") => in_t = false,
            Tok::Text(x) if in_t => {
                if let Some(c) = cur.as_mut() {
                    c.push_str(&unescape(x));
                }
            }
            _ => {}
        }
    }
    out
}

/// The zero-based column of a cell reference such as `AB12`.
fn column_of(cell_ref: &str) -> usize {
    let mut n = 0usize;
    for c in cell_ref.chars().take_while(char::is_ascii_alphabetic) {
        n = n.saturating_mul(26).saturating_add(usize::from(c.to_ascii_uppercase() as u8 - b'A') + 1);
    }
    n.saturating_sub(1)
}

/// The most columns read from a row; a sheet that wide is not a point table.
const MAX_COLS: usize = 256;

fn sheet_rows(xml: &str, shared: &[String]) -> Vec<(usize, Vec<String>)> {
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    let mut row_no = 0usize;
    let mut cells: Vec<String> = Vec::new();
    let (mut col, mut kind) = (0usize, String::new());
    let mut text = String::new();
    let mut capture = false; // inside <v> or an inline string's <t>
    for t in tokens(xml) {
        match t {
            Tok::Open { name: "row", attrs, .. } => {
                row_no = attr(attrs, "r").and_then(|r| r.parse().ok()).unwrap_or(row_no + 1);
                cells.clear();
            }
            Tok::Close("row") => {
                if cells.iter().any(|c| !c.is_empty()) {
                    rows.push((row_no, std::mem::take(&mut cells)));
                }
                cells.clear();
            }
            Tok::Open { name: "c", attrs, .. } => {
                col = attr(attrs, "r").map_or(cells.len(), column_of).min(MAX_COLS);
                kind = attr(attrs, "t").unwrap_or("").to_string();
                text.clear();
            }
            Tok::Open { name: "v", empty: false, .. } | Tok::Open { name: "t", empty: false, .. } => capture = true,
            Tok::Close("v") | Tok::Close("t") => capture = false,
            Tok::Text(x) if capture => text.push_str(&unescape(x)),
            Tok::Close("c") => {
                let value = match kind.as_str() {
                    "s" => text.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).cloned().unwrap_or_default(),
                    _ => text.clone(),
                };
                if cells.len() <= col {
                    cells.resize(col + 1, String::new());
                }
                if let Some(slot) = cells.get_mut(col) {
                    *slot = value;
                }
            }
            _ => {}
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn workbook(sheet: &str, shared: &str) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("xl/worksheets/sheet1.xml", o).unwrap();
            z.write_all(sheet.as_bytes()).unwrap();
            z.start_file("xl/sharedStrings.xml", o).unwrap();
            z.write_all(shared.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn reads_shared_strings_numbers_and_skips_gaps() {
        let shared = r#"<sst><si><t>Punto</t></si><si><t>Este</t></si><si><t>Cota</t></si><si><t>A &amp; B</t></si></sst>"#;
        let sheet = r#"<worksheet><sheetData>
            <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="D1" t="s"><v>2</v></c></row>
            <row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>500.25</v></c><c r="D2"><v>12.5</v></c></row>
            <row r="4"><c r="A4" t="s"><v>3</v></c></row></sheetData></worksheet>"#;
        let rows = read_rows(&workbook(sheet, shared)).unwrap();
        assert_eq!(rows[0], (1, vec!["Punto".into(), "Este".into(), "".into(), "Cota".into()]));
        assert_eq!(rows[1].1[1], "500.25");
        assert_eq!(rows[2], (4, vec!["A & B".to_string()]));
    }

    #[test]
    fn old_xls_and_garbage_are_refused_with_a_reason() {
        assert!(read_rows(&[0xD0, 0xCF, 0x11, 0xE0, 0, 0]).unwrap_err().0.contains(".xls"));
        assert!(read_rows(b"not a zip").is_err());
    }
}
