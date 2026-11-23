//! Radiations: points computed from total-station observations.
//!
//! Each observation is made from a station of known coordinates, towards a point, as a horizontal
//! angle read from a backsight of known azimuth, a vertical (zenith) angle and a distance. The
//! columns of an observation file are named by their usual abbreviations:
//!
//! | Name | Meaning |
//! |---|---|
//! | `EST` | station name (defaults to the first station given) |
//! | `PV` | name of the point sighted |
//! | `ANGH` | horizontal angle, clockwise from the backsight |
//! | `ANGV` | vertical angle, measured from the zenith |
//! | `DINC` | slope distance |
//! | `DHOR` | horizontal distance (used instead of `DINC` and `ANGV`) |
//! | `DIFH` | height difference from the instrument axis to the target (instead of `ANGV`) |
//! | `HAP` | height of the target (prism) |
//! | `DESC` | description |
//!
//! Any other column name is ignored.

use crate::points::SurveyPoint;

/// How the angles of a file are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AngleUnit {
    /// Decimal degrees.
    #[default]
    Degrees,
    /// Degrees, minutes and seconds as `g.mmss` (for example `45.3015` is 45°30'15").
    Dms,
    /// Gradians (400 to the circle).
    Gradians,
}

impl AngleUnit {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "" | "deg" | "degrees" | "grados" | "decimal" => Self::Degrees,
            "dms" | "gms" | "sexagesimal" => Self::Dms,
            "grad" | "gon" | "gradians" | "centesimal" => Self::Gradians,
            _ => return None,
        })
    }

    /// An angle as written in the file, in degrees.
    pub fn to_degrees(self, v: f64) -> f64 {
        match self {
            Self::Degrees => v,
            Self::Gradians => v * 0.9,
            Self::Dms => {
                let sign = if v < 0.0 { -1.0 } else { 1.0 };
                let a = v.abs();
                let d = a.trunc();
                let rest = (a - d) * 100.0;
                let m = (rest + 1e-9).trunc();
                let sec = (rest - m) * 100.0;
                sign * (d + m / 60.0 + sec / 3600.0)
            }
        }
    }
}

/// A station of known position.
#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Height of the instrument above the station mark.
    pub hi: f64,
    /// Azimuth (degrees, clockwise from north) of the backsight, where the horizontal angle is 0.
    pub backsight_azimuth: f64,
}

/// One row of an observation file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observation {
    pub station: Option<String>,
    pub point: String,
    pub angh: f64,
    pub angv: Option<f64>,
    pub dinc: Option<f64>,
    pub dhor: Option<f64>,
    pub difh: Option<f64>,
    pub hap: f64,
    pub desc: String,
}

/// The points computed and the rows that could not be.
#[derive(Clone, Debug, Default)]
pub struct Radiated {
    pub points: Vec<SurveyPoint>,
    pub skipped: Vec<(usize, String)>,
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().trim_matches('"').replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn canonical(h: &str) -> Option<&'static str> {
    Some(match h.trim().to_uppercase().as_str() {
        "EST" | "ESTACION" | "STATION" | "ST" => "EST",
        "PV" | "PUNTO" | "P" | "POINT" => "PV",
        "ANGH" | "AH" | "HZ" => "ANGH",
        "ANGV" | "AV" | "ZENIT" | "ZENITH" => "ANGV",
        "DINC" | "DI" | "DIST" | "DISTANCIA" | "SLOPE" => "DINC",
        "DHOR" | "DH" | "HDIST" => "DHOR",
        "DIFH" | "DZ" | "DH_V" => "DIFH",
        "HAP" | "HP" | "HT" | "PRISMA" => "HAP",
        "DESC" | "D" | "CODIGO" | "DESCRIPCION" => "DESC",
        _ => return None,
    })
}

/// Read observations from text. `columns` names the columns in order (see the module docs);
/// without it the first row is read as the header when it has column names, and otherwise the
/// order `PV ANGH ANGV DINC HAP` is assumed. The angles are converted to degrees with `unit`.
pub fn parse_observations(text: &str, columns: Option<&str>, unit: AngleUnit) -> Result<(Vec<(usize, Observation)>, Vec<(usize, String)>), String> {
    let text = text.trim_start_matches('\u{feff}');
    let split = |l: &str| -> Vec<String> {
        let d = [';', '\t', ','].into_iter().find(|d| l.matches(*d).count() >= 2);
        match d {
            Some(d) => l.split(d).map(|c| c.trim().to_string()).collect(),
            None => l.split_whitespace().map(str::to_string).collect(),
        }
    };
    let mut cols: Option<Vec<Option<&'static str>>> = columns
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';').filter(|t| !t.is_empty()).map(canonical).collect());
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let cells = split(t);
        if cols.is_none() {
            let named: Vec<_> = cells.iter().map(|c| canonical(c)).collect();
            if named.iter().filter(|c| c.is_some()).count() >= 2 && named.contains(&Some("PV")) {
                cols = Some(named);
                continue;
            }
            cols = Some(vec![Some("PV"), Some("ANGH"), Some("ANGV"), Some("DINC"), Some("HAP")]);
        }
        let Some(cs) = cols.as_ref() else { continue };
        if !cs.contains(&Some("PV")) {
            return Err("the columns need PV (the point sighted)".into());
        }
        let mut o = Observation::default();
        let mut bad: Option<String> = None;
        for (c, cell) in cs.iter().zip(&cells) {
            let Some(name) = c else { continue };
            let n = num(cell);
            match *name {
                "EST" => o.station = Some(cell.trim().to_string()).filter(|s| !s.is_empty()),
                "PV" => o.point = cell.trim().to_string(),
                "DESC" => o.desc = cell.trim().to_string(),
                _ if cell.trim().is_empty() => {}
                other => match n {
                    None => bad = Some(format!("{other}: `{cell}` is not a number")),
                    Some(v) => match other {
                        "ANGH" => o.angh = unit.to_degrees(v),
                        "ANGV" => o.angv = Some(unit.to_degrees(v)),
                        "DINC" => o.dinc = Some(v),
                        "DHOR" => o.dhor = Some(v),
                        "DIFH" => o.difh = Some(v),
                        _ => o.hap = v,
                    },
                },
            }
        }
        match bad {
            Some(why) => skipped.push((i + 1, why)),
            None if o.point.is_empty() => skipped.push((i + 1, "no point name".into())),
            None => out.push((i + 1, o)),
        }
    }
    if out.is_empty() && skipped.is_empty() {
        return Err("the file has no observations".into());
    }
    Ok((out, skipped))
}

/// Compute the points of `obs` from `stations`.
pub fn radiate(stations: &[Station], obs: &[(usize, Observation)]) -> Radiated {
    let mut r = Radiated::default();
    for (row, o) in obs {
        let st = match &o.station {
            Some(id) => stations.iter().find(|s| s.id.eq_ignore_ascii_case(id)),
            None => stations.first(),
        };
        let Some(st) = st else {
            r.skipped.push((*row, format!("station `{}` is not defined", o.station.as_deref().unwrap_or(""))));
            continue;
        };
        let zen = o.angv.map(f64::to_radians);
        let hd = match (o.dhor, o.dinc, zen) {
            (Some(d), _, _) => d,
            (None, Some(d), Some(z)) => d * z.sin(),
            (None, Some(d), None) => d,
            _ => {
                r.skipped.push((*row, "needs DHOR, or DINC (with ANGV)".into()));
                continue;
            }
        };
        let dz = match (o.difh, o.dinc, zen) {
            (Some(d), _, _) => d,
            (None, Some(d), Some(z)) => d * z.cos(),
            _ => 0.0,
        };
        let az = (st.backsight_azimuth + o.angh).rem_euclid(360.0).to_radians();
        r.points.push(SurveyPoint {
            id: o.point.clone(),
            x: st.x + hd * az.sin(),
            y: st.y + hd * az.cos(),
            z: st.z + st.hi + dz - o.hap,
            code: o.desc.clone(),
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> Station {
        Station { id: "E1".into(), x: 1000.0, y: 2000.0, z: 100.0, hi: 1.5, backsight_azimuth: 0.0 }
    }

    #[test]
    fn a_sight_due_east_adds_to_x_and_the_height_follows_the_zenith_angle() {
        let (obs, skipped) = parse_observations("PV ANGH ANGV DINC HAP\nA 90 90 50 1.5\nB 180 80 100 1.5\n", None, AngleUnit::Degrees).unwrap();
        assert!(skipped.is_empty());
        let r = radiate(&[st()], &obs);
        let a = &r.points[0];
        assert!((a.x - 1050.0).abs() < 1e-9 && (a.y - 2000.0).abs() < 1e-9 && (a.z - 100.0).abs() < 1e-9);
        let b = &r.points[1];
        assert!((b.y - (2000.0 - 100.0 * 80f64.to_radians().sin())).abs() < 1e-9);
        assert!((b.z - (100.0 + 100.0 * 80f64.to_radians().cos())).abs() < 1e-9);
    }

    #[test]
    fn angles_read_as_dms_gradians_and_the_backsight_azimuth_turns_the_sights() {
        assert!((AngleUnit::Dms.to_degrees(45.3015) - (45.0 + 30.0 / 60.0 + 15.0 / 3600.0)).abs() < 1e-9);
        assert!((AngleUnit::Gradians.to_degrees(100.0) - 90.0).abs() < 1e-12);
        let mut s = st();
        s.backsight_azimuth = 90.0;
        let (obs, _) = parse_observations("EST,PV,ANGH,DHOR\nE1,A,0,10\n", None, AngleUnit::Degrees).unwrap();
        let p = &radiate(&[s], &obs).points[0];
        assert!((p.x - 1010.0).abs() < 1e-9 && (p.y - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn rows_that_cannot_be_computed_are_reported_not_dropped_silently() {
        let (obs, skipped) = parse_observations("PV ANGH ANGV DINC\nA 10 90 x\nB 10\n", None, AngleUnit::Degrees).unwrap();
        assert_eq!(skipped.len(), 1);
        let r = radiate(&[st()], &obs);
        assert!(r.points.is_empty() && r.skipped.len() == 1);
        let (obs, _) = parse_observations("EST PV DHOR\nZZ A 5\n", Some("EST PV DHOR"), AngleUnit::Degrees).unwrap();
        assert!(radiate(&[st()], &obs).skipped[0].1.contains("ZZ"));
    }
}
