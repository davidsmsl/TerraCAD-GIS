//! Survey points: reading them from text files, spreadsheets and drawings.
//!
//! A point file lists one point per line: an optional point number, the coordinates, an optional
//! elevation and an optional description (code). The order of the columns is given as a column
//! spec, one letter per column:
//!
//! | Letter | Column |
//! |---|---|
//! | `P` | point number or name |
//! | `N` | northing (the Y coordinate) |
//! | `E` | easting (the X coordinate) |
//! | `X`, `Y`, `Z` | the coordinate of that name; `Z` is the elevation |
//! | `D` | description or code |
//! | `-` | a column to ignore |
//!
//! Without a spec the module reads the header row when there is one (it knows the usual Spanish
//! and English column names), and otherwise assumes `PNEZD` for four or more columns and `XYZ`
//! for three.

use cadcraft_doc::{Drawing, EntityKind};

/// More points than this in one file is refused: a TIN of them would not finish in a useful time.
pub const MAX_POINTS: usize = 1_000_000;

/// One surveyed point.
#[derive(Clone, Debug, PartialEq)]
pub struct SurveyPoint {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub code: String,
}

/// What a column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Id,
    X,
    Y,
    Z,
    Code,
    Skip,
}

/// How to read a text file or a table of cells.
#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    /// A column spec such as `PNEZD` or `XYZ`; `None` reads the header or guesses.
    pub columns: Option<String>,
    /// The field separator; `None` detects tab, semicolon, comma or blanks.
    pub delimiter: Option<char>,
}

/// The points read and what was left out.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub points: Vec<SurveyPoint>,
    /// `(row number, reason)` for each row that was not a point (1-based, as in the file).
    pub skipped: Vec<(usize, String)>,
    /// The column spec that was used, so the caller can show it.
    pub columns: String,
}

/// Why a file could not be read at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(msg.into()))
}

/// A column spec such as `PNEZD` as columns; an unknown letter is an error.
pub fn parse_spec(spec: &str) -> Result<Vec<Col>, ParseError> {
    let mut cols = Vec::new();
    for c in spec.trim().chars().filter(|c| !c.is_whitespace() && *c != ',') {
        cols.push(match c.to_ascii_uppercase() {
            'P' => Col::Id,
            'N' | 'Y' => Col::Y,
            'E' | 'X' => Col::X,
            'Z' | 'H' => Col::Z,
            'D' | 'C' => Col::Code,
            '-' | '_' => Col::Skip,
            o => return err(format!("column spec: unknown letter `{o}` (use P, N, E, X, Y, Z, D or -)")),
        });
    }
    if !cols.contains(&Col::X) || !cols.contains(&Col::Y) {
        return err("column spec needs both an easting (E or X) and a northing (N or Y)");
    }
    Ok(cols)
}

fn spec_string(cols: &[Col]) -> String {
    cols.iter()
        .map(|c| match c {
            Col::Id => 'P',
            Col::X => 'X',
            Col::Y => 'Y',
            Col::Z => 'Z',
            Col::Code => 'D',
            Col::Skip => '-',
        })
        .collect()
}

/// The column a header cell names, if it is one of the usual names.
fn header_col(cell: &str) -> Option<Col> {
    let k: String = cell
        .trim()
        .trim_matches('"')
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' => 'u',
            o => o,
        })
        .filter(|c| c.is_alphanumeric())
        .collect();
    Some(match k.as_str() {
        "p" | "punto" | "pto" | "id" | "no" | "num" | "numero" | "point" | "name" | "nombre" | "pointid" => Col::Id,
        "n" | "norte" | "north" | "northing" | "y" | "latitud" => Col::Y,
        "e" | "este" | "east" | "easting" | "x" | "longitud" => Col::X,
        "cota" | "elev" | "elevacion" | "elevation" | "z" | "altura" | "altitud" | "h" => Col::Z,
        "descripcion" | "desc" | "codigo" | "code" | "description" | "observaciones" | "obs" => Col::Code,
        _ => return None,
    })
}

/// A number from text: a point or a comma as the decimal mark, no thousands separators.
fn number(s: &str) -> Option<f64> {
    let t = s.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    let v: f64 = if t.contains(',') && !t.contains('.') { t.replace(',', ".").parse().ok()? } else { t.parse().ok()? };
    v.is_finite().then_some(v)
}

/// Split a line on `delim`, honouring double quotes; `None` splits on runs of blanks.
fn split_line(line: &str, delim: Option<char>) -> Vec<String> {
    let Some(d) = delim else { return line.split_whitespace().map(str::to_string).collect() };
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c == d && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// The separator that splits the first lines into the most equal, largest number of fields.
fn detect_delimiter(lines: &[&str]) -> Option<char> {
    let sample: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).take(10).collect();
    for d in ['\t', ';', ','] {
        let counts: Vec<usize> = sample.iter().map(|l| l.matches(d).count()).collect();
        if counts.first().is_some_and(|c| *c >= 2) && counts.iter().all(|c| Some(c) == counts.first()) {
            return Some(d);
        }
    }
    // A comma file with one or two columns less than the rest still counts when most lines agree.
    ['\t', ';', ','].into_iter().find(|&d| sample.iter().filter(|l| l.matches(d).count() >= 2).count() * 2 > sample.len())
}

/// Text from file bytes: UTF-8, or Windows-1252 when the bytes are not valid UTF-8.
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(t) => t.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

/// Read points from delimited text.
pub fn parse_text(text: &str, opts: &ParseOptions) -> Result<Parsed, ParseError> {
    let text = text.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = text.lines().collect();
    let delim = match opts.delimiter {
        Some(d) => Some(d),
        None => detect_delimiter(&lines),
    };
    let rows: Vec<(usize, Vec<String>)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("//")
        })
        .map(|(i, l)| (i + 1, split_line(l, delim)))
        .collect();
    parse_rows(&rows, opts)
}

/// Read points from rows of cells, each with its row number. Used by the text and XLSX readers.
pub fn parse_rows(rows: &[(usize, Vec<String>)], opts: &ParseOptions) -> Result<Parsed, ParseError> {
    let Some((_, first)) = rows.first() else { return err("the file has no rows") };
    let mut start = 0;
    let cols: Vec<Col> = match &opts.columns {
        Some(spec) => {
            let cols = parse_spec(spec)?;
            // A header row is skipped even when the spec was given.
            if looks_like_header(first, &cols) {
                start = 1;
            }
            cols
        }
        None => {
            let named: Vec<Option<Col>> = first.iter().map(|c| header_col(c)).collect();
            let numeric = first.iter().filter(|c| number(c).is_some()).count();
            if named.iter().filter(|c| c.is_some()).count() >= 2 && numeric < 2 {
                start = 1;
                named.into_iter().map(|c| c.unwrap_or(Col::Skip)).collect()
            } else {
                let n = first.len();
                match n {
                    0..=2 => return err("a point row needs at least an easting, a northing and an elevation"),
                    3 => vec![Col::X, Col::Y, Col::Z],
                    4 => vec![Col::Id, Col::Y, Col::X, Col::Z],
                    _ => vec![Col::Id, Col::Y, Col::X, Col::Z, Col::Code],
                }
            }
        }
    };
    if !cols.contains(&Col::X) || !cols.contains(&Col::Y) {
        return err("the header has no easting/northing columns; give the column order, e.g. PNEZD");
    }
    let mut out = Parsed { columns: spec_string(&cols), ..Parsed::default() };
    for (line, cells) in rows.iter().skip(start) {
        match row_point(cells, &cols, out.points.len() + 1) {
            Ok(p) => {
                if out.points.len() >= MAX_POINTS {
                    return err(format!("more than {MAX_POINTS} points: split the file"));
                }
                out.points.push(p);
            }
            Err(why) => out.skipped.push((*line, why)),
        }
    }
    if out.points.is_empty() {
        let hint = out.skipped.first().map(|(l, w)| format!(" (row {l}: {w})")).unwrap_or_default();
        return err(format!("no points found with columns {}{hint}", out.columns));
    }
    Ok(out)
}

fn looks_like_header(cells: &[String], cols: &[Col]) -> bool {
    let coords: Vec<usize> = cols.iter().enumerate().filter(|(_, c)| matches!(c, Col::X | Col::Y)).map(|(i, _)| i).collect();
    !coords.is_empty() && coords.iter().all(|i| cells.get(*i).is_none_or(|c| number(c).is_none()))
}

fn row_point(cells: &[String], cols: &[Col], ordinal: usize) -> Result<SurveyPoint, String> {
    let mut p = SurveyPoint { id: String::new(), x: 0.0, y: 0.0, z: 0.0, code: String::new() };
    let (mut has_x, mut has_y) = (false, false);
    for (col, cell) in cols.iter().zip(cells.iter()) {
        let cell = cell.trim();
        match col {
            Col::Id => p.id = cell.to_string(),
            Col::Code => p.code = cell.to_string(),
            Col::Skip => {}
            Col::X => {
                p.x = number(cell).ok_or_else(|| format!("easting `{cell}` is not a number"))?;
                has_x = true;
            }
            Col::Y => {
                p.y = number(cell).ok_or_else(|| format!("northing `{cell}` is not a number"))?;
                has_y = true;
            }
            // A missing or non-numeric elevation is a point at elevation 0, not a lost point.
            Col::Z => p.z = number(cell).unwrap_or(0.0),
        }
    }
    if !has_x || !has_y {
        return Err("the row has fewer columns than the spec".into());
    }
    if p.id.is_empty() {
        p.id = ordinal.to_string();
    }
    Ok(p)
}

/// The points a drawing holds: POINT objects, the vertices of 3D polylines and the corners of
/// 3D faces, each with its Z. Duplicates (same X, Y and Z) are listed once.
/// With `layer` set, only objects on that layer (compared without regard to case) are read.
pub fn from_drawing(d: &Drawing, layer: Option<&str>) -> Vec<SurveyPoint> {
    let mut out: Vec<SurveyPoint> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |x: f64, y: f64, z: f64, code: &str| {
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            return;
        }
        if seen.insert((x.to_bits(), y.to_bits(), z.to_bits())) && out.len() < MAX_POINTS {
            let id = (out.len() + 1).to_string();
            out.push(SurveyPoint { id, x, y, z, code: code.to_string() });
        }
    };
    for e in d.model.iter() {
        let on = e.common.layer.as_str();
        if layer.is_some_and(|l| !l.eq_ignore_ascii_case(on)) {
            continue;
        }
        match &e.kind {
            EntityKind::Point(p) => add(p.p.x, p.p.y, p.p.z, on),
            EntityKind::Polyline3d(pl) => pl.points.iter().for_each(|v| add(v.x, v.y, v.z, on)),
            EntityKind::Face3d(f) => f.corners.iter().for_each(|v| add(v.x, v.y, v.z, on)),
            _ => {}
        }
    }
    out
}

/// Read a point file by what its name says it is: `.xlsx` (first sheet), `.dxf` or `.dwg` (the
/// points of the drawing), or else delimited text (CSV, TXT, XYZ, PTS...).
pub fn read_file(bytes: &[u8], name: &str, opts: &ParseOptions) -> Result<Parsed, ParseError> {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "xlsx" | "xlsm" | "xls" => {
            let rows = crate::xlsx::read_rows(bytes)?;
            parse_rows(&rows, opts)
        }
        "dxf" | "dwg" => {
            let d = cadcraft_io::read(bytes, name).map_err(|e| ParseError(format!("can't read {name}: {e}")))?;
            let points = from_drawing(&d, None);
            if points.is_empty() {
                return err(format!("{name} has no POINT objects, 3D polylines or 3D faces to take points from"));
            }
            Ok(Parsed { points, skipped: Vec::new(), columns: ext.to_ascii_uppercase() })
        }
        _ => {
            let text = match std::str::from_utf8(bytes) {
                Ok(t) => std::borrow::Cow::Borrowed(t),
                Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0,
            };
            parse_text(&text, opts)
        }
    }
}

/// The points as delimited text with a header row, for export.
pub fn to_csv(points: &[SurveyPoint], delimiter: char) -> String {
    let d = delimiter;
    let mut s = format!("Punto{d}Norte{d}Este{d}Cota{d}Descripcion\n");
    for p in points {
        let code = p.code.replace(['\n', '\r', d], " ");
        s.push_str(&format!("{}{d}{:.4}{d}{:.4}{d}{:.4}{d}{}\n", p.id.replace(['\n', '\r', d], " "), p.y, p.x, p.z, code));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_header_by_name_whatever_the_column_order() {
        let t = "Punto,Este,Norte,Cota,Descripción\n1,100.5,200.25,10.1,BORDE\n2,101,201,10.4,EJE\n";
        let r = parse_text(t, &ParseOptions::default()).unwrap();
        assert_eq!(r.points.len(), 2);
        assert_eq!((r.points[0].x, r.points[0].y, r.points[0].z), (100.5, 200.25, 10.1));
        assert_eq!(r.points[1].code, "EJE");
    }

    #[test]
    fn headerless_four_columns_are_point_northing_easting_elevation() {
        let t = "1 5000 6000 12.5\n2 5001 6002 12.7\n";
        let r = parse_text(t, &ParseOptions::default()).unwrap();
        assert_eq!((r.points[0].y, r.points[0].x), (5000.0, 6000.0));
        assert_eq!(r.columns, "PYXZ");
    }

    #[test]
    fn semicolons_with_decimal_commas_are_numbers() {
        let t = "1;100,5;200,25;10,1;A\n2;101;201;10,4;B\n";
        let r = parse_text(t, &ParseOptions { columns: Some("PENZD".into()), delimiter: None }).unwrap();
        assert_eq!(r.points[0].x, 100.5);
        assert_eq!(r.points[0].z, 10.1);
    }

    #[test]
    fn bad_rows_are_reported_not_fatal() {
        let t = "1,10,20,5\nx,y,z,w\n3,11,21,6\n";
        let r = parse_text(t, &ParseOptions { columns: Some("PXYZ".into()), delimiter: None }).unwrap();
        assert_eq!(r.points.len(), 2);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].0, 2);
    }

    #[test]
    fn a_spec_without_coordinates_is_refused() {
        assert!(parse_spec("PZD").is_err());
        assert!(parse_spec("PQEZ").is_err());
    }

    #[test]
    fn an_empty_file_is_an_error() {
        assert!(parse_text("\n\n", &ParseOptions::default()).is_err());
    }

    #[test]
    fn csv_round_trips() {
        let pts = vec![SurveyPoint { id: "7".into(), x: 1.5, y: 2.5, z: 3.5, code: "TN".into() }];
        let r = parse_text(&to_csv(&pts, ','), &ParseOptions::default()).unwrap();
        assert_eq!(r.points, pts);
    }
}
