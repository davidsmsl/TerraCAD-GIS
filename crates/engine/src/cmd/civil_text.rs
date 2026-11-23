//! Civil-ING text tools: text along an arc, numbering, swapping, respacing, real-millimetre
//! heights, text import and export, and inserting a detail block at a size that fits.

use cadcraft_doc::{EntityKind, Handle, entity_bounds};
use cadcraft_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::civil::{on_layer, text_at};
use super::{CommandSpec, bad, bool_or, f64_or, find_command, str_param, targets};
use crate::{Result, Session};

const L_TEXT: &str = "CIVIL-TEXTO";
const MAX_TEXTS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("civil.text.arc", "Text Along Arc...", run_arc)
            .menu(&["Civil-ING", "Text", "Text Along Arc..."])
            .params("{text, handle? (an arc or circle; else center + radius), center?: [x,y], radius?, angle?: 90 (degrees, the middle of the text), reverse?: false (read along the bottom, counterclockwise), height?: 1, spacing?: 1 (multiplier of the letter advance), layer?} → {count}"),
        CommandSpec::new("civil.text.number", "Number Texts...", run_number)
            .menu(&["Civil-ING", "Text", "Number Texts..."])
            .params("{handles? (else the selection), prefix?, start?: 1, step?: 1, digits?: 0 (zero padding), suffix?, order?: selection|x|y} → {count}"),
        CommandSpec::new("civil.text.swap", "Swap Texts", run_swap)
            .menu(&["Civil-ING", "Text", "Swap Texts"])
            .params("{handles? (else the selection): exactly two texts} → swaps their contents"),
        CommandSpec::new("civil.text.space", "Respace Texts...", run_space)
            .menu(&["Civil-ING", "Text", "Respace Texts..."])
            .params("{handles? (else the selection), spacing? (default 1.5 times the first text's height), axis?: y|x} → {count}; lines the texts up on the first one at an even distance, in their current order along the axis"),
        CommandSpec::new("civil.text.height", "Real Text Height...", run_height)
            .menu(&["Civil-ING", "Text", "Real Text Height..."])
            .params("{scale (plot scale denominator, 500 for 1:500), mm?: 2.5 (height on paper), units_per_meter?: 1 (drawing units in a metre; 1 when the drawing is in metres), handles? (else the selection), all?: bool} → {height, count}"),
        CommandSpec::new("civil.text.export", "Export Texts...", run_export)
            .menu(&["Civil-ING", "Text", "Export Texts..."])
            .params("{path (CSV to write), handles? (else all texts)} → {count}; columns x, y, height, rotation (degrees), layer, text"),
        CommandSpec::new("civil.text.import", "Import Texts...", run_import)
            .menu(&["Civil-ING", "Text", "Import Texts..."])
            .params("{path (CSV with x, y, text and optionally height, rotation, layer, or a plain list of lines), at?: [x,y] and spacing? for plain lists, height?} → {count}"),
        CommandSpec::new("civil.detail.list", "List Details", run_detail_list)
            .menu(&["Civil-ING", "Details", "List Details"])
            .params("{} → {details: [{name, description, width, height, entities}]}; the blocks of the drawing, for the road and project detail library")
            .noundo(),
        CommandSpec::new("civil.detail.insert", "Insert Detail...", run_detail_insert)
            .menu(&["Civil-ING", "Details", "Insert Detail..."])
            .params("{name, at: [x,y], width? (target width in drawing units), or paper_mm? with scale? (plot scale) for a width on paper, rotation?} → {scale, description}; the scale is found so the detail fits that width (without width, the block's own size)"),
    ]
}

fn texts_of(s: &Session, p: &Value) -> Result<Vec<Handle>> {
    let hs = targets(s, p).unwrap_or_default();
    let d = s.doc()?;
    Ok(hs.into_iter().filter(|h| matches!(d.model.get(*h).map(|e| &e.kind), Some(EntityKind::Text(_)))).collect())
}

fn set_text(s: &mut Session, h: Handle, f: impl FnOnce(&mut cadcraft_doc::Text)) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::Text(t) = &mut e.kind {
            f(t)
        }
    })?;
    Ok(())
}

/// Text advance per letter, as a fraction of the height (a typical proportional font).
const ADVANCE: f64 = 0.62;

fn run_arc(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.arc";
    let text = str_param(p, "text").filter(|t| !t.is_empty()).ok_or_else(|| bad(CMD, "`text` is required"))?;
    let (mut center, mut radius) = (crate::cmd::point_param(p, "center"), f64_or(p, "radius", 0.0));
    if center.is_none() || radius <= 0.0 {
        let hs = targets(s, p).unwrap_or_default();
        let d = s.doc()?;
        for h in hs {
            match d.model.get(h).map(|e| &e.kind) {
                Some(EntityKind::Arc(a)) => (center, radius) = (Some(Vec2::new(a.center.x, a.center.y)), a.radius),
                Some(EntityKind::Circle(c)) => (center, radius) = (Some(Vec2::new(c.center.x, c.center.y)), c.radius),
                _ => continue,
            }
            break;
        }
    }
    let (Some(c), true) = (center, radius > 0.0) else { return Err(bad(CMD, "give an arc or circle (`handle`), or `center` and `radius`")) };
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > 2000 {
        return Err(bad(CMD, "the text is too long for an arc"));
    }
    let height = f64_or(p, "height", 1.0);
    if height <= 0.0 {
        return Err(bad(CMD, "`height` must be above zero"));
    }
    let adv = height * ADVANCE * f64_or(p, "spacing", 1.0).max(0.1);
    let reverse = bool_or(p, "reverse", false);
    let mid = f64_or(p, "angle", 90.0).to_radians();
    let total = adv * chars.len() as f64;
    let layer = str_param(p, "layer").filter(|l| !l.trim().is_empty()).unwrap_or(L_TEXT).to_string();
    on_layer(s, &layer, 7, |s| {
        for (i, ch) in chars.iter().enumerate() {
            // Distance of the middle of this letter from the middle of the text, along the arc.
            let off = adv * (i as f64 + 0.5) - total / 2.0;
            // On top, text reads clockwise; along the bottom it reads counterclockwise.
            let theta = if reverse { mid + off / radius } else { mid - off / radius };
            let rot = if reverse { theta + std::f64::consts::FRAC_PI_2 } else { theta - std::f64::consts::FRAC_PI_2 };
            let pos = Vec2::new(c.x + radius * theta.cos(), c.y + radius * theta.sin());
            // The text object starts at its left end: back off half an advance along the baseline.
            let at = Vec2::new(pos.x - rot.cos() * adv / 2.0, pos.y - rot.sin() * adv / 2.0);
            if !ch.is_whitespace() {
                text_at(s, at, &ch.to_string(), height, rot)?;
            }
        }
        Ok(())
    })?;
    Ok(
        json!({ "count": chars.iter().filter(|c| !c.is_whitespace()).count(), "message": format!("Text of {} letters set along an arc of radius {radius}.", chars.len()) }),
    )
}

fn run_number(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.number";
    let mut hs = texts_of(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "select the texts to number first"));
    }
    let pos = |s: &Session, h: Handle| match s.doc().ok().and_then(|d| d.model.get(h)).map(|e| &e.kind) {
        Some(EntityKind::Text(t)) => (t.insert.x, t.insert.y),
        _ => (0.0, 0.0),
    };
    match str_param(p, "order") {
        Some("x") => hs.sort_by(|a, b| pos(s, *a).0.total_cmp(&pos(s, *b).0)),
        Some("y") => hs.sort_by(|a, b| pos(s, *b).1.total_cmp(&pos(s, *a).1)),
        _ => {}
    }
    let (prefix, suffix) = (str_param(p, "prefix").unwrap_or(""), str_param(p, "suffix").unwrap_or(""));
    let (start, step) = (f64_or(p, "start", 1.0).round() as i64, f64_or(p, "step", 1.0).round() as i64);
    let digits = f64_or(p, "digits", 0.0).clamp(0.0, 12.0) as usize;
    for (i, h) in hs.iter().enumerate() {
        let n = start + step * i as i64;
        let v = format!("{prefix}{n:0digits$}{suffix}");
        set_text(s, *h, |t| t.value = v)?;
    }
    Ok(json!({ "count": hs.len(), "message": format!("Numbered {} texts from {start}.", hs.len()) }))
}

fn run_swap(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.swap";
    let hs = texts_of(s, p)?;
    if hs.len() != 2 {
        return Err(bad(CMD, format!("select exactly 2 texts ({} selected)", hs.len())));
    }
    let get = |s: &Session, h: Handle| match s.doc().ok().and_then(|d| d.model.get(h)).map(|e| &e.kind) {
        Some(EntityKind::Text(t)) => t.value.clone(),
        _ => String::new(),
    };
    let (a, b) = (get(s, hs[0]), get(s, hs[1]));
    set_text(s, hs[0], |t| t.value = b)?;
    set_text(s, hs[1], |t| t.value = a)?;
    Ok(json!({ "message": "Swapped the contents of the 2 texts." }))
}

fn run_space(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.space";
    let mut hs = texts_of(s, p)?;
    if hs.len() < 2 {
        return Err(bad(CMD, "select at least 2 texts"));
    }
    let along_x = str_param(p, "axis").is_some_and(|a| a.eq_ignore_ascii_case("x"));
    let info = |s: &Session, h: Handle| match s.doc().ok().and_then(|d| d.model.get(h)).map(|e| &e.kind) {
        Some(EntityKind::Text(t)) => (t.insert.x, t.insert.y, t.height),
        _ => (0.0, 0.0, 1.0),
    };
    // In reading order along the axis: left to right, or top to bottom.
    hs.sort_by(|a, b| if along_x { info(s, *a).0.total_cmp(&info(s, *b).0) } else { info(s, *b).1.total_cmp(&info(s, *a).1) });
    let (x0, y0, h0) = info(s, hs[0]);
    let gap = match f64_or(p, "spacing", 0.0) {
        g if g > 0.0 => g,
        _ => h0 * 1.5,
    };
    for (i, h) in hs.iter().enumerate() {
        let k = i as f64 * gap;
        set_text(s, *h, |t| {
            if along_x {
                t.insert.x = x0 + k;
                t.insert.y = y0;
            } else {
                t.insert.x = x0;
                t.insert.y = y0 - k;
            }
        })?;
    }
    Ok(json!({ "count": hs.len(), "message": format!("Lined up {} texts every {gap}.", hs.len()) }))
}

fn run_height(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.height";
    let scale = f64_or(p, "scale", 0.0);
    if scale.is_nan() || scale <= 0.0 {
        return Err(bad(CMD, "`scale` is required: the plot scale denominator (500 for 1:500)"));
    }
    let mm = f64_or(p, "mm", 2.5);
    let upm = f64_or(p, "units_per_meter", 1.0);
    if !(mm > 0.0 && upm > 0.0) {
        return Err(bad(CMD, "`mm` and `units_per_meter` must be above zero"));
    }
    // mm on paper × scale = mm in the world; /1000 gives metres; × units per metre, drawing units.
    let height = mm / 1000.0 * scale * upm;
    let hs: Vec<Handle> = if bool_or(p, "all", false) {
        s.doc()?.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).map(|e| e.handle).collect()
    } else {
        texts_of(s, p)?
    };
    if hs.is_empty() {
        return Err(bad(CMD, "select the texts first, or pass `all`"));
    }
    for h in &hs {
        set_text(s, *h, |t| t.height = height)?;
    }
    Ok(
        json!({ "height": height, "count": hs.len(), "message": format!("{} texts set to {height:.4} drawing units ({mm} mm at 1:{scale}).", hs.len()) }),
    )
}

fn csv_field(v: &str) -> String {
    if v.contains([',', '"', '\n', '\r']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.export";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?;
    let chosen = texts_of(s, p)?;
    let d = s.doc()?;
    let mut csv = String::from("x,y,height,rotation,layer,text\n");
    let mut n = 0;
    for e in d.model.iter() {
        let EntityKind::Text(t) = &e.kind else { continue };
        if !chosen.is_empty() && !chosen.contains(&e.handle) {
            continue;
        }
        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            t.insert.x,
            t.insert.y,
            t.height,
            t.rotation.to_degrees(),
            csv_field(&e.common.layer),
            csv_field(&t.value.replace('\n', " "))
        ));
        n += 1;
        if n >= MAX_TEXTS {
            break;
        }
    }
    super::civil::write_output(path, csv.as_bytes())?;
    Ok(json!({ "count": n, "message": format!("Exported {n} texts.") }))
}

/// Split one CSV line, honouring double quotes.
fn csv_line(l: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    let mut it = l.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' if quoted && it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.text.import";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?;
    let bytes = super::civil::read_input(path)?;
    let text = cadcraft_civil::points::decode_text(&bytes);
    let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Err(bad(CMD, "the file has no lines"));
    }
    if lines.len() > MAX_TEXTS {
        return Err(bad(CMD, "too many lines"));
    }
    let head: Vec<String> = csv_line(lines[0]).iter().map(|h| h.trim().to_lowercase()).collect();
    let col = |n: &str| head.iter().position(|h| h == n);
    let height0 = f64_or(p, "height", 1.0);
    let mut count = 0;
    if let (Some(cx), Some(cy), Some(ct)) = (col("x"), col("y"), col("text")) {
        let (ch, cr, cl) = (col("height"), col("rotation"), col("layer"));
        for l in &lines[1..] {
            let f = csv_line(l);
            let num = |i: usize| f.get(i).and_then(|v| v.trim().parse::<f64>().ok());
            let (Some(x), Some(y)) = (num(cx), num(cy)) else { continue };
            let layer = cl.and_then(|i| f.get(i)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| L_TEXT.to_string());
            let value = f.get(ct).cloned().unwrap_or_default();
            let (h, r) = (ch.and_then(num).filter(|h| *h > 0.0).unwrap_or(height0), cr.and_then(num).unwrap_or(0.0).to_radians());
            on_layer(s, &layer, 7, |s| text_at(s, Vec2::new(x, y), &value, h, r))?;
            count += 1;
        }
    } else {
        // A plain list: one text per line, stacked downwards from `at`.
        let at = crate::cmd::point_param(p, "at").unwrap_or(Vec2::new(0.0, 0.0));
        let gap = match f64_or(p, "spacing", 0.0) {
            g if g > 0.0 => g,
            _ => height0 * 1.5,
        };
        on_layer(s, L_TEXT, 7, |s| {
            for (i, l) in lines.iter().enumerate() {
                text_at(s, Vec2::new(at.x, at.y - gap * i as f64), l.trim(), height0, 0.0)?;
            }
            Ok(())
        })?;
        count = lines.len();
    }
    Ok(json!({ "count": count, "message": format!("Imported {count} texts.") }))
}

/// The box of the contents of a block, `None` for an empty one.
fn block_box(s: &Session, name: &str) -> Result<Option<(Bounds2, usize, String)>> {
    let d = s.doc()?;
    let Some(b) = d.blocks.get(name) else { return Ok(None) };
    let mut bx: Option<Bounds2> = None;
    let mut n = 0;
    for e in b.entities.iter() {
        let eb = entity_bounds(d, e, 0);
        if eb.is_empty() {
            continue;
        }
        bx = Some(bx.map_or(eb, |a| a.union(&eb)));
        n += 1;
    }
    Ok(bx.map(|b| (b, n, d.blocks.get(name).map(|x| x.description.clone()).unwrap_or_default())))
}

fn run_detail_list(s: &mut Session, _: &Value) -> Result<Value> {
    let names: Vec<String> = s.doc()?.blocks.keys().filter(|n| !n.starts_with('*')).cloned().collect();
    let mut out = Vec::new();
    for n in names {
        let desc = s.doc()?.blocks.get(&n).map(|b| b.description.clone()).unwrap_or_default();
        match block_box(s, &n)? {
            Some((b, count, d)) => out.push(json!({ "name": n, "description": d, "width": b.width(), "height": b.height(), "entities": count })),
            None => out.push(json!({ "name": n, "description": desc, "width": 0.0, "height": 0.0, "entities": 0 })),
        }
    }
    let msg = if out.is_empty() {
        "The drawing has no blocks: make or insert some (BLOCK, INSERT) to build a detail library.".to_string()
    } else {
        format!("{} details.", out.len())
    };
    Ok(json!({ "details": out, "message": msg }))
}

fn run_detail_insert(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "civil.detail.insert";
    let name = str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(CMD, "`name` is required"))?;
    let at = crate::cmd::point_param(p, "at").ok_or_else(|| bad(CMD, "`at` is required: [x, y]"))?;
    let Some((bx, _, desc)) = block_box(s, name)? else {
        return Err(bad(CMD, format!("no block `{name}` with contents in this drawing (CIVIL.DETAIL.LIST shows them)")));
    };
    let width = match (f64_or(p, "width", 0.0), f64_or(p, "paper_mm", 0.0), f64_or(p, "scale", 0.0)) {
        (w, _, _) if w > 0.0 => Some(w),
        (_, mm, sc) if mm > 0.0 && sc > 0.0 => Some(mm / 1000.0 * sc * f64_or(p, "units_per_meter", 1.0)),
        _ => None,
    };
    let scale = match width {
        Some(w) if bx.width() > 0.0 => w / bx.width(),
        Some(_) => return Err(bad(CMD, "the detail has no width to fit")),
        None => 1.0,
    };
    let spec = find_command("insert").ok_or_else(|| bad(CMD, "the INSERT command is missing"))?;
    (spec.run)(s, &json!({ "name": name, "at": [at.x, at.y], "scale": scale, "rotation": f64_or(p, "rotation", 0.0) }))?;
    Ok(
        json!({ "scale": scale, "description": desc, "message": format!("Inserted `{name}` at scale {scale:.4}{}", if desc.is_empty() { String::new() } else { format!(": {desc}") }) }),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn texts(s: &Session) -> Vec<(String, f64, f64, f64)> {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                cadcraft_doc::EntityKind::Text(t) => Some((t.value.clone(), t.insert.x, t.insert.y, t.height)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn text_follows_an_arc_numbers_swaps_respaces_and_takes_a_real_height() {
        let mut s = Session::new();
        let r = s.execute("civil.text.arc", &json!({ "text": "VIA 1", "center": [0, 0], "radius": 50, "height": 2 })).unwrap();
        assert_eq!(r["count"], 4);
        let t = texts(&s);
        // Centred on the top of the circle, the letters sit on it and read left to right.
        assert!(t.iter().all(|(_, x, y, _)| ((x * x + y * y).sqrt() - 50.0).abs() < 2.0));
        let (first, last) = (t.iter().find(|x| x.0 == "V").unwrap(), t.iter().find(|x| x.0 == "1").unwrap());
        assert!(first.1 < last.1);
        assert!(s.execute("civil.text.arc", &json!({ "text": "x" })).is_err());
        let ids: Vec<String> = (0..3)
            .map(|i| {
                s.execute("text", &json!({ "at": [0, -(i as i64) * 5], "text": format!("t{i}"), "height": 1 })).unwrap()["handle"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        s.execute("civil.text.number", &json!({ "handles": ids, "prefix": "P", "start": 7, "digits": 3, "order": "y" })).unwrap();
        let v: Vec<String> = texts(&s).into_iter().map(|x| x.0).filter(|v| v.starts_with('P')).collect();
        assert_eq!(v, ["P007", "P008", "P009"]);
        s.execute("civil.text.swap", &json!({ "handles": [ids[0].clone(), ids[1].clone()] })).unwrap();
        assert!(s.execute("civil.text.swap", &json!({ "handles": [ids[0].clone()] })).is_err());
        s.execute("civil.text.space", &json!({ "handles": ids, "spacing": 2 })).unwrap();
        let ys: Vec<f64> = texts(&s).into_iter().filter(|x| x.0.starts_with('P')).map(|x| x.2).collect();
        assert!(ys.iter().any(|y| (y - 0.0).abs() < 1e-9) && ys.iter().any(|y| (y + 4.0).abs() < 1e-9));
        let h = s.execute("civil.text.height", &json!({ "handles": ids, "scale": 500, "mm": 2.5 })).unwrap();
        assert!((h["height"].as_f64().unwrap() - 1.25).abs() < 1e-12);
        assert!(s.execute("civil.text.height", &json!({ "handles": ids })).is_err());
    }

    #[test]
    fn texts_round_trip_through_csv_and_plain_lists_stack_down() {
        let path = std::env::temp_dir().join(format!("civil-ing-{}-txt.csv", std::process::id()));
        let p = path.to_str().unwrap();
        let mut s = Session::new();
        s.execute("text", &json!({ "at": [3, 4], "text": "a, \"b\"", "height": 2 })).unwrap();
        assert_eq!(s.execute("civil.text.export", &json!({ "path": p })).unwrap()["count"], 1);
        let mut s2 = Session::new();
        assert_eq!(s2.execute("civil.text.import", &json!({ "path": p })).unwrap()["count"], 1);
        let t = texts(&s2);
        assert_eq!(t[0].0, "a, \"b\"");
        assert!((t[0].1 - 3.0).abs() < 1e-9 && (t[0].3 - 2.0).abs() < 1e-9);
        std::fs::write(&path, "uno\ndos\n").unwrap();
        let mut s3 = Session::new();
        s3.execute("civil.text.import", &json!({ "path": p, "at": [0, 10], "height": 2 })).unwrap();
        let ys: Vec<f64> = texts(&s3).into_iter().map(|x| x.2).collect();
        assert!(ys.contains(&10.0) && ys.contains(&7.0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_detail_is_listed_and_inserted_at_the_size_asked() {
        let mut s = Session::new();
        let empty = s.execute("civil.detail.list", &json!({})).unwrap();
        assert!(empty["details"].as_array().unwrap().is_empty());
        let l = s.execute("line", &json!({ "points": [[0, 0], [4, 0], [4, 2]] })).unwrap();
        s.execute("block", &json!({ "name": "CUNETA", "base": [0, 0], "handles": l["handles"].clone(), "description": "Cuneta triangular" }))
            .unwrap();
        let list = s.execute("civil.detail.list", &json!({})).unwrap();
        assert_eq!(list["details"][0]["name"], "CUNETA");
        assert!((list["details"][0]["width"].as_f64().unwrap() - 4.0).abs() < 1e-9);
        let r = s.execute("civil.detail.insert", &json!({ "name": "CUNETA", "at": [10, 10], "width": 8 })).unwrap();
        assert!((r["scale"].as_f64().unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(r["description"], "Cuneta triangular");
        let r = s.execute("civil.detail.insert", &json!({ "name": "CUNETA", "at": [0, 0], "paper_mm": 20, "scale": 100 })).unwrap();
        assert!((r["scale"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert!(s.execute("civil.detail.insert", &json!({ "name": "NOPE", "at": [0, 0] })).is_err());
    }
}
