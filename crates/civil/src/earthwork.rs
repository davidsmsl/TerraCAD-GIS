//! Design grade, cross-section areas, earthwork volumes and the mass curve.
//!
//! The design grade (rasante) is given by its points of vertical intersection, each with the
//! length of the parabolic vertical curve there (0 for a sharp break). Along an axis, at each
//! station, the ground across the axis is compared with a template (a level or crowned surface
//! at the grade elevation, `half_width` to each side) to get the area of cut and of fill.
//! Volumes follow by the average end area method, and the mass curve accumulates the cut minus
//! the fill, the fill multiplied by a factor for shrinkage or swell.

use crate::alignment::Alignment;
use crate::tin::Tin;

/// A point of vertical intersection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pvi {
    pub station: f64,
    pub elevation: f64,
    /// Length of the vertical curve centred on this point; 0 for none.
    pub curve: f64,
}

/// A design grade.
#[derive(Clone, Debug, PartialEq)]
pub struct Grade {
    pvis: Vec<Pvi>,
}

/// What an annotation of a vertical curve needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveInfo {
    pub bvc: f64,
    pub evc: f64,
    pub bvc_elevation: f64,
    pub evc_elevation: f64,
    /// Grade in, grade out (metres per metre).
    pub g1: f64,
    pub g2: f64,
    /// Length per percent of algebraic grade change (the K value).
    pub k: f64,
    /// The station and elevation of the highest or lowest point of the curve, when it has one.
    pub extreme: Option<(f64, f64)>,
}

impl Grade {
    pub fn new(pvis: Vec<Pvi>) -> Result<Grade, String> {
        if pvis.len() < 2 {
            return Err("the grade needs at least 2 points (station, elevation)".into());
        }
        if pvis.iter().any(|p| !(p.station.is_finite() && p.elevation.is_finite() && p.curve.is_finite() && p.curve >= 0.0)) {
            return Err("the grade has a value that is not a number (or a negative curve length)".into());
        }
        if pvis.windows(2).any(|w| w[1].station <= w[0].station) {
            return Err("the stations of the grade must increase".into());
        }
        let n = pvis.len();
        for (i, p) in pvis.iter().enumerate() {
            if p.curve > 0.0 {
                if i == 0 || i == n - 1 {
                    return Err("the first and last points of the grade cannot have a curve".into());
                }
                let room = (p.station - pvis[i - 1].station).min(pvis[i + 1].station - p.station) * 2.0;
                if p.curve > room + 1e-9 {
                    return Err(format!("the curve at station {} is longer than the room it has ({room:.2})", p.station));
                }
            }
        }
        // Neighbouring curves must not overlap.
        for w in pvis.windows(2) {
            if w[0].curve / 2.0 + w[1].curve / 2.0 > w[1].station - w[0].station + 1e-9 {
                return Err(format!("the curves at stations {} and {} overlap", w[0].station, w[1].station));
            }
        }
        Ok(Grade { pvis })
    }

    pub fn start(&self) -> f64 {
        self.pvis[0].station
    }

    pub fn end(&self) -> f64 {
        self.pvis[self.pvis.len() - 1].station
    }

    pub fn pvis(&self) -> &[Pvi] {
        &self.pvis
    }

    fn slope(&self, i: usize) -> f64 {
        let (a, b) = (self.pvis[i], self.pvis[i + 1]);
        (b.elevation - a.elevation) / (b.station - a.station)
    }

    /// The elevation of the grade at `station`; `None` outside it.
    pub fn elevation(&self, station: f64) -> Option<f64> {
        if station < self.start() - 1e-9 || station > self.end() + 1e-9 {
            return None;
        }
        let n = self.pvis.len();
        for i in 1..n - 1 {
            if let Some(c) = self.curve_info(i)
                && station >= c.bvc
                && station <= c.evc
            {
                let (x, l) = (station - c.bvc, c.evc - c.bvc);
                return Some(c.bvc_elevation + c.g1 * x + (c.g2 - c.g1) / (2.0 * l) * x * x);
            }
        }
        let i = (0..n - 1).find(|&i| station <= self.pvis[i + 1].station + 1e-12).unwrap_or(n - 2);
        Some(self.pvis[i].elevation + self.slope(i) * (station - self.pvis[i].station))
    }

    /// The curve at point `i`, when it has one.
    pub fn curve_info(&self, i: usize) -> Option<CurveInfo> {
        let p = *self.pvis.get(i)?;
        if p.curve <= 0.0 || i == 0 || i + 1 >= self.pvis.len() {
            return None;
        }
        let (g1, g2) = (self.slope(i - 1), self.slope(i));
        let (bvc, evc) = (p.station - p.curve / 2.0, p.station + p.curve / 2.0);
        let bvc_elevation = p.elevation - g1 * p.curve / 2.0;
        let evc_elevation = p.elevation + g2 * p.curve / 2.0;
        let a = (g2 - g1) * 100.0;
        let k = if a.abs() > 1e-12 { p.curve / a.abs() } else { f64::INFINITY };
        // The turning point is where the grade crosses zero: x = -g1 * L / (g2 - g1).
        let extreme = (g1 * g2 < 0.0).then(|| {
            let x = -g1 * p.curve / (g2 - g1);
            (bvc + x, bvc_elevation + g1 * x + (g2 - g1) / (2.0 * p.curve) * x * x)
        });
        Some(CurveInfo { bvc, evc, bvc_elevation, evc_elevation, g1, g2, k, extreme })
    }
}

/// Parse points of vertical intersection from text: `station,elevation[,curve]` separated by
/// semicolons or new lines.
pub fn parse_pvis(text: &str) -> Result<Vec<Pvi>, String> {
    let mut out = Vec::new();
    for part in text.split([';', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
        let nums: Vec<f64> = part
            .split([',', ' ', '\t'])
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f64>().map_err(|_| format!("`{part}`: `{t}` is not a number")))
            .collect::<Result<_, _>>()?;
        match nums.as_slice() {
            [s, z] => out.push(Pvi { station: *s, elevation: *z, curve: 0.0 }),
            [s, z, l] => out.push(Pvi { station: *s, elevation: *z, curve: *l }),
            _ => return Err(format!("`{part}`: write station, elevation and optionally the curve length")),
        }
    }
    Ok(out)
}

/// The areas of cut and fill of a section: the ground at offsets from `-half_width` (left) to
/// `half_width` against a template at `design` elevation that falls away from the axis at
/// `crossfall` (metres per metre) on each side. Steps with no ground at either end are skipped.
pub fn section_areas(ground: &dyn Fn(f64) -> Option<f64>, design: f64, half_width: f64, crossfall: f64, step: f64) -> (f64, f64) {
    let n = ((2.0 * half_width / step).ceil() as usize).clamp(1, 20_000);
    let w = 2.0 * half_width / n as f64;
    let diff = |o: f64| ground(o).map(|z| z - (design - crossfall * o.abs()));
    let (mut cut, mut fill) = (0.0, 0.0);
    let mut prev = diff(-half_width);
    for i in 1..=n {
        let o = -half_width + w * i as f64;
        let cur = diff(o);
        if let (Some(d0), Some(d1)) = (prev, cur) {
            if d0 >= 0.0 && d1 >= 0.0 {
                cut += (d0 + d1) / 2.0 * w;
            } else if d0 <= 0.0 && d1 <= 0.0 {
                fill += (-d0 - d1) / 2.0 * w;
            } else {
                let t = d0 / (d0 - d1);
                let (pos, neg) = if d0 > 0.0 { (d0 * t * w / 2.0, -d1 * (1.0 - t) * w / 2.0) } else { (d1 * (1.0 - t) * w / 2.0, -d0 * t * w / 2.0) };
                cut += pos;
                fill += neg;
            }
        }
        prev = cur;
    }
    (cut, fill)
}

/// One station of the earthwork table.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub station: f64,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    pub ground: f64,
    pub grade: f64,
    /// Ground minus grade at the axis: positive is cut.
    pub depth: f64,
    pub cut_area: f64,
    pub fill_area: f64,
    /// Volumes from the previous row to this one.
    pub cut_volume: f64,
    pub fill_volume: f64,
    /// Cumulative cut minus `factor` times fill.
    pub mass: f64,
}

/// Options of [`earthwork`].
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub start: f64,
    pub interval: f64,
    pub half_width: f64,
    pub crossfall: f64,
    pub step: f64,
    pub factor: f64,
}

/// The earthwork table along `axis` where the grade, the survey and the axis all exist.
/// Returns the rows and the number of stations left out for lacking one of them.
pub fn earthwork(axis: &Alignment, tin: &Tin, grade: &Grade, p: &Params) -> Result<(Vec<Row>, usize), String> {
    if !(p.interval > 0.0 && p.half_width > 0.0 && p.step > 0.0 && p.factor > 0.0) {
        return Err("interval, half width, step and factor must be above zero".into());
    }
    let marks = axis.stations(p.interval, p.start)?;
    let mut rows: Vec<Row> = Vec::new();
    let mut left_out = 0;
    for m in marks {
        let (Some(g), Some(z)) = (grade.elevation(m.station), tin.z_at(m.x, m.y)) else {
            left_out += 1;
            continue;
        };
        // Offsets are positive to the left of the direction of travel.
        let (nx, ny) = (-m.angle.sin(), m.angle.cos());
        let ground = |o: f64| tin.z_at(m.x + nx * o, m.y + ny * o);
        let (cut_area, fill_area) = section_areas(&ground, g, p.half_width, p.crossfall, p.step);
        let (cut_volume, fill_volume, mass) = match rows.last() {
            Some(prev) => {
                let d = m.station - prev.station;
                let (c, f) = ((prev.cut_area + cut_area) / 2.0 * d, (prev.fill_area + fill_area) / 2.0 * d);
                (c, f, prev.mass + c - p.factor * f)
            }
            None => (0.0, 0.0, 0.0),
        };
        rows.push(Row {
            station: m.station,
            x: m.x,
            y: m.y,
            angle: m.angle,
            ground: z,
            grade: g,
            depth: z - g,
            cut_area,
            fill_area,
            cut_volume,
            fill_volume,
            mass,
        });
    }
    if rows.len() < 2 {
        return Err(
            "fewer than 2 stations have ground and grade: check that the axis lies inside the survey and the grade covers its stations".into()
        );
    }
    Ok((rows, left_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::SurveyPoint;

    fn pvi(s: f64, z: f64, l: f64) -> Pvi {
        Pvi { station: s, elevation: z, curve: l }
    }

    #[test]
    fn the_grade_follows_tangents_and_a_parabolic_curve_through_its_ends() {
        let g = Grade::new(vec![pvi(0.0, 100.0, 0.0), pvi(100.0, 102.0, 40.0), pvi(200.0, 100.0, 0.0)]).unwrap();
        assert!((g.elevation(0.0).unwrap() - 100.0).abs() < 1e-9);
        assert!((g.elevation(50.0).unwrap() - 101.0).abs() < 1e-9);
        let c = g.curve_info(1).unwrap();
        assert!((c.bvc - 80.0).abs() < 1e-9 && (c.evc - 120.0).abs() < 1e-9);
        assert!((g.elevation(80.0).unwrap() - 101.6).abs() < 1e-9);
        // At the point of intersection the curve is half way between it and the chord: e = A L / 800.
        let a = (c.g2 - c.g1) * 100.0;
        assert!((g.elevation(100.0).unwrap() - (102.0 + a * 40.0 / 800.0)).abs() < 1e-9);
        assert!((c.k - 40.0 / 4.0).abs() < 1e-9);
        let (xs, zs) = c.extreme.unwrap();
        assert!((xs - 100.0).abs() < 1e-9 && (zs - g.elevation(xs).unwrap()).abs() < 1e-9);
        assert!(g.elevation(-1.0).is_none() && g.elevation(201.0).is_none());
    }

    #[test]
    fn bad_grades_are_refused_with_a_reason() {
        assert!(Grade::new(vec![pvi(0.0, 1.0, 0.0)]).is_err());
        assert!(Grade::new(vec![pvi(10.0, 1.0, 0.0), pvi(5.0, 1.0, 0.0)]).is_err());
        assert!(Grade::new(vec![pvi(0.0, 1.0, 0.0), pvi(10.0, 1.0, 30.0), pvi(20.0, 1.0, 0.0)]).unwrap_err().contains("longer"));
        assert!(Grade::new(vec![pvi(0.0, 1.0, 10.0), pvi(10.0, 1.0, 0.0)]).is_err());
        assert_eq!(parse_pvis("0,100; 100,102,40").unwrap().len(), 2);
        assert!(parse_pvis("0,abc").is_err() && parse_pvis("1").is_err());
    }

    #[test]
    fn a_section_splits_into_cut_and_fill_where_the_ground_crosses_the_template() {
        // Ground rises 0.5 m per metre of offset; template level at 0: cut on the right, fill on the left.
        let ground = |o: f64| Some(0.5 * o);
        let (cut, fill) = section_areas(&ground, 0.0, 4.0, 0.0, 0.5);
        assert!((cut - 4.0).abs() < 1e-9 && (fill - 4.0).abs() < 1e-9, "{cut} {fill}");
        // A flat ground 1 m above the template is all cut.
        let (cut, fill) = section_areas(&|_| Some(1.0), 0.0, 5.0, 0.0, 1.0);
        assert!((cut - 10.0).abs() < 1e-9 && fill == 0.0);
        // The crossfall lowers the template away from the axis.
        let (cut, _) = section_areas(&|_| Some(0.0), 0.0, 5.0, 0.02, 1.0);
        assert!((cut - 2.0 * 0.02 * 5.0 * 5.0 / 2.0).abs() < 1e-9);
        assert_eq!(section_areas(&|_| None, 0.0, 5.0, 0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn volumes_between_stations_follow_the_average_end_area_and_the_mass_curve_accumulates() {
        // A plane rising 1 m every 10 m along x; the axis runs along x; the grade is level at 5.
        let mut pts = Vec::new();
        for i in 0..=20 {
            for j in -5..=5 {
                pts.push(SurveyPoint {
                    id: String::new(),
                    x: f64::from(i) * 5.0,
                    y: f64::from(j) * 2.0 + f64::from(i) * 1e-4,
                    z: f64::from(i) * 0.5,
                    code: String::new(),
                });
            }
        }
        let tin = Tin::build(&pts, None).unwrap();
        let axis = Alignment::new(&[[0.0, 0.0], [100.0, 0.0]]).unwrap();
        let grade = Grade::new(vec![pvi(0.0, 5.0, 0.0), pvi(100.0, 5.0, 0.0)]).unwrap();
        let p = Params { start: 0.0, interval: 20.0, half_width: 4.0, crossfall: 0.0, step: 1.0, factor: 1.0 };
        let (rows, out) = earthwork(&axis, &tin, &grade, &p).unwrap();
        assert_eq!(out, 0);
        assert_eq!(rows.len(), 6);
        // The ground is at 5 m half way between stations 40 and 60: fill before, cut after.
        assert!(rows[2].cut_area.abs() < 1e-6 && (rows[2].fill_area - 8.0).abs() < 1e-6);
        assert!((rows[3].cut_area - 8.0).abs() < 1e-6 && rows[3].fill_area.abs() < 1e-6);
        assert!((rows[5].cut_area - 8.0 * 5.0).abs() < 1e-6, "{}", rows[5].cut_area);
        assert!((rows[0].fill_area - 8.0 * 5.0).abs() < 1e-6);
        // Symmetric cut and fill: the mass curve returns to zero.
        assert!(rows.last().unwrap().mass.abs() < 1e-6, "{}", rows.last().unwrap().mass);
        assert!(rows[1].fill_volume > 0.0 && rows[4].cut_volume > 0.0);
        assert!((rows[1].depth - (rows[1].ground - 5.0)).abs() < 1e-12);
        let bad = Params { interval: 0.0, ..p };
        assert!(earthwork(&axis, &tin, &grade, &bad).is_err());
    }
}
