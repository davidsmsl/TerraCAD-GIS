//! The construction table of a closed boundary: for every side its bearing, azimuth and length,
//! for every vertex its interior angle and coordinates, plus the area, and a table of curves for
//! the sides that are arcs.
//!
//! A boundary is a ring of vertices where each may carry a bulge (the polyline convention: the
//! tangent of a quarter of the arc's angle, positive when the arc turns counterclockwise).
//! Angles are in degrees, azimuths are clockwise from north.

/// One vertex of the boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub x: f64,
    pub y: f64,
    /// The bulge of the side that starts here and ends at the next vertex.
    pub bulge: f64,
}

/// How vertices are named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Naming {
    #[default]
    Numbers,
    /// A, B, ... Z, AA, AB, ...
    Letters,
}

/// Options of the table.
#[derive(Clone, Debug)]
pub struct Options {
    /// Index (0-based) of the vertex that is numbered first.
    pub start: usize,
    /// Number the vertices clockwise (as seen from above with north up); otherwise counterclockwise.
    pub clockwise: bool,
    pub naming: Naming,
}

impl Default for Options {
    fn default() -> Self {
        Options { start: 0, clockwise: true, naming: Naming::Numbers }
    }
}

/// One side of the boundary, from vertex `from` to vertex `to`.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub from: String,
    pub to: String,
    /// The azimuth of the chord.
    pub azimuth: f64,
    /// Length of the chord.
    pub distance: f64,
    /// Interior angle at `to`, measured between its two chords.
    pub angle: f64,
    pub x: f64,
    pub y: f64,
}

/// A side that is an arc.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub from: String,
    pub to: String,
    pub radius: f64,
    /// Central angle, degrees.
    pub delta: f64,
    pub length: f64,
    pub tangent: f64,
    pub chord: f64,
    pub chord_azimuth: f64,
    /// `true` when the curve turns right (clockwise) in the direction of the table.
    pub right: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub sides: Vec<Side>,
    pub curves: Vec<Curve>,
    pub area: f64,
    pub perimeter: f64,
    /// Whether the boundary as given runs clockwise.
    pub clockwise: bool,
}

/// The azimuth, in degrees from north clockwise, of the direction from `a` to `b`.
pub fn azimuth(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).atan2(b.1 - a.1).to_degrees().rem_euclid(360.0)
}

/// An angle in degrees as `g°mm'ss.ss"`, with `decimals` decimals in the seconds.
pub fn dms(deg: f64, decimals: usize) -> String {
    let scale = 10f64.powi(decimals as i32);
    let total = (deg.abs() * 3600.0 * scale).round() / scale;
    let d = (total / 3600.0).floor();
    let m = ((total - d * 3600.0) / 60.0).floor();
    let s = total - d * 3600.0 - m * 60.0;
    let w = if decimals == 0 { 2 } else { decimals + 3 };
    format!("{}{d:.0}°{m:02.0}'{s:0w$.decimals$}\"", if deg < 0.0 { "-" } else { "" })
}

/// A bearing (rumbo) such as `N 45°30'15" E` from an azimuth.
pub fn bearing(az: f64, decimals: usize) -> String {
    let a = az.rem_euclid(360.0);
    let (ns, e, ew) = if a <= 90.0 {
        ("N", a, "E")
    } else if a <= 180.0 {
        ("S", 180.0 - a, "E")
    } else if a <= 270.0 {
        ("S", a - 180.0, "W")
    } else {
        ("N", 360.0 - a, "W")
    };
    // Exactly north, south, east or west read as such.
    if e == 0.0 {
        return if a == 0.0 { "N".into() } else { "S".into() };
    }
    format!("{ns} {} {ew}", dms(e, decimals))
}

fn letters(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.iter().rev().collect()
}

fn name(i: usize, n: Naming) -> String {
    match n {
        Naming::Numbers => (i + 1).to_string(),
        Naming::Letters => letters(i),
    }
}

/// The signed area of the ring (counterclockwise positive), the arcs included.
fn signed_area(v: &[Vertex]) -> f64 {
    let n = v.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (v[i], v[(i + 1) % n]);
        a += (p.x * q.y - q.x * p.y) / 2.0;
        if p.bulge != 0.0 {
            let theta = 4.0 * p.bulge.atan();
            let c = ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
            let r = c / (2.0 * (theta.abs() / 2.0).sin());
            a += p.bulge.signum() * r * r / 2.0 * (theta.abs() - theta.abs().sin());
        }
    }
    a
}

/// Build the table of the ring `v` (at least 3 vertices, not repeating the first at the end).
pub fn table(v: &[Vertex], opts: &Options) -> Result<Table, String> {
    let mut ring: Vec<Vertex> = v.to_vec();
    while ring.len() > 3 && ring.first().zip(ring.last()).is_some_and(|(a, b)| a.x == b.x && a.y == b.y) {
        ring.pop();
    }
    let n = ring.len();
    if n < 3 {
        return Err("a boundary needs at least 3 vertices".into());
    }
    if ring.iter().any(|p| !(p.x.is_finite() && p.y.is_finite() && p.bulge.is_finite())) {
        return Err("the boundary has a coordinate that is not a number".into());
    }
    let signed = signed_area(&ring);
    if signed == 0.0 {
        return Err("the boundary encloses no area".into());
    }
    let clockwise = signed < 0.0;
    // Reverse to the asked direction; a bulge changes side and sign when the ring is reversed.
    let mut first = opts.start % n;
    if clockwise != opts.clockwise {
        first = n - 1 - first;
        let bulges: Vec<f64> = ring.iter().map(|p| p.bulge).collect();
        ring.reverse();
        for (i, p) in ring.iter_mut().enumerate() {
            // After reversing, the side leaving ring[i] is the old side entering it.
            let old_side = (n - 1 - i + n - 1) % n;
            p.bulge = -bulges[old_side];
        }
    }
    ring.rotate_left(first);
    let mut sides = Vec::with_capacity(n);
    let mut curves = Vec::new();
    let mut perimeter = 0.0;
    for i in 0..n {
        let (p, q, r) = (ring[i], ring[(i + 1) % n], ring[(i + 2) % n]);
        let az = azimuth((p.x, p.y), (q.x, q.y));
        let dist = ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
        let a_in = azimuth((q.x, q.y), (p.x, p.y));
        let a_out = azimuth((q.x, q.y), (r.x, r.y));
        let angle = if opts.clockwise { (a_in - a_out).rem_euclid(360.0) } else { (a_out - a_in).rem_euclid(360.0) };
        let (from, to) = (name(i, opts.naming), name((i + 1) % n, opts.naming));
        if p.bulge != 0.0 {
            let theta = 4.0 * p.bulge.atan();
            let radius = dist / (2.0 * (theta.abs() / 2.0).sin());
            let length = radius * theta.abs();
            perimeter += length;
            curves.push(Curve {
                from: from.clone(),
                to: to.clone(),
                radius,
                delta: theta.abs().to_degrees(),
                length,
                tangent: radius * (theta.abs() / 2.0).tan(),
                chord: dist,
                chord_azimuth: az,
                // A positive bulge turns counterclockwise, that is to the left.
                right: p.bulge < 0.0,
            });
        } else {
            perimeter += dist;
        }
        sides.push(Side { from, to, azimuth: az, distance: dist, angle, x: q.x, y: q.y });
    }
    Ok(Table { sides, curves, area: signed.abs(), perimeter, clockwise: opts.clockwise })
}

/// The sum the interior angles of a straight-sided ring must make: `(n - 2) * 180`.
pub fn angle_sum_error(t: &Table) -> f64 {
    t.sides.iter().map(|s| s.angle).sum::<f64>() - (t.sides.len() as f64 - 2.0) * 180.0
}

/// A hectare/square-metre area as text: `7,512.16 m²`, and hectares when it is large.
pub fn area_text(area: f64, decimals: usize) -> String {
    let m2 = group(area, decimals);
    if area >= 10_000.0 { format!("{m2} m² ({} ha)", group(area / 10_000.0, 4)) } else { format!("{m2} m²") }
}

fn group(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 && c != '-' {
            out.push(',');
        }
        out.push(c);
    }
    if frac.is_empty() { out } else { format!("{out}.{frac}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn v(x: f64, y: f64) -> Vertex {
        Vertex { x, y, bulge: 0.0 }
    }

    #[test]
    fn a_rectangle_has_square_corners_the_right_area_and_bearings() {
        // Counterclockwise as given: east, north, west, south.
        let ring = [v(0.0, 0.0), v(30.0, 0.0), v(30.0, 20.0), v(0.0, 20.0)];
        let t = table(&ring, &Options::default()).unwrap();
        assert!((t.area - 600.0).abs() < 1e-9 && (t.perimeter - 100.0).abs() < 1e-9);
        assert!(t.clockwise);
        assert!(t.sides.iter().all(|s| (s.angle - 90.0).abs() < 1e-9));
        assert!(angle_sum_error(&t).abs() < 1e-9);
        // Clockwise from vertex 1 (0,0): north first.
        assert_eq!(bearing(t.sides[0].azimuth, 0), "N");
        assert_eq!(t.sides[0].from, "1");
        assert!((t.sides[0].distance - 20.0).abs() < 1e-9);
    }

    #[test]
    fn counterclockwise_numbering_keeps_angles_interior() {
        let ring = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0)];
        let o = Options { clockwise: false, ..Options::default() };
        let t = table(&ring, &o).unwrap();
        assert!(angle_sum_error(&t).abs() < 1e-9);
        assert!((t.area - 50.0).abs() < 1e-9);
        let o = Options { start: 1, naming: Naming::Letters, ..Options::default() };
        let t = table(&ring, &o).unwrap();
        assert_eq!(t.sides[0].from, "A");
        assert!(angle_sum_error(&t).abs() < 1e-9);
    }

    #[test]
    fn bearings_use_quadrants_and_dms_rounds_the_seconds() {
        assert_eq!(bearing(45.5042, 0), "N 45°30'15\" E");
        assert_eq!(bearing(135.0, 0), "S 45°00'00\" E");
        assert_eq!(bearing(225.0, 1), "S 45°00'00.0\" W");
        assert_eq!(bearing(315.0, 0), "N 45°00'00\" W");
        assert_eq!(bearing(180.0, 0), "S");
        assert_eq!(dms(29.99999999, 0), "30°00'00\"");
        assert!((azimuth((0.0, 0.0), (1.0, 1.0)) - 45.0).abs() < 1e-9);
    }

    #[test]
    fn a_semicircle_side_adds_its_segment_and_is_listed_as_a_curve() {
        // A 20 x 10 rectangle whose top side bulges out as a half circle of radius 10.
        let ring = [v(0.0, 0.0), v(20.0, 0.0), Vertex { x: 20.0, y: 10.0, bulge: 1.0 }, v(0.0, 10.0)];
        // Bulge 1 turns counterclockwise (left) from (20,10) to (0,10): over the top. The ring runs
        // counterclockwise, so the arc is outward.
        let t = table(&ring, &Options { clockwise: false, ..Options::default() }).unwrap();
        assert!((t.area - (200.0 + PI * 100.0 / 2.0)).abs() < 1e-6, "{}", t.area);
        assert_eq!(t.curves.len(), 1);
        let c = &t.curves[0];
        assert!((c.radius - 10.0).abs() < 1e-9 && (c.delta - 180.0).abs() < 1e-9 && (c.length - PI * 10.0).abs() < 1e-9);
        assert!((t.perimeter - (20.0 + 10.0 + 10.0 + PI * 10.0)).abs() < 1e-9);
        // Numbered clockwise the same arc turns the other way round and keeps its radius.
        let t2 = table(&ring, &Options::default()).unwrap();
        assert!((t2.area - t.area).abs() < 1e-6 && (t2.curves[0].radius - 10.0).abs() < 1e-9);
        assert!(t2.curves[0].right && !t.curves[0].right);
    }

    #[test]
    fn degenerate_rings_are_refused_and_areas_are_grouped() {
        assert!(table(&[v(0.0, 0.0), v(1.0, 1.0)], &Options::default()).is_err());
        assert!(table(&[v(0.0, 0.0), v(1.0, 1.0), v(2.0, 2.0)], &Options::default()).is_err());
        assert_eq!(area_text(7512.161, 2), "7,512.16 m²");
        assert!(area_text(25_000.0, 2).contains("2.5000 ha"));
        assert_eq!(letters(26), "AA");
    }
}
