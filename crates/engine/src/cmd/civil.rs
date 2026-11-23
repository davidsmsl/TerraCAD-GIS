//! Civil-ING commands: survey points, TIN, contours, stations and ground profile.
//!
//! The computation is in `cadcraft-civil`; these commands read the drawing's points, call it and
//! draw the result on `CIVIL-*` layers. Each has a JSON form (so agents and the AI assistant can
//! run it) and, from the menu or the command line, asks for what it needs one prompt at a time.

use cadcraft_civil::alignment::{Alignment, flatten_bulges, format_station};
use cadcraft_civil::contour::{break_points, contours};
use cadcraft_civil::earthwork::{Grade, Params as EarthParams, Pvi, earthwork, parse_pvis, section_areas};
use cadcraft_civil::parcel::{Naming, Options, Vertex, angle_sum_error, area_text, bearing, dms, table};
use cadcraft_civil::platform::{Method, Pad, VolumeOptions};
use cadcraft_civil::points::{ParseOptions, SurveyPoint, from_drawing, read_file, to_csv};
use cadcraft_civil::radiation::{AngleUnit, Station, parse_observations, radiate};
use cadcraft_civil::tin::Tin;
use cadcraft_color::Color;
use cadcraft_doc::{EntityKind, Face3d, HAlign, Layer, Point, Polyline3d, Text, VAlign};
use cadcraft_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::helpers::{line, lwpoly};
use super::machines::number;
use super::{CommandSpec, bad, bool_or, f64_or, find_command, str_param, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadcraft_geom::PolyVertex;

const L_POINTS: &str = "CIVIL-PUNTOS";
const L_JOIN: &str = "CIVIL-UNION";
const L_TABLE: &str = "CIVIL-CUADRO";
const L_TABLE_NUM: &str = "CIVIL-VERTICES";
const L_GRADE: &str = "CIVIL-RASANTE";
const L_SECTIONS: &str = "CIVIL-SECCIONES";
const L_EARTH: &str = "CIVIL-TERRACERIAS";
const L_SLOPE: &str = "CIVIL-TALUD";
const L_ZERO: &str = "CIVIL-LINEA-CERO";
const L_TIN: &str = "CIVIL-TIN";
const L_MINOR: &str = "CIVIL-CURVAS-MENORES";
const L_MAJOR: &str = "CIVIL-CURVAS-MAYORES";
const L_STATIONS: &str = "CIVIL-ESTACIONES";
const L_PROFILE: &str = "CIVIL-PERFIL";

/// The largest point file read: bigger than any survey a TIN could triangulate in useful time.
const MAX_FILE: usize = 256 * 1024 * 1024;
/// Labels are drawn for up to this many points; past it only the points are (a label per point
/// of a very large survey makes the drawing unusable).
const MAX_LABELLED: usize = 20_000;
/// The most triangles or contour polylines one command adds.
const MAX_OBJECTS: usize = 400_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("civil.points.import", "Import Points...", run_import)
            .menu(&["Civil-ING", "Import Points..."])
            .params("{path: CSV, TXT, XLSX, DXF or DWG file, columns?: e.g. \"PNEZD\" (P point, N northing, E easting, X, Y, Z elevation, D description, - skip), delimiter?: \",\" | \";\" | \"tab\", layer?, labels?: bool, text_height?, decimals?} → {count, skipped, columns, bounds}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.points.import", "CIVIL.POINTS.IMPORT", &IMPORT_QS)))),
        CommandSpec::new("civil.points.radiate", "Radiations...", run_radiate)
            .menu(&["Civil-ING", "Radiations..."])
            .params("{path (observations: PV ANGH ANGV DINC DHOR DIFH HAP DESC), station: {id?, x, y, z?, hi?, backsight_azimuth?} (or stations: [..]), columns?, angles?: deg|dms|grad, order?, join?: polyline|polyline3d, close?, layer?, labels?, text_height?, decimals?} → {count, skipped, points}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.points.radiate", "CIVIL.POINTS.RADIATE", &RADIATE_QS)))),
        CommandSpec::new("civil.points.join", "Join Points...", run_join)
            .menu(&["Civil-ING", "Join Points..."])
            .params("{source_layer?, kind?: polyline3d|polyline, close?} → {count}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.points.join", "CIVIL.POINTS.JOIN", &JOIN_QS)))),
        CommandSpec::new("civil.parcel", "Construction Table...", run_parcel)
            .menu(&["Civil-ING", "Construction Table..."])
            .params("{handle? (closed polyline; else the selection), start?: 1 (first vertex), direction?: cw|ccw, naming?: numbers|letters, decimals?: 2, seconds_decimals?: 0, at?: [x,y], text_height?, language?: es|en, curves?: bool, label_vertices?: bool} → {area, perimeter, sides: [{from, to, bearing, azimuth, distance, angle, x, y}], angleError}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.parcel", "CIVIL.PARCEL", &PARCEL_QS)))),
        CommandSpec::new("civil.earthwork", "Earthwork & Mass Curve...", run_earthwork)
            .menu(&["Civil-ING", "Earthwork & Mass Curve..."])
            .params("{handle? (axis), pvis (design grade: [[station, elevation, curve?]..] or \"0,100; 120,104,40\"), half_width?: 5, crossfall?: 0.02, interval?: 20, start?: 0, factor?: 1 (fill multiplier), offset_step?: 0.5, at?: [x,y], path? (CSV to write), language?: es|en, text_height?, mass_scale?, source_layer?, max_edge?} → {cut, fill, mass, rows: [{station, ground, grade, depth, cutArea, fillArea, cutVolume, fillVolume, mass}]}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.earthwork", "CIVIL.EARTHWORK", &EARTH_QS)))),
        CommandSpec::new("civil.sections", "Cross Sections...", run_sections)
            .menu(&["Civil-ING", "Cross Sections..."])
            .params("{handle? (axis), pvis? (design grade; without it the areas are 0), half_width?: 5, crossfall?: 0.02, interval?: 20, start?: 0, vscale?: 1, columns?: 4, at?: [x,y], offset_step?, language?, text_height?, source_layer?, max_edge?} → {sections, rows: [{station, grade, cutArea, fillArea}]}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.sections", "CIVIL.SECTIONS", &SECTION_QS)))),
        CommandSpec::new("civil.platform", "Platform & Slopes...", run_platform)
            .menu(&["Civil-ING", "Platform & Slopes..."])
            .params("{handle? (closed polyline outline; else the selection), elevation? (default: mean ground at the outline), cut_slope?: 1 (H per V), fill_slope?: 1.5, bench_height?: 0, berm_width?: 2, method?: prismoidal|average, interval?: 2 (section spacing), step?: 0.5, angle?: 0, along?, ticks?: bool, language?, text_height?, source_layer?, max_edge?} → {elevation, area, cut, fill, net, sections, daylightPoints}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.platform", "CIVIL.PLATFORM", &PLATFORM_QS)))),
        CommandSpec::new("civil.points.export", "Export Points...", run_export)
            .menu(&["Civil-ING", "Export Points..."])
            .params("{path: .csv file to write, delimiter?: \",\" | \";\" | \"tab\", layer?} → {count}")
            .noundo()
            .interactive(|_| Ok(Box::new(Ask::new("civil.points.export", "CIVIL.POINTS.EXPORT", &EXPORT_QS)))),
        CommandSpec::new("civil.tin", "Create TIN", run_tin)
            .menu(&["Civil-ING", "Create TIN"])
            .params("{max_edge?: longest triangle side kept, source_layer?: use only points on this layer, layer?} → {points, triangles}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.tin", "CIVIL.TIN", &TIN_QS)))),
        CommandSpec::new("civil.contours", "Contours...", run_contours)
            .menu(&["Civil-ING", "Contours..."])
            .params("{interval, base?: 0, major_every?: 5, break_layer? (lines on it are break lines), break_step?, minor_color?: 8, major_color?: 30 (ACI; for a new layer), max_edge?, source_layer?, labels?: bool, text_height?} → {minor, major, levels}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.contours", "CIVIL.CONTOURS", &CONTOUR_QS)))),
        CommandSpec::new("civil.stations", "Alignment Stations...", run_stations)
            .menu(&["Civil-ING", "Alignment Stations..."])
            .params("{handle? (a polyline or line; else the selection), interval?: 20, start?: 0, text_height?} → {length, stations}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.stations", "CIVIL.STATIONS", &STATION_QS)))),
        CommandSpec::new("civil.profile", "Longitudinal Profile...", run_profile)
            .menu(&["Civil-ING", "Longitudinal Profile..."])
            .params("{handle? (the axis; else the selection), pvis? (design grade: [[station, elevation, curve?]..] or \"0,100; 120,104,40\"; adds the grade, vertical curves and cut/fill thickness), language?: es|en, interval?: 20 (labels), step?: 10 (sampling), start?: 0, origin?: [x,y], vscale?: 10, max_edge?, source_layer?, text_height?} → {length, datum, table: [{station, label, ground}]}")
            .interactive(|_| Ok(Box::new(Ask::new("civil.profile", "CIVIL.PROFILE", &PROFILE_QS)))),
    ]
}

// ---------- prompts ----------

/// One question of a command's prompt sequence.
struct Q {
    key: &'static str,
    prompt: &'static str,
    /// The answer when the user just presses Enter; `None` makes the question required.
    default: Option<&'static str>,
    number: bool,
}

const IMPORT_QS: [Q; 2] = [
    Q { key: "path", prompt: "Point file (CSV, TXT, XLSX, DXF or DWG)", default: None, number: false },
    Q {
        key: "columns",
        prompt: "Column order (P point, N northing, E easting, Z elevation, D description) or Enter for automatic",
        default: Some(""),
        number: false,
    },
];
const RADIATE_QS: [Q; 6] = [
    Q { key: "path", prompt: "Observation file (PV ANGH ANGV DINC HAP ...)", default: None, number: false },
    Q { key: "station_x", prompt: "Station easting (X)", default: None, number: true },
    Q { key: "station_y", prompt: "Station northing (Y)", default: None, number: true },
    Q { key: "station_z", prompt: "Station elevation", default: Some("0"), number: true },
    Q { key: "station_hi", prompt: "Instrument height", default: Some("0"), number: true },
    Q { key: "station_az", prompt: "Backsight azimuth in degrees", default: Some("0"), number: true },
];
const JOIN_QS: [Q; 1] = [Q { key: "kind", prompt: "Join with polyline3d or polyline", default: Some("polyline3d"), number: false }];
const PARCEL_QS: [Q; 3] = [
    Q { key: "start", prompt: "Number of the first vertex", default: Some("1"), number: true },
    Q { key: "direction", prompt: "Number vertices clockwise (cw) or counterclockwise (ccw)", default: Some("cw"), number: false },
    Q { key: "decimals", prompt: "Decimals for distances and coordinates", default: Some("2"), number: true },
];
const EARTH_QS: [Q; 4] = [
    Q { key: "pvis", prompt: "Design grade: station,elevation[,curve length]; next point; ...", default: None, number: false },
    Q { key: "half_width", prompt: "Half width of the template each side of the axis", default: Some("5"), number: true },
    Q { key: "interval", prompt: "Station interval", default: Some("20"), number: true },
    Q { key: "factor", prompt: "Fill factor (shrinkage or swell)", default: Some("1"), number: true },
];
const SECTION_QS: [Q; 3] = [
    Q { key: "pvis", prompt: "Design grade: station,elevation[,curve length]; next point; ... (Enter for none)", default: Some(""), number: false },
    Q { key: "half_width", prompt: "Half width of the template each side of the axis", default: Some("5"), number: true },
    Q { key: "interval", prompt: "Station interval", default: Some("20"), number: true },
];
const PLATFORM_QS: [Q; 5] = [
    Q { key: "elevation", prompt: "Platform elevation (Enter for the mean ground level)", default: Some(""), number: true },
    Q { key: "cut_slope", prompt: "Cut slope (horizontal per 1 vertical)", default: Some("1"), number: true },
    Q { key: "fill_slope", prompt: "Fill slope (horizontal per 1 vertical)", default: Some("1.5"), number: true },
    Q { key: "bench_height", prompt: "Height between berms (0 for none)", default: Some("0"), number: true },
    Q { key: "berm_width", prompt: "Berm width", default: Some("2"), number: true },
];
const EXPORT_QS: [Q; 1] = [Q { key: "path", prompt: "CSV file to write", default: None, number: false }];
const TIN_QS: [Q; 1] = [Q { key: "max_edge", prompt: "Longest triangle side to keep (0 for no limit)", default: Some("0"), number: true }];
const CONTOUR_QS: [Q; 2] = [
    Q { key: "interval", prompt: "Contour interval", default: Some("1"), number: true },
    Q { key: "major_every", prompt: "Major contour every how many contours", default: Some("5"), number: true },
];
const STATION_QS: [Q; 2] = [
    Q { key: "interval", prompt: "Station interval", default: Some("20"), number: true },
    Q { key: "text_height", prompt: "Text height", default: Some("2"), number: true },
];
const PROFILE_QS: [Q; 2] = [
    Q { key: "interval", prompt: "Station interval for labels", default: Some("20"), number: true },
    Q { key: "vscale", prompt: "Vertical exaggeration", default: Some("10"), number: true },
];

/// Asks each question in turn and then runs the command with the answers. The axis commands use
/// the current selection, so select the polyline first.
struct Ask {
    cmd: &'static str,
    name: &'static str,
    qs: &'static [Q],
    at: usize,
    params: serde_json::Map<String, Value>,
}

impl Ask {
    fn new(cmd: &'static str, name: &'static str, qs: &'static [Q]) -> Self {
        Ask { cmd, name, qs, at: 0, params: serde_json::Map::new() }
    }

    fn finish(&mut self, s: &mut Session) -> Result<Step> {
        let Some(spec) = find_command(self.cmd) else { return Ok(Step::Cancel) };
        match (spec.run)(s, &Value::Object(std::mem::take(&mut self.params))) {
            Ok(v) => s.echo(v.get("message").and_then(Value::as_str).unwrap_or("Done.").to_string()),
            Err(EngineError::BadParams { msg, .. }) => s.echo(msg),
            Err(e) => s.echo(e.to_string()),
        }
        Ok(Step::Done)
    }
}

impl Interactive for Ask {
    fn name(&self) -> &'static str {
        self.name
    }

    fn prompt(&self, _: &Session) -> Prompt {
        match self.qs.get(self.at) {
            Some(q) => {
                let p = Prompt::new(q.prompt, if q.number { Accept::NUMBER } else { Accept::TEXT });
                match q.default.filter(|d| !d.is_empty()) {
                    Some(d) => p.default(d),
                    None => p,
                }
            }
            None => Prompt::new("Done", Accept::TEXT),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(q) = self.qs.get(self.at) else { return self.finish(s) };
        let answer = match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => match q.default {
                Some(d) => d.to_string(),
                None => return Ok(Step::Cancel),
            },
            Input::Text(t) => t.trim().trim_matches('"').to_string(),
            _ => return Ok(Step::Continue),
        };
        if q.number {
            let Some(n) = number(&answer) else {
                s.echo("Requires a number.");
                return Ok(Step::Continue);
            };
            // A max edge of 0 means no limit: leave it out rather than send a zero.
            if !(q.key == "max_edge" && n == 0.0) {
                self.params.insert(q.key.into(), json!(n));
            }
        } else if !answer.is_empty() {
            self.params.insert(q.key.into(), json!(answer));
        }
        self.at += 1;
        if self.at >= self.qs.len() { self.finish(s) } else { Ok(Step::Continue) }
    }
}

// ---------- helpers ----------

fn civil_err(cmd: &str, e: impl std::fmt::Display) -> EngineError {
    bad(cmd, e.to_string())
}

/// The layer's colour when it is made, so the first run is already readable.
fn ensure_layer(s: &mut Session, name: &str, aci: i16) -> Result<()> {
    let d = s.doc_mut()?;
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
        let mut l = Layer::new(name);
        l.color = Color::from_aci(aci);
        d.layers.push(l);
    }
    Ok(())
}

/// Run `f` with `layer` as the current layer, then put the old one back, so everything `f` adds
/// lands on it without touching each object.
pub(super) fn on_layer<T>(s: &mut Session, layer: &str, aci: i16, f: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    ensure_layer(s, layer, aci)?;
    let old = s.doc()?.header.str("CLAYER", "0");
    s.doc_mut()?.header.set_str("CLAYER", layer);
    let r = f(s);
    if let Ok(d) = s.doc_mut() {
        d.header.set_str("CLAYER", &old);
    }
    r
}

pub(super) fn text_at(s: &mut Session, at: Vec2, value: &str, height: f64, rotation: f64) -> Result<()> {
    let style = s.doc().map(|d| d.header.str("TEXTSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into());
    s.add_entity(EntityKind::Text(Text {
        insert: Vec3::new(at.x, at.y, 0.0),
        align_pt: None,
        height,
        value: value.into(),
        rotation,
        width_factor: 1.0,
        oblique: 0.0,
        style,
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    }))?;
    Ok(())
}

fn delimiter(p: &Value) -> Option<char> {
    match str_param(p, "delimiter")? {
        "tab" | "\\t" | "\t" => Some('\t'),
        d => d.chars().next(),
    }
}

/// The points of the drawing, optionally of one layer.
fn drawing_points(s: &Session, p: &Value, cmd: &str) -> Result<Vec<SurveyPoint>> {
    let pts = from_drawing(s.doc()?, str_param(p, "source_layer"));
    if pts.len() < 3 {
        return Err(bad(cmd, "the drawing needs at least 3 POINT objects with elevation (import points first: CIVIL.POINTS.IMPORT)"));
    }
    Ok(pts)
}

fn max_edge(p: &Value) -> Option<f64> {
    Some(f64_or(p, "max_edge", 0.0)).filter(|e| *e > 0.0)
}

fn positive(cmd: &str, p: &Value, key: &str, default: f64) -> Result<f64> {
    let v = f64_or(p, key, default);
    if v > 0.0 { Ok(v) } else { Err(bad(cmd, format!("`{key}` must be a number above zero"))) }
}

fn build_tin(s: &Session, p: &Value, cmd: &str) -> Result<Tin> {
    let pts = drawing_points(s, p, cmd)?;
    Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(cmd, e))
}

/// A readable default text height: 1/400 of the survey's larger side, at least a hundredth.
fn auto_height(pts: &[SurveyPoint]) -> f64 {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in pts {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    (((hi[0] - lo[0]).max(hi[1] - lo[1])) / 400.0).max(0.01)
}

// ---------- commands ----------

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.points.import";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?;
    let bytes = read_input(path)?;
    let opts = ParseOptions { columns: str_param(p, "columns").filter(|c| !c.trim().is_empty()).map(str::to_string), delimiter: delimiter(p) };
    let parsed = read_file(&bytes, path, &opts).map_err(|e| civil_err(CMD, e))?;
    let mut parsed = parsed;
    if str_param(p, "order").is_some_and(|o| o.eq_ignore_ascii_case("number")) || bool_or(p, "order", false) {
        order_by_number(&mut parsed.points);
    }
    let (labels, pts) = (draw_survey_points(s, p, &parsed.points)?, &parsed.points);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in pts {
        lo = [lo[0].min(q.x), lo[1].min(q.y)];
        hi = [hi[0].max(q.x), hi[1].max(q.y)];
    }
    let mut message = format!("Imported {} points (columns {}); use ZOOM Extents to see them.", pts.len(), parsed.columns);
    if !parsed.skipped.is_empty() {
        message.push_str(&format!(" {} rows were not points (first: row {}: {}).", parsed.skipped.len(), parsed.skipped[0].0, parsed.skipped[0].1));
    }
    if bool_or(p, "labels", true) && !labels {
        message.push_str(&format!(" Labels were not drawn: more than {MAX_LABELLED} points."));
    }
    let skipped: Vec<Value> = parsed.skipped.iter().take(20).map(|(l, w)| json!({ "row": l, "reason": w })).collect();
    Ok(
        json!({ "count": pts.len(), "skipped": parsed.skipped.len(), "skippedRows": skipped, "columns": parsed.columns, "bounds": [lo[0], lo[1], hi[0], hi[1]], "message": message }),
    )
}

/// Sort points by their number: numerically when both are numbers, otherwise as text.
fn order_by_number(pts: &mut [SurveyPoint]) {
    pts.sort_by(|a, b| match (a.id.trim().parse::<f64>(), b.id.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        _ => a.id.cmp(&b.id),
    });
}

/// Draw `pts` as POINT objects with their number and elevation labels (`layer`, `labels`,
/// `text_height`, `decimals`), and join them with a polyline when `join` asks for one. Returns
/// whether the labels were drawn.
fn draw_survey_points(s: &mut Session, p: &Value, pts: &[SurveyPoint]) -> Result<bool> {
    let layer = str_param(p, "layer").filter(|l| !l.trim().is_empty()).unwrap_or(L_POINTS).to_string();
    let labels = bool_or(p, "labels", true) && pts.len() <= MAX_LABELLED;
    let height = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => auto_height(pts),
    };
    let decimals = f64_or(p, "decimals", 2.0).clamp(0.0, 6.0) as usize;
    on_layer(s, &layer, 7, |s| {
        for q in pts {
            s.add_entity(EntityKind::Point(Point { p: Vec3::new(q.x, q.y, q.z), angle: 0.0 }))?;
        }
        Ok(())
    })?;
    if labels {
        on_layer(s, &format!("{layer}-NUM"), 3, |s| {
            for q in pts {
                text_at(s, Vec2::new(q.x + height * 0.5, q.y + height * 0.5), &q.id, height, 0.0)?;
            }
            Ok(())
        })?;
        on_layer(s, &format!("{layer}-COTA"), 2, |s| {
            for q in pts {
                text_at(s, Vec2::new(q.x + height * 0.5, q.y - height * 1.5), &format!("{:.*}", decimals, q.z), height, 0.0)?;
            }
            Ok(())
        })?;
    }
    match str_param(p, "join").map(str::to_lowercase).as_deref() {
        Some("polyline") | Some("pline") | Some("2d") => join_points(s, pts, false, bool_or(p, "close", false))?,
        Some("polyline3d") | Some("3d") => join_points(s, pts, true, bool_or(p, "close", false))?,
        _ => {}
    }
    Ok(labels)
}

fn join_points(s: &mut Session, pts: &[SurveyPoint], three_d: bool, close: bool) -> Result<()> {
    if pts.len() < 2 {
        return Ok(());
    }
    on_layer(s, L_JOIN, 5, |s| {
        let kind = if three_d {
            EntityKind::Polyline3d(Polyline3d { points: pts.iter().map(|q| Vec3::new(q.x, q.y, q.z)).collect(), closed: close })
        } else {
            lwpoly(pts.iter().map(|q| PolyVertex::new(Vec2::new(q.x, q.y))).collect(), close)
        };
        s.add_entity(kind).map(|_| ())
    })
}

fn run_join(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.points.join";
    let pts = from_drawing(s.doc()?, str_param(p, "source_layer"));
    if pts.len() < 2 {
        return Err(bad(CMD, "the drawing needs at least 2 POINT objects to join"));
    }
    let three_d = !str_param(p, "kind").is_some_and(|k| k.eq_ignore_ascii_case("polyline"));
    join_points(s, &pts, three_d, bool_or(p, "close", false))?;
    Ok(json!({ "count": pts.len(), "message": format!("Joined {} points with a {}.", pts.len(), if three_d { "3D polyline" } else { "polyline" }) }))
}

fn stations_of(p: &Value, cmd: &str) -> Result<Vec<Station>> {
    let list: Vec<&Value> = match p.get("stations") {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => p.get("station").into_iter().collect(),
    };
    // The prompts give the station as flat `station_x`, `station_y`... answers.
    let flat = json!({ "id": p.get("station_id"), "x": p.get("station_x"), "y": p.get("station_y"), "z": p.get("station_z"), "hi": p.get("station_hi"), "az": p.get("station_az") });
    let list = if list.is_empty() && p.get("station_x").is_some() { vec![&flat] } else { list };
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let mut out = Vec::new();
    for v in list {
        let (Some(x), Some(y)) = (f(v, "x"), f(v, "y")) else { return Err(bad(cmd, "each station needs `x` and `y`")) };
        let id = v.get("id").and_then(Value::as_str).unwrap_or("EST").to_string();
        out.push(Station {
            id,
            x,
            y,
            z: f(v, "z").unwrap_or(0.0),
            hi: f(v, "hi").unwrap_or(0.0),
            backsight_azimuth: f(v, "backsight_azimuth").or_else(|| f(v, "az")).unwrap_or(0.0),
        });
    }
    if out.is_empty() {
        return Err(bad(cmd, "give the `station` (or `stations`): {id, x, y, z, hi, backsight_azimuth}"));
    }
    Ok(out)
}

fn run_radiate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.points.radiate";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?;
    let stations = stations_of(p, CMD)?;
    let unit = match str_param(p, "angles") {
        None => AngleUnit::Degrees,
        Some(u) => AngleUnit::parse(u).ok_or_else(|| bad(CMD, "`angles` is deg, dms or grad"))?,
    };
    let bytes = read_input(path)?;
    let text = cadcraft_civil::points::decode_text(&bytes);
    let (obs, mut skipped) = parse_observations(&text, str_param(p, "columns"), unit).map_err(|e| civil_err(CMD, e))?;
    let mut r = radiate(&stations, &obs);
    skipped.append(&mut r.skipped);
    skipped.sort_by_key(|(row, _)| *row);
    if bool_or(p, "order", false) {
        order_by_number(&mut r.points);
    }
    if r.points.is_empty() {
        return Err(bad(CMD, format!("no point could be computed{}", skipped.first().map(|(l, w)| format!(" (row {l}: {w})")).unwrap_or_default())));
    }
    let labels = draw_survey_points(s, p, &r.points)?;
    let mut message = format!("Computed {} points from {} station(s).", r.points.len(), stations.len());
    if !skipped.is_empty() {
        message.push_str(&format!(" {} rows were left out (first: row {}: {}).", skipped.len(), skipped[0].0, skipped[0].1));
    }
    let rows: Vec<Value> = skipped.iter().take(20).map(|(l, w)| json!({ "row": l, "reason": w })).collect();
    let pts: Vec<Value> = r.points.iter().take(50).map(|q| json!({ "id": q.id, "x": q.x, "y": q.y, "z": q.z })).collect();
    Ok(json!({ "count": r.points.len(), "labels": labels, "skipped": skipped.len(), "skippedRows": rows, "points": pts, "message": message }))
}

/// The first selected (or named) closed polyline as a boundary ring.
fn boundary_of(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Vertex>> {
    for h in targets(s, p)? {
        let Some(e) = s.doc()?.model.get(h) else { continue };
        let ring: Vec<Vertex> = match &e.kind {
            EntityKind::LwPolyline(pl) if pl.vertices.len() >= 3 => {
                pl.vertices.iter().map(|v| Vertex { x: v.p.x, y: v.p.y, bulge: v.bulge }).collect()
            }
            EntityKind::Polyline3d(pl) if pl.points.len() >= 3 => pl.points.iter().map(|v| Vertex { x: v.x, y: v.y, bulge: 0.0 }).collect(),
            _ => continue,
        };
        return Ok(ring);
    }
    Err(bad(cmd, "select the boundary (a closed polyline) first, or give its `handle`"))
}

fn run_parcel(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.parcel";
    let ring = boundary_of(s, p, CMD)?;
    let es = !str_param(p, "language").is_some_and(|l| l.eq_ignore_ascii_case("en"));
    let tr = |a: &'static str, b: &'static str| if es { a } else { b };
    let opts = Options {
        start: (f64_or(p, "start", 1.0).round().max(1.0) as usize) - 1,
        clockwise: !str_param(p, "direction")
            .is_some_and(|d| d.to_lowercase().starts_with("ccw") || d.to_lowercase().starts_with("anti") || d.to_lowercase().starts_with("counter")),
        naming: if str_param(p, "naming").is_some_and(|n| n.to_lowercase().starts_with("let")) { Naming::Letters } else { Naming::Numbers },
    };
    let t = table(&ring, &opts).map_err(|e| civil_err(CMD, e))?;
    let decimals = f64_or(p, "decimals", 2.0).clamp(0.0, 6.0) as usize;
    let sec = f64_or(p, "seconds_decimals", 0.0).clamp(0.0, 3.0) as usize;
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for v in &ring {
        lo = [lo[0].min(v.x), lo[1].min(v.y)];
        hi = [hi[0].max(v.x), hi[1].max(v.y)];
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6);
    let h = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => (span / 120.0).max(0.01),
    };
    let at = match p.get("at").and_then(Value::as_array) {
        Some(a) if a.len() >= 2 => Vec2::new(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)),
        _ => Vec2::new(hi[0] + span * 0.08, hi[1]),
    };
    // Rows of text, each a list of cells.
    let head: Vec<&str> = vec![
        tr("LADO", "SIDE"),
        tr("RUMBO", "BEARING"),
        tr("DISTANCIA", "DISTANCE"),
        tr("ANG. INTERIOR", "INT. ANGLE"),
        tr("VERT.", "VERT."),
        "Y",
        "X",
    ];
    let mut rows: Vec<Vec<String>> = vec![head.iter().map(|c| c.to_string()).collect()];
    for sd in &t.sides {
        rows.push(vec![
            format!("{}-{}", sd.from, sd.to),
            bearing(sd.azimuth, sec),
            format!("{:.*}", decimals, sd.distance),
            dms(sd.angle, sec),
            sd.to.clone(),
            format!("{:.*}", decimals, sd.y),
            format!("{:.*}", decimals, sd.x),
        ]);
    }
    let widths = [9.0, 17.0, 11.0, 17.0, 6.0, 15.0, 15.0];
    let row_h = h * 2.0;
    let total_w: f64 = widths.iter().sum::<f64>() * h;
    let top = at.y;
    let layer_cfg = (L_TABLE, 7);
    on_layer(s, layer_cfg.0, layer_cfg.1, |s| {
        text_at(s, Vec2::new(at.x, top + row_h * 0.6), tr("CUADRO DE CONSTRUCCIÓN", "CONSTRUCTION TABLE"), h * 1.3, 0.0)?;
        let n = rows.len();
        for i in 0..=n {
            let y = top - row_h * i as f64;
            s.add_entity(line(Vec2::new(at.x, y), Vec2::new(at.x + total_w, y)))?;
        }
        let mut x = at.x;
        for (c, w) in widths.iter().enumerate() {
            s.add_entity(line(Vec2::new(x, top), Vec2::new(x, top - row_h * n as f64)))?;
            for (r, row) in rows.iter().enumerate() {
                text_at(s, Vec2::new(x + h * 0.4, top - row_h * (r as f64 + 1.0) + h * 0.6), &row[c], h, 0.0)?;
            }
            x += w * h;
        }
        s.add_entity(line(Vec2::new(x, top), Vec2::new(x, top - row_h * n as f64)))?;
        let foot = top - row_h * n as f64 - row_h;
        text_at(
            s,
            Vec2::new(at.x, foot),
            &format!(
                "{}: {}   {}: {:.*} m",
                tr("SUPERFICIE", "AREA"),
                area_text(t.area, decimals),
                tr("PERÍMETRO", "PERIMETER"),
                decimals,
                t.perimeter
            ),
            h,
            0.0,
        )?;
        if !t.curves.is_empty() && bool_or(p, "curves", true) {
            let top2 = foot - row_h * 2.0;
            let head2 = [tr("CURVA", "CURVE"), "R", "Δ", tr("LONGITUD", "LENGTH"), "T", tr("CUERDA", "CHORD"), tr("RUMBO CUERDA", "CHORD BEARING")];
            let w2 = [9.0, 11.0, 17.0, 11.0, 11.0, 11.0, 20.0];
            text_at(s, Vec2::new(at.x, top2 + row_h * 0.6), tr("CUADRO DE CURVAS", "CURVE TABLE"), h * 1.3, 0.0)?;
            let mut rows2: Vec<Vec<String>> = vec![head2.iter().map(|c| c.to_string()).collect()];
            for c in &t.curves {
                rows2.push(vec![
                    format!("{}-{}", c.from, c.to),
                    format!("{:.*}", decimals, c.radius),
                    format!("{} {}", dms(c.delta, sec), if c.right { tr("D", "R") } else { tr("I", "L") }),
                    format!("{:.*}", decimals, c.length),
                    format!("{:.*}", decimals, c.tangent),
                    format!("{:.*}", decimals, c.chord),
                    bearing(c.chord_azimuth, sec),
                ]);
            }
            let total2: f64 = w2.iter().sum::<f64>() * h;
            for i in 0..=rows2.len() {
                let y = top2 - row_h * i as f64;
                s.add_entity(line(Vec2::new(at.x, y), Vec2::new(at.x + total2, y)))?;
            }
            let mut x = at.x;
            for (ci, w) in w2.iter().enumerate() {
                s.add_entity(line(Vec2::new(x, top2), Vec2::new(x, top2 - row_h * rows2.len() as f64)))?;
                for (r, row) in rows2.iter().enumerate() {
                    text_at(s, Vec2::new(x + h * 0.4, top2 - row_h * (r as f64 + 1.0) + h * 0.6), &row[ci], h, 0.0)?;
                }
                x += w * h;
            }
            s.add_entity(line(Vec2::new(x, top2), Vec2::new(x, top2 - row_h * rows2.len() as f64)))?;
        }
        Ok(())
    })?;
    if bool_or(p, "label_vertices", true) {
        let mut ordered: Vec<(String, f64, f64)> = t.sides.iter().map(|sd| (sd.to.clone(), sd.x, sd.y)).collect();
        ordered.dedup_by(|a, b| a.0 == b.0);
        on_layer(s, L_TABLE_NUM, 3, |s| {
            for (id, x, y) in &ordered {
                text_at(s, Vec2::new(x + h * 0.6, y + h * 0.6), id, h * 1.2, 0.0)?;
            }
            Ok(())
        })?;
    }
    let sides: Vec<Value> = t
        .sides
        .iter()
        .map(|sd| json!({ "from": sd.from, "to": sd.to, "bearing": bearing(sd.azimuth, sec), "azimuth": sd.azimuth, "distance": sd.distance, "angle": sd.angle, "x": sd.x, "y": sd.y }))
        .collect();
    Ok(json!({
        "area": t.area, "perimeter": t.perimeter, "vertices": t.sides.len(), "curves": t.curves.len(), "angleError": angle_sum_error(&t), "sides": sides,
        "message": format!("Construction table: {} vertices, area {}, perimeter {:.*} m.", t.sides.len(), area_text(t.area, decimals), decimals, t.perimeter)
    }))
}

/// Where a table or sheet goes when `at` is not given: right of the survey, at its top.
fn sheet_origin(p: &Value, pts: &[SurveyPoint], shift: f64) -> Vec2 {
    if let Some(o) = crate::cmd::point_param(p, "at") {
        return o;
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in pts {
        lo = [lo[0].min(q.x), lo[1].min(q.y)];
        hi = [hi[0].max(q.x), hi[1].max(q.y)];
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6);
    Vec2::new(hi[0] + span * (0.1 + shift), hi[1])
}

fn grid(s: &mut Session, at: Vec2, row_h: f64, widths: &[f64], rows: &[Vec<String>], h: f64) -> Result<()> {
    let total: f64 = widths.iter().sum();
    for i in 0..=rows.len() {
        let y = at.y - row_h * i as f64;
        s.add_entity(line(Vec2::new(at.x, y), Vec2::new(at.x + total, y)))?;
    }
    let mut x = at.x;
    for (c, w) in widths.iter().enumerate() {
        s.add_entity(line(Vec2::new(x, at.y), Vec2::new(x, at.y - row_h * rows.len() as f64)))?;
        for (r, row) in rows.iter().enumerate() {
            text_at(s, Vec2::new(x + h * 0.4, at.y - row_h * (r as f64 + 1.0) + h * 0.6), &row[c], h, 0.0)?;
        }
        x += w;
    }
    s.add_entity(line(Vec2::new(x, at.y), Vec2::new(x, at.y - row_h * rows.len() as f64)))?;
    Ok(())
}

fn run_earthwork(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.earthwork";
    let axis = axis_of(s, p, CMD)?;
    let grade = grade_param(p, CMD)?.ok_or_else(|| bad(CMD, "`pvis` is required: the design grade as \"station,elevation[,curve]; ...\""))?;
    let start = f64_or(p, "start", 0.0);
    let ep = earth_params(p, CMD, start)?;
    let pts = drawing_points(s, p, CMD)?;
    let tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
    let (rows, left_out) = earthwork(&axis, &tin, &grade, &ep).map_err(|e| civil_err(CMD, e))?;
    let es = lang_es(p);
    let tr = |a: &'static str, b: &'static str| if es { a } else { b };
    let dec = f64_or(p, "decimals", 2.0).clamp(0.0, 4.0) as usize;
    let h = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => auto_height(&pts),
    };
    let (cut_total, fill_total) = rows.iter().fold((0.0, 0.0), |(c, f), r| (c + r.cut_volume, f + r.fill_volume));
    let final_mass = rows.last().map_or(0.0, |r| r.mass);
    let at = sheet_origin(p, &pts, 0.0);
    let mut table: Vec<Vec<String>> = vec![
        [
            tr("ESTACIÓN", "STATION"),
            tr("TERRENO", "GROUND"),
            tr("RASANTE", "GRADE"),
            tr("ESPESOR", "DEPTH"),
            tr("ÁREA CORTE", "CUT AREA"),
            tr("ÁREA RELLENO", "FILL AREA"),
            tr("VOL. CORTE", "CUT VOL."),
            tr("VOL. RELLENO", "FILL VOL."),
            tr("ORDENADA", "MASS"),
        ]
        .iter()
        .map(|c| c.to_string())
        .collect(),
    ];
    for r in &rows {
        table.push(vec![
            format_station(r.station),
            format!("{:.*}", dec, r.ground),
            format!("{:.*}", dec, r.grade),
            format!(
                "{} {:.*}",
                if r.depth >= 0.0 {
                    "C"
                } else if es {
                    "T"
                } else {
                    "F"
                },
                dec,
                r.depth.abs()
            ),
            format!("{:.*}", dec, r.cut_area),
            format!("{:.*}", dec, r.fill_area),
            format!("{:.*}", dec, r.cut_volume),
            format!("{:.*}", dec, r.fill_volume),
            format!("{:.*}", dec, r.mass),
        ]);
    }
    let widths: Vec<f64> = [13.0, 10.0, 10.0, 10.0, 12.0, 13.0, 12.0, 13.0, 12.0].iter().map(|w| w * h).collect();
    let row_h = h * 2.0;
    on_layer(s, L_EARTH, 6, |s| {
        text_at(s, Vec2::new(at.x, at.y + row_h * 0.6), tr("TABLA DE TERRACERÍAS", "EARTHWORK TABLE"), h * 1.3, 0.0)?;
        grid(s, at, row_h, &widths, &table, h)?;
        let foot = at.y - row_h * (table.len() as f64 + 1.0);
        text_at(
            s,
            Vec2::new(at.x, foot),
            &format!(
                "{}: {:.*}   {}: {:.*}   {}: {:.*}",
                tr("CORTE", "CUT"),
                dec,
                cut_total,
                tr("RELLENO", "FILL"),
                dec,
                fill_total,
                tr("ORDENADA FINAL", "FINAL MASS"),
                dec,
                final_mass
            ),
            h,
            0.0,
        )?;
        // The mass curve below the table: stations along X, ordinates up.
        let top = foot - row_h * 3.0;
        let (mlo, mhi) = rows.iter().fold((0f64, 0f64), |(l, u), r| (l.min(r.mass), u.max(r.mass)));
        let range = (mhi - mlo).max(1e-9);
        let height = widths.iter().sum::<f64>() * 0.4;
        let vs = f64_or(p, "mass_scale", 0.0);
        let vs = if vs > 0.0 { vs } else { height / range };
        let length = rows.last().map_or(0.0, |r| r.station) - rows[0].station;
        let hs = widths.iter().sum::<f64>() / length.max(1e-9);
        let base = Vec2::new(at.x, top - mhi * vs - row_h);
        text_at(s, Vec2::new(at.x, base.y + mhi * vs + row_h * 0.6), tr("CURVA MASA", "MASS CURVE"), h * 1.3, 0.0)?;
        s.add_entity(line(Vec2::new(at.x, base.y), Vec2::new(at.x + length * hs, base.y)))?;
        let vs_pts: Vec<PolyVertex> =
            rows.iter().map(|r| PolyVertex::new(Vec2::new(at.x + (r.station - rows[0].station) * hs, base.y + r.mass * vs))).collect();
        for (r, v) in rows.iter().zip(&vs_pts) {
            s.add_entity(line(Vec2::new(v.p.x, base.y), v.p))?;
            text_at(s, Vec2::new(v.p.x - h * 0.5, base.y - h * 7.0), &format_station(r.station), h, std::f64::consts::FRAC_PI_2)?;
            text_at(s, Vec2::new(v.p.x + h * 0.5, v.p.y + h * 0.5), &format!("{:.0}", r.mass), h, 0.0)?;
        }
        s.add_entity(lwpoly(vs_pts, false))?;
        Ok(())
    })?;
    if let Some(path) = str_param(p, "path").filter(|x| !x.trim().is_empty()) {
        let mut csv = String::from("station,ground,grade,depth,cut_area,fill_area,cut_volume,fill_volume,mass\n");
        for r in &rows {
            csv.push_str(&format!(
                "{:.3},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}\n",
                r.station, r.ground, r.grade, r.depth, r.cut_area, r.fill_area, r.cut_volume, r.fill_volume, r.mass
            ));
        }
        write_output(path, csv.as_bytes())?;
    }
    let rows_json: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "station": r.station, "label": format_station(r.station), "ground": r.ground, "grade": r.grade, "depth": r.depth, "cutArea": r.cut_area, "fillArea": r.fill_area, "cutVolume": r.cut_volume, "fillVolume": r.fill_volume, "mass": r.mass }))
        .collect();
    let mut message = format!(
        "Earthwork over {} stations: cut {:.0} m³, fill {:.0} m³ (factor {}), final mass ordinate {:.0}.",
        rows.len(),
        cut_total,
        fill_total,
        ep.factor,
        final_mass
    );
    if left_out > 0 {
        message.push_str(&format!(" {left_out} stations were left out: no ground or grade there."));
    }
    Ok(
        json!({ "cut": cut_total, "fill": fill_total, "mass": final_mass, "stations": rows.len(), "leftOut": left_out, "rows": rows_json, "message": message }),
    )
}

fn run_sections(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.sections";
    let axis = axis_of(s, p, CMD)?;
    let grade = grade_param(p, CMD)?;
    let start = f64_or(p, "start", 0.0);
    let ep = earth_params(p, CMD, start)?;
    let pts = drawing_points(s, p, CMD)?;
    let tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
    let es = lang_es(p);
    let h = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => (ep.half_width / 25.0).max(0.01),
    };
    let vs = positive(CMD, p, "vscale", 1.0)?;
    let cols = f64_or(p, "columns", 4.0).clamp(1.0, 12.0) as usize;
    let marks = axis.stations(ep.interval, start).map_err(|e| civil_err(CMD, e))?;
    if marks.len() > 2000 {
        return Err(bad(CMD, format!("{} sections: use a larger interval", marks.len())));
    }
    struct Sec {
        station: f64,
        design: f64,
        ground: Vec<Option<f64>>,
        cut: f64,
        fill: f64,
    }
    let n = ((2.0 * ep.half_width / ep.step).ceil() as usize).clamp(1, 4000);
    let mut secs: Vec<Sec> = Vec::new();
    for m in &marks {
        let (nx, ny) = (-m.angle.sin(), m.angle.cos());
        let g_at = |o: f64| tin.z_at(m.x + nx * o, m.y + ny * o);
        let Some(z0) = g_at(0.0) else { continue };
        let design = match &grade {
            Some(g) => match g.elevation(m.station) {
                Some(z) => z,
                None => continue,
            },
            None => z0,
        };
        let (cut, fill) = if grade.is_some() { section_areas(&g_at, design, ep.half_width, ep.crossfall, ep.step) } else { (0.0, 0.0) };
        let ground = (0..=n).map(|i| g_at(-ep.half_width + 2.0 * ep.half_width * i as f64 / n as f64)).collect();
        secs.push(Sec { station: m.station, design, ground, cut, fill });
    }
    if secs.is_empty() {
        return Err(bad(CMD, "no station has ground and grade: check that the axis lies inside the survey and the grade covers it"));
    }
    let dev = secs.iter().flat_map(|c| c.ground.iter().flatten().map(|z| (z - c.design).abs())).fold(1.0, f64::max);
    let (cw, ch) = (2.0 * ep.half_width + 8.0 * h, 2.0 * dev * vs + 14.0 * h);
    let at = sheet_origin(p, &pts, 0.9);
    on_layer(s, L_SECTIONS, 4, |s| {
        for (i, c) in secs.iter().enumerate() {
            let (col, row) = (i % cols, i / cols);
            let cx = at.x + cw * col as f64 + cw / 2.0;
            let cy = at.y - ch * row as f64 - ch / 2.0 - 4.0 * h;
            let pos = |o: f64, z: f64| Vec2::new(cx + o, cy + (z - c.design) * vs);
            text_at(
                s,
                Vec2::new(cx - ep.half_width, cy + dev * vs + 3.0 * h),
                &format!("{}  {:.2}", format_station(c.station), c.design),
                h * 1.2,
                0.0,
            )?;
            let mut run: Vec<PolyVertex> = Vec::new();
            for (k, z) in c.ground.iter().enumerate() {
                match z {
                    Some(z) => run.push(PolyVertex::new(pos(-ep.half_width + 2.0 * ep.half_width * k as f64 / n as f64, *z))),
                    None => flush(s, &mut run)?,
                }
            }
            flush(s, &mut run)?;
            if grade.is_some() {
                on_layer(s, L_GRADE, 1, |s| {
                    let edge = c.design - ep.crossfall * ep.half_width;
                    s.add_entity(lwpoly(
                        vec![
                            PolyVertex::new(pos(-ep.half_width, edge)),
                            PolyVertex::new(pos(0.0, c.design)),
                            PolyVertex::new(pos(ep.half_width, edge)),
                        ],
                        false,
                    ))
                    .map(|_| ())
                })?;
                text_at(
                    s,
                    Vec2::new(cx - ep.half_width, cy - dev * vs - 2.0 * h),
                    &format!("{} {:.2}  {} {:.2}", "C", c.cut, if es { "T" } else { "F" }, c.fill),
                    h,
                    0.0,
                )?;
            }
            s.add_entity(line(pos(0.0, c.design - dev), pos(0.0, c.design + dev)))?;
        }
        Ok(())
    })?;
    let rows: Vec<Value> = secs
        .iter()
        .map(|c| json!({ "station": c.station, "label": format_station(c.station), "grade": c.design, "cutArea": c.cut, "fillArea": c.fill }))
        .collect();
    Ok(
        json!({ "sections": secs.len(), "rows": rows, "message": format!("{} cross sections of ±{} m drawn in {cols} columns.", secs.len(), ep.half_width) }),
    )
}

fn run_platform(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.platform";
    let hs = targets(s, p)?;
    let ring: Vec<[f64; 2]> = {
        let d = s.doc()?;
        let mut found = None;
        for h in hs {
            let Some(e) = d.model.get(h) else { continue };
            let r: Vec<[f64; 2]> = match &e.kind {
                EntityKind::LwPolyline(pl) if pl.vertices.len() >= 3 => {
                    flatten_bulges(&pl.vertices.iter().map(|v| (v.p.x, v.p.y, v.bulge)).collect::<Vec<_>>(), true)
                }
                EntityKind::Polyline3d(pl) if pl.points.len() >= 3 => pl.points.iter().map(|v| [v.x, v.y]).collect(),
                _ => continue,
            };
            found = Some(r);
            break;
        }
        found.ok_or_else(|| bad(CMD, "select the platform outline (a closed polyline) first, or give its `handle`"))?
    };
    let pts = drawing_points(s, p, CMD)?;
    let tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
    let elevation = match p.get("elevation").and_then(Value::as_f64) {
        Some(z) => z,
        None => {
            let zs: Vec<f64> = ring.iter().filter_map(|q| tin.z_at(q[0], q[1])).collect();
            if zs.is_empty() {
                return Err(bad(CMD, "`elevation` is required: the outline is outside the survey"));
            }
            zs.iter().sum::<f64>() / zs.len() as f64
        }
    };
    let pad = Pad::new(
        &ring,
        elevation,
        f64_or(p, "cut_slope", 1.0),
        f64_or(p, "fill_slope", 1.5),
        f64_or(p, "bench_height", 0.0),
        f64_or(p, "berm_width", 2.0),
    )
    .map_err(|e| civil_err(CMD, e))?;
    let method = match str_param(p, "method").map(str::to_lowercase).as_deref() {
        None | Some("prismoidal") | Some("prismoide") => Method::Prismoidal,
        Some("average") | Some("promedio") => Method::Average,
        Some(_) => return Err(bad(CMD, "`method` is prismoidal or average")),
    };
    let opts =
        VolumeOptions { interval: positive(CMD, p, "interval", 2.0)?, angle: f64_or(p, "angle", 0.0), step: positive(CMD, p, "step", 0.5)?, method };
    let vol = pad.volumes(&tin, &opts).map_err(|e| civil_err(CMD, e))?;
    let perimeter: f64 =
        (0..ring.len()).map(|i| (ring[(i + 1) % ring.len()][0] - ring[i][0]).hypot(ring[(i + 1) % ring.len()][1] - ring[i][1])).sum();
    let along = match f64_or(p, "along", 0.0) {
        a if a > 0.0 => a,
        _ => (perimeter / 150.0).max(0.25),
    };
    let runs = pad.daylight(&tin, along, f64_or(p, "march", 0.25).max(0.01));
    let zero = pad.zero_line(&tin);
    let height = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => auto_height(&pts),
    };
    let es = lang_es(p);
    let ticks = bool_or(p, "ticks", true);
    let mut day_points = 0;
    on_layer(s, L_SLOPE, 3, |s| {
        for r in &runs {
            let vs: Vec<PolyVertex> = r.iter().map(|d| PolyVertex::new(Vec2::new(d.pt[0], d.pt[1]))).collect();
            day_points += vs.len();
            s.add_entity(lwpoly(vs, false))?;
            if ticks {
                for d in r.iter().take(MAX_OBJECTS / 4) {
                    s.add_entity(line(Vec2::new(d.edge[0], d.edge[1]), Vec2::new(d.pt[0], d.pt[1])))?;
                }
            }
        }
        let c = ring.iter().fold([0.0, 0.0], |a, q| [a[0] + q[0] / ring.len() as f64, a[1] + q[1] / ring.len() as f64]);
        let label = format!(
            "{} {:.2}  {} {:.1} m³  {} {:.1} m³",
            if es { "PLATAFORMA" } else { "PLATFORM" },
            elevation,
            if es { "CORTE" } else { "CUT" },
            vol.cut,
            if es { "RELLENO" } else { "FILL" },
            vol.fill
        );
        text_at(s, Vec2::new(c[0] - height * 10.0, c[1]), &label, height, 0.0)
    })?;
    on_layer(s, L_ZERO, 1, |s| {
        for (pl, closed) in &zero {
            let vs: Vec<PolyVertex> = pl.iter().map(|q| PolyVertex::new(Vec2::new(q[0], q[1]))).collect();
            if vs.len() >= 2 {
                s.add_entity(lwpoly(vs, *closed))?;
            }
        }
        Ok(())
    })?;
    Ok(json!({
        "elevation": elevation, "area": pad.area(), "cut": vol.cut, "fill": vol.fill, "net": vol.cut - vol.fill, "sections": vol.sections,
        "daylightPoints": day_points, "daylightRuns": runs.len(), "zeroLines": zero.len(),
        "message": format!("Platform at {elevation:.2}: cut {:.1} m³, fill {:.1} m³ ({} sections, {}); {day_points} daylight points.", vol.cut, vol.fill, vol.sections, if method == Method::Prismoidal { "prismoidal" } else { "average end area" })
    }))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_input(path: &str) -> Result<Vec<u8>> {
    crate::file_access::check_read(path).map_err(EngineError::Other)?;
    crate::capped_read::read_capped(std::path::Path::new(path), MAX_FILE).map_err(|e| bad("civil.points.import", format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn read_input(_: &str) -> Result<Vec<u8>> {
    Err(bad("civil.points.import", "this build has no file system: use the desktop app"))
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.points.export";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?;
    let pts = from_drawing(s.doc()?, str_param(p, "layer"));
    if pts.is_empty() {
        return Err(bad(CMD, "the drawing has no points to export"));
    }
    let csv = to_csv(&pts, delimiter(p).unwrap_or(','));
    write_output(path, csv.as_bytes())?;
    Ok(json!({ "count": pts.len(), "message": format!("Exported {} points to {path}.", pts.len()) }))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn write_output(path: &str, bytes: &[u8]) -> Result<()> {
    crate::file_access::check_write(path).map_err(EngineError::Other)?;
    crate::atomic_write::write_atomic(std::path::Path::new(path), bytes).map_err(|e| bad("civil.points.export", format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn write_output(_: &str, _: &[u8]) -> Result<()> {
    Err(bad("civil.points.export", "this build has no file system: use the desktop app"))
}

fn run_tin(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.tin";
    let tin = build_tin(s, p, CMD)?;
    if tin.tris.len() > MAX_OBJECTS {
        return Err(bad(CMD, format!("{} triangles: use `max_edge` or fewer points (the limit is {MAX_OBJECTS})", tin.tris.len())));
    }
    let layer = str_param(p, "layer").filter(|l| !l.trim().is_empty()).unwrap_or(L_TIN).to_string();
    let n = tin.tris.len();
    on_layer(s, &layer, 8, |s| {
        for t in &tin.tris {
            let c: Vec<Vec3> = t.iter().filter_map(|&i| tin.pts.get(i)).map(|q| Vec3::new(q[0], q[1], q[2])).collect();
            if let [a, b, cc] = c[..] {
                s.add_entity(EntityKind::Face3d(Face3d { corners: [a, b, cc, cc], hidden_edges: 0 }))?;
            }
        }
        Ok(())
    })?;
    Ok(
        json!({ "points": tin.pts.len(), "triangles": n, "message": format!("TIN created: {} points, {n} triangles on layer {layer}.", tin.pts.len()) }),
    )
}

fn run_contours(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.contours";
    let interval = positive(CMD, p, "interval", 1.0)?;
    let base = f64_or(p, "base", 0.0);
    let major_every = f64_or(p, "major_every", 5.0).round().max(1.0) as i64;
    let mut pts = drawing_points(s, p, CMD)?;
    let mut tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
    let mut breaks = 0;
    if let Some(layer) = str_param(p, "break_layer").filter(|l| !l.trim().is_empty()) {
        let step = match f64_or(p, "break_step", 0.0) {
            v if v > 0.0 => v,
            _ => auto_height(&pts) * 10.0,
        };
        let mut extra = Vec::new();
        for e in s.doc()?.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(layer)) {
            let line: Vec<(f64, f64, Option<f64>)> = match &e.kind {
                EntityKind::Polyline3d(pl) => pl.points.iter().map(|v| (v.x, v.y, Some(v.z))).collect(),
                EntityKind::LwPolyline(pl) => {
                    let z = Some(pl.elevation).filter(|z| *z != 0.0);
                    flatten_bulges(&pl.vertices.iter().map(|v| (v.p.x, v.p.y, v.bulge)).collect::<Vec<_>>(), pl.closed)
                        .iter()
                        .map(|v| (v[0], v[1], z))
                        .collect()
                }
                EntityKind::Line(l) => vec![(l.a.x, l.a.y, Some(l.a.z).filter(|z| *z != 0.0)), (l.b.x, l.b.y, Some(l.b.z).filter(|z| *z != 0.0))],
                _ => continue,
            };
            breaks += 1;
            extra.extend(break_points(&tin, &line, step));
            if extra.len() > MAX_OBJECTS {
                return Err(bad(CMD, "the break lines give too many points: raise `break_step`"));
            }
        }
        if !extra.is_empty() {
            pts.extend(extra);
            tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
        }
    }
    let lines = contours(&tin, interval, base).map_err(|e| civil_err(CMD, e))?;
    if lines.len() > MAX_OBJECTS {
        return Err(bad(CMD, format!("{} contour lines: use a larger interval", lines.len())));
    }
    let labels = bool_or(p, "labels", true);
    let height = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => auto_height(&pts),
    };
    let is_major = |level: f64| (((level - base) / interval).round() as i64) % major_every == 0;
    let (mut minor, mut major) = (0usize, 0usize);
    let mut levels = std::collections::BTreeSet::new();
    let aci_of = |key: &str, d: i16| (f64_or(p, key, f64::from(d)) as i16).clamp(1, 255);
    for (name, aci, want) in [(L_MINOR, aci_of("minor_color", 8), false), (L_MAJOR, aci_of("major_color", 30), true)] {
        let count = on_layer(s, name, aci, |s| {
            let mut n = 0;
            for c in lines.iter().filter(|c| is_major(c.level) == want) {
                let vs: Vec<PolyVertex> = c.pts.iter().map(|q| PolyVertex::new(Vec2::new(q[0], q[1]))).collect();
                let mut k = lwpoly(vs, c.closed);
                if let EntityKind::LwPolyline(pl) = &mut k {
                    pl.elevation = c.level;
                }
                s.add_entity(k)?;
                if want
                    && labels
                    && let Some(m) = c.pts.get(c.pts.len() / 2)
                {
                    text_at(s, Vec2::new(m[0], m[1]), &format!("{}", (c.level * 1000.0).round() / 1000.0), height, 0.0)?;
                }
                n += 1;
            }
            Ok(n)
        })?;
        if want { major = count } else { minor = count }
    }
    levels.extend(lines.iter().map(|c| (c.level * 1e6).round() as i64));
    Ok(
        json!({ "minor": minor, "major": major, "levels": levels.len(), "breakLines": breaks, "message": format!("Contours: {minor} minor and {major} major lines over {} levels (interval {interval}){}.", levels.len(), if breaks > 0 { format!(", following {breaks} break line(s)") } else { String::new() }) }),
    )
}

/// The axis the command works on: the first selected or named polyline, line or 3D polyline.
fn axis_of(s: &Session, p: &Value, cmd: &str) -> Result<Alignment> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    for h in hs {
        let Some(e) = d.model.get(h) else { continue };
        let pts: Vec<[f64; 2]> = match &e.kind {
            EntityKind::LwPolyline(pl) => flatten_bulges(&pl.vertices.iter().map(|v| (v.p.x, v.p.y, v.bulge)).collect::<Vec<_>>(), pl.closed),
            EntityKind::Polyline3d(pl) => pl.points.iter().map(|v| [v.x, v.y]).collect(),
            EntityKind::Line(l) => vec![[l.a.x, l.a.y], [l.b.x, l.b.y]],
            _ => continue,
        };
        if let Some(a) = Alignment::new(&pts) {
            return Ok(a);
        }
    }
    Err(bad(cmd, "select the axis (a polyline or line) first, or give its `handle`"))
}

fn run_stations(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.stations";
    let axis = axis_of(s, p, CMD)?;
    let interval = positive(CMD, p, "interval", 20.0)?;
    let start = f64_or(p, "start", 0.0);
    let height = positive(CMD, p, "text_height", (interval / 10.0).clamp(0.5, 5.0))?;
    let marks = axis.stations(interval, start).map_err(|e| civil_err(CMD, e))?;
    on_layer(s, L_STATIONS, 3, |s| {
        for m in &marks {
            let (nx, ny) = (-m.angle.sin(), m.angle.cos());
            let tick = height * 1.2;
            s.add_entity(line(Vec2::new(m.x - nx * tick, m.y - ny * tick), Vec2::new(m.x + nx * tick, m.y + ny * tick)))?;
            // The label sits left of the direction of travel, turned to read along the axis.
            let mut rot = m.angle;
            let flip = rot.cos() < -1e-9;
            if flip {
                rot += std::f64::consts::PI;
            }
            let side = if flip { -1.0 } else { 1.0 };
            let at = Vec2::new(m.x + nx * tick * 1.3 * side, m.y + ny * tick * 1.3 * side);
            text_at(s, at, &format_station(m.station), height, rot)?;
        }
        Ok(())
    })?;
    Ok(
        json!({ "length": axis.length(), "stations": marks.len(), "message": format!("{} stations every {interval} along {:.2} m.", marks.len(), axis.length()) }),
    )
}

/// The design grade from `pvis`: an array of `[station, elevation, curve?]` or text such as
/// `0,100; 120,104,40; 300,101`.
fn grade_param(p: &Value, cmd: &str) -> Result<Option<Grade>> {
    let pvis: Vec<Pvi> = match p.get("pvis") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(t)) if t.trim().is_empty() => return Ok(None),
        Some(Value::String(t)) => parse_pvis(t).map_err(|e| civil_err(cmd, e))?,
        Some(Value::Array(a)) => {
            let mut v = Vec::new();
            for e in a {
                let n: Vec<f64> = e.as_array().map(|r| r.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                match n.as_slice() {
                    [s, z] => v.push(Pvi { station: *s, elevation: *z, curve: 0.0 }),
                    [s, z, l] => v.push(Pvi { station: *s, elevation: *z, curve: *l }),
                    _ => return Err(bad(cmd, "each of `pvis` is [station, elevation] or [station, elevation, curve length]")),
                }
            }
            v
        }
        Some(_) => return Err(bad(cmd, "`pvis` is an array of [station, elevation, curve?] or text \"0,100; 120,104,40\"")),
    };
    Grade::new(pvis).map(Some).map_err(|e| civil_err(cmd, e))
}

fn lang_es(p: &Value) -> bool {
    !str_param(p, "language").is_some_and(|l| l.eq_ignore_ascii_case("en"))
}

fn earth_params(p: &Value, cmd: &str, start: f64) -> Result<EarthParams> {
    let interval = positive(cmd, p, "interval", 20.0)?;
    Ok(EarthParams {
        start,
        interval,
        half_width: positive(cmd, p, "half_width", 5.0)?,
        crossfall: f64_or(p, "crossfall", 0.02),
        step: positive(cmd, p, "offset_step", 0.5)?,
        factor: positive(cmd, p, "factor", 1.0)?,
    })
}

fn run_profile(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.profile";
    let axis = axis_of(s, p, CMD)?;
    let interval = positive(CMD, p, "interval", 20.0)?;
    let step = positive(CMD, p, "step", (interval / 2.0).min(10.0))?;
    let start = f64_or(p, "start", 0.0);
    let vscale = positive(CMD, p, "vscale", 10.0)?;
    let pts = drawing_points(s, p, CMD)?;
    let height = match f64_or(p, "text_height", 0.0) {
        h if h > 0.0 => h,
        _ => auto_height(&pts),
    };
    let tin = Tin::build(&pts, max_edge(p)).map_err(|e| civil_err(CMD, e))?;
    let prof = axis.profile(&tin, step, start).map_err(|e| civil_err(CMD, e))?;
    let grounds: Vec<f64> = prof.iter().filter_map(|q| q.ground).collect();
    if grounds.is_empty() {
        return Err(bad(CMD, "the axis lies outside the surveyed ground: nothing to profile"));
    }
    let grade = grade_param(p, CMD)?;
    let es = lang_es(p);
    let grade_samples: Vec<(f64, f64)> = match &grade {
        Some(g) => {
            let (a, b) = (g.start().max(start), g.end().min(start + axis.length()));
            let n = (((b - a) / step).ceil() as usize).clamp(1, 100_000);
            (0..=n)
                .filter_map(|i| {
                    let st = a + (b - a) * i as f64 / n as f64;
                    g.elevation(st).map(|z| (st, z))
                })
                .collect()
        }
        None => Vec::new(),
    };
    let (zlo, zhi) = grounds.iter().chain(grade_samples.iter().map(|(_, z)| z)).fold((f64::MAX, f64::MIN), |(l, h), z| (l.min(*z), h.max(*z)));
    let datum = zlo.floor() - 1.0;
    // Below the axis's lowest point, so the profile does not land on the plan.
    let origin = match crate::cmd::point_param(p, "origin") {
        Some(o) => o,
        None => {
            let ymin = pts.iter().map(|q| q.y).fold(f64::MAX, f64::min);
            let xmin = pts.iter().map(|q| q.x).fold(f64::MAX, f64::min);
            Vec2::new(xmin, ymin - (zhi - datum + 4.0) * vscale - 10.0 * height)
        }
    };
    let at = |station: f64, z: f64| Vec2::new(origin.x + (station - start), origin.y + (z - datum) * vscale);
    let length = axis.length();
    let marks = axis.stations(interval, start).map_err(|e| civil_err(CMD, e))?;
    on_layer(s, L_PROFILE, 2, |s| {
        // The datum line and the ground, in runs that break where the axis leaves the survey.
        s.add_entity(line(at(start, datum), at(start + length, datum)))?;
        text_at(s, Vec2::new(origin.x, origin.y - height * 3.0), &format!("Datum {datum:.2}   V:H = {vscale}:1"), height, 0.0)?;
        let mut run: Vec<PolyVertex> = Vec::new();
        for q in &prof {
            match q.ground {
                Some(z) => run.push(PolyVertex::new(at(q.station, z))),
                None => flush(s, &mut run)?,
            }
        }
        flush(s, &mut run)?;
        if let Some(g) = &grade {
            on_layer(s, L_GRADE, 1, |s| {
                let vs: Vec<PolyVertex> = grade_samples.iter().map(|(st, z)| PolyVertex::new(at(*st, *z))).collect();
                if vs.len() >= 2 {
                    s.add_entity(lwpoly(vs, false))?;
                }
                for (i, pv) in g.pvis().iter().enumerate() {
                    let top = at(pv.station, pv.elevation);
                    text_at(
                        s,
                        Vec2::new(top.x + height * 0.5, top.y + height * 1.5),
                        &format!("PIV {}  {:.2}", format_station(pv.station), pv.elevation),
                        height,
                        std::f64::consts::FRAC_PI_2,
                    )?;
                    if let Some(c) = g.curve_info(i) {
                        let label = format!(
                            "{} L={:.2} K={:.2} {}→{}",
                            if es { "CV" } else { "VC" },
                            pv.curve,
                            c.k,
                            format_station(c.bvc),
                            format_station(c.evc)
                        );
                        text_at(s, Vec2::new(top.x - height * 2.0, top.y - height * 3.0), &label, height, 0.0)?;
                    }
                }
                Ok(())
            })?;
        }
        for m in &marks {
            let Some(z) = tin.z_at(m.x, m.y) else { continue };
            let (base, top) = (at(m.station, datum), at(m.station, z));
            if let Some(gz) = grade.as_ref().and_then(|g| g.elevation(m.station)) {
                let d = z - gz;
                let tag = format!(
                    "{} {:.2}  {:.2}",
                    if d >= 0.0 {
                        "C"
                    } else if es {
                        "T"
                    } else {
                        "F"
                    },
                    d.abs(),
                    gz
                );
                text_at(s, Vec2::new(base.x + height * 1.0, base.y - height * 8.0), &tag, height, std::f64::consts::FRAC_PI_2)?;
            }
            s.add_entity(line(base, top))?;
            text_at(s, Vec2::new(base.x - height * 0.5, base.y - height * 8.0), &format_station(m.station), height, std::f64::consts::FRAC_PI_2)?;
            text_at(s, Vec2::new(top.x - height * 0.5, top.y + height * 0.8), &format!("{z:.2}"), height, std::f64::consts::FRAC_PI_2)?;
        }
        Ok(())
    })?;
    let table: Vec<Value> = prof.iter().map(|q| json!({ "station": q.station, "label": format_station(q.station), "ground": q.ground })).collect();
    Ok(
        json!({ "length": length, "datum": datum, "table": table, "message": format!("Profile of {length:.2} m drawn below the plan (datum {datum:.2}, ground {zlo:.2} to {zhi:.2}).") }),
    )
}

/// Draw the vertices collected so far as one polyline (if there are at least two) and start over.
fn flush(s: &mut Session, run: &mut Vec<PolyVertex>) -> Result<()> {
    if run.len() >= 2 {
        s.add_entity(lwpoly(std::mem::take(run), false))?;
    }
    run.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    /// A 9 x 9 grid of points on the plane `z = x / 2`, in a CSV with a header and decimal commas
    /// the way a Spanish-language spreadsheet exports them.
    fn survey_file(name: &str) -> String {
        let mut csv = String::from("Punto;Norte;Este;Cota;Descripcion\n");
        let mut n = 0;
        for i in 0..9 {
            for j in 0..9 {
                n += 1;
                csv.push_str(&format!("{n};{},5;{},0;{},25;TN\n", j * 10, i * 10, f64::from(i) * 5.0));
            }
        }
        let path = std::env::temp_dir().join(format!("civil-ing-{}-{name}.csv", std::process::id()));
        std::fs::write(&path, csv).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn types(s: &Session) -> Vec<&'static str> {
        s.doc().unwrap().model.iter().map(|e| e.kind.type_name()).collect()
    }

    #[test]
    fn importing_a_csv_draws_points_with_elevation_and_labels_on_civil_layers() {
        let path = survey_file("import");
        let mut s = Session::new();
        let r = s.execute("civil.points.import", &json!({ "path": path })).unwrap();
        assert_eq!(r["count"], 81);
        assert_eq!(r["columns"], "PYXZD");
        let d = s.doc().unwrap();
        let pts: Vec<_> = d.model.iter().filter(|e| e.kind.type_name() == "Point").collect();
        assert_eq!(pts.len(), 81);
        assert!(pts.iter().all(|e| e.common.layer == "CIVIL-PUNTOS"));
        assert!(d.layers.iter().any(|l| l.name == "CIVIL-PUNTOS-COTA"));
        assert_eq!(s.doc().unwrap().header.str("CLAYER", "0"), "0", "the current layer is put back");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_bad_file_says_why_and_draws_nothing() {
        let mut s = Session::new();
        let e = s.execute("civil.points.import", &json!({ "path": "/no/such/file.csv" })).unwrap_err();
        assert!(e.to_string().contains("file.csv"));
        assert!(s.execute("civil.points.import", &json!({})).is_err());
        assert_eq!(s.doc().unwrap().model.len(), 0);
    }

    #[test]
    fn tin_contours_stations_and_profile_follow_from_imported_points() {
        let path = survey_file("flow");
        let mut s = Session::new();
        s.execute("civil.points.import", &json!({ "path": path, "labels": false })).unwrap();
        let tin = s.execute("civil.tin", &json!({})).unwrap();
        assert_eq!(tin["triangles"], 2 * 8 * 8);
        let c = s.execute("civil.contours", &json!({ "interval": 5, "labels": false })).unwrap();
        assert!(c["levels"].as_u64().unwrap() >= 7);
        assert!(c["major"].as_u64().unwrap() > 0 && c["minor"].as_u64().unwrap() > 0);
        let axis = s.execute("pline", &json!({ "vertices": [[5.0, 5.0], [75.0, 5.0]] })).unwrap();
        let h = axis["handle"].as_str().unwrap().to_string();
        let st = s.execute("civil.stations", &json!({ "handle": h, "interval": 20 })).unwrap();
        assert_eq!(st["stations"], 5);
        let pr = s.execute("civil.profile", &json!({ "handle": h, "interval": 20, "step": 10 })).unwrap();
        let first = &pr["table"][0];
        assert_eq!(first["label"], "0+000.00");
        assert!(first["ground"].as_f64().is_some());
        assert!(types(&s).contains(&"Face3d") || types(&s).iter().any(|t| t.contains("3D")), "TIN faces are drawn");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn commands_that_need_points_or_an_axis_explain_what_is_missing() {
        let mut s = Session::new();
        assert!(s.execute("civil.contours", &json!({ "interval": 1 })).unwrap_err().to_string().contains("POINT"));
        s.execute("point", &json!({ "points": [[0, 0], [1, 0], [0, 1]] })).unwrap();
        assert!(s.execute("civil.stations", &json!({})).unwrap_err().to_string().contains("axis"));
        assert!(s.execute("civil.contours", &json!({ "interval": 0 })).is_err());
    }

    #[test]
    fn exported_points_read_back_with_the_same_elevations() {
        let path = survey_file("export-src");
        let out = std::env::temp_dir().join(format!("civil-ing-{}-out.csv", std::process::id()));
        let mut s = Session::new();
        s.execute("civil.points.import", &json!({ "path": path, "labels": false })).unwrap();
        s.execute("civil.points.export", &json!({ "path": out.to_string_lossy() })).unwrap();
        let mut again = Session::new();
        let r = again.execute("civil.points.import", &json!({ "path": out.to_string_lossy(), "labels": false })).unwrap();
        assert_eq!(r["count"], 81);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn radiations_are_drawn_joined_and_a_break_line_bends_the_contours() {
        let path = std::env::temp_dir().join(format!("civil-ing-{}-rad.txt", std::process::id()));
        std::fs::write(&path, "PV ANGH ANGV DINC HAP\n2 90 90 50 0\n1 0 90 30 0\n3 180 90 40 0\n").unwrap();
        let mut s = Session::new();
        let r = s
            .execute(
                "civil.points.radiate",
                &json!({ "path": path.to_str().unwrap(), "station": { "x": 100, "y": 200, "z": 10 }, "order": true, "join": "polyline3d", "labels": false }),
            )
            .unwrap();
        assert_eq!(r["count"], 3);
        let d = s.doc().unwrap();
        let joined: Vec<_> = d.model.iter().filter(|e| e.common.layer == "CIVIL-UNION").collect();
        assert_eq!(joined.len(), 1);
        match &joined[0].kind {
            cadcraft_doc::EntityKind::Polyline3d(pl) => {
                // Ordered by number: 1 is due north, 2 east, 3 south.
                assert!((pl.points[0].y - 230.0).abs() < 1e-9 && (pl.points[1].x - 150.0).abs() < 1e-9 && (pl.points[2].y - 160.0).abs() < 1e-9);
            }
            o => panic!("expected a 3D polyline, got {o:?}"),
        }
        assert!(s.execute("civil.points.radiate", &json!({ "path": path.to_str().unwrap() })).unwrap_err().to_string().contains("station"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn contours_take_break_lines_and_colours_and_join_command_joins_drawing_points() {
        let path = survey_file("brk");
        let mut s = Session::new();
        s.execute("civil.points.import", &json!({ "path": path, "labels": false })).unwrap();
        s.execute("pline", &json!({ "vertices": [[0, 0], [8, 8]], "layer": "BREAK" })).ok();
        let c = s.execute("civil.contours", &json!({ "interval": 5, "labels": false, "break_layer": "BREAK", "major_color": 1 })).unwrap();
        assert!(c["levels"].as_u64().unwrap() > 0);
        assert!(s.doc().unwrap().layers.iter().any(|l| l.name == "CIVIL-CURVAS-MAYORES"));
        let j = s.execute("civil.points.join", &json!({ "kind": "polyline" })).unwrap();
        assert!(j["count"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn the_construction_table_reports_area_and_draws_on_its_layers() {
        let mut s = Session::new();
        let h = s.execute("pline", &json!({ "vertices": [[0, 0], [30, 0], [30, 20], [0, 20]], "closed": true })).unwrap();
        let handle = h["handle"].clone();
        let r = s.execute("civil.parcel", &json!({ "handle": handle, "language": "en" })).unwrap();
        assert!((r["area"].as_f64().unwrap() - 600.0).abs() < 1e-9);
        assert_eq!(r["vertices"], 4);
        assert!(r["angleError"].as_f64().unwrap().abs() < 1e-9);
        assert_eq!(r["sides"][0]["bearing"], "N");
        let d = s.doc().unwrap();
        assert!(d.layers.iter().any(|l| l.name == "CIVIL-CUADRO") && d.layers.iter().any(|l| l.name == "CIVIL-VERTICES"));
        assert!(s.execute("civil.parcel", &json!({ "handle": 0 })).is_err());
    }

    #[test]
    fn earthwork_sections_and_a_profile_with_grade_follow_from_the_survey() {
        let path = survey_file("earth");
        let csv = std::env::temp_dir().join(format!("civil-ing-{}-earth-out.csv", std::process::id()));
        let mut s = Session::new();
        s.execute("civil.points.import", &json!({ "path": path, "labels": false })).unwrap();
        let axis = s.execute("pline", &json!({ "vertices": [[5.0, 20.0], [75.0, 20.0]] })).unwrap();
        let h = axis["handle"].as_str().unwrap().to_string();
        let pvis = "0,10; 35,20,20; 70,30";
        let pr = s.execute("civil.profile", &json!({ "handle": h, "interval": 20, "step": 10, "pvis": pvis })).unwrap();
        assert!(pr["message"].as_str().unwrap().contains("Profile"));
        assert!(s.doc().unwrap().layers.iter().any(|l| l.name == "CIVIL-RASANTE"));
        let r = s.execute("civil.earthwork", &json!({ "handle": h, "pvis": pvis, "half_width": 6, "path": csv.to_str().unwrap() })).unwrap();
        assert_eq!(r["stations"], 5);
        assert!(r["cut"].as_f64().unwrap() > 0.0 && r["fill"].as_f64().unwrap() > 0.0);
        assert_eq!(r["rows"].as_array().unwrap().len(), 5);
        assert!(std::fs::read_to_string(&csv).unwrap().lines().count() == 6);
        let sec = s.execute("civil.sections", &json!({ "handle": h, "pvis": pvis, "half_width": 6 })).unwrap();
        assert_eq!(sec["sections"], 5);
        assert!(
            s.doc().unwrap().layers.iter().any(|l| l.name == "CIVIL-SECCIONES")
                && s.doc().unwrap().layers.iter().any(|l| l.name == "CIVIL-TERRACERIAS")
        );
        let e = s.execute("civil.earthwork", &json!({ "handle": h })).unwrap_err().to_string();
        assert!(e.contains("pvis"));
        assert!(s.execute("civil.earthwork", &json!({ "handle": h, "pvis": "0,1" })).is_err());
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(csv);
    }

    #[test]
    fn a_platform_gets_slopes_a_daylight_line_volumes_and_a_label() {
        let path = survey_file("pad");
        let mut s = Session::new();
        s.execute("civil.points.import", &json!({ "path": path, "labels": false })).unwrap();
        let pad = s.execute("pline", &json!({ "vertices": [[20, 20], [50, 20], [50, 50], [20, 50]], "closed": true })).unwrap();
        let h = pad["handle"].as_str().unwrap().to_string();
        let r = s.execute("civil.platform", &json!({ "handle": h, "elevation": 12.0, "interval": 2, "step": 1 })).unwrap();
        assert!((r["area"].as_f64().unwrap() - 900.0).abs() < 1e-6);
        assert!(r["cut"].as_f64().unwrap() > 0.0 && r["fill"].as_f64().unwrap() > 0.0, "{r}");
        assert!(r["daylightPoints"].as_u64().unwrap() > 10);
        let d = s.doc().unwrap();
        assert!(d.layers.iter().any(|l| l.name == "CIVIL-TALUD") && d.layers.iter().any(|l| l.name == "CIVIL-LINEA-CERO"));
        assert!(s.execute("civil.platform", &json!({ "handle": h, "method": "x" })).is_err());
        assert!(s.execute("civil.platform", &json!({ "handle": h, "cut_slope": 0 })).is_err());
        let _ = std::fs::remove_file(path);
    }
}
