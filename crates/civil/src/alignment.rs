//! An alignment (eje): a polyline axis for a road, channel or pipe, with stations along it and
//! the ground profile below it.

use crate::tin::Tin;

/// A point on the axis with its station and the direction of travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub station: f64,
    pub x: f64,
    pub y: f64,
    /// Direction of the axis in radians (0 along +X, counter-clockwise).
    pub angle: f64,
}

/// A ground elevation at a station; `None` where the axis leaves the surveyed ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfilePoint {
    pub station: f64,
    pub x: f64,
    pub y: f64,
    pub ground: Option<f64>,
}

/// An axis as a polyline, with the length along it up to each vertex.
#[derive(Clone, Debug)]
pub struct Alignment {
    pts: Vec<[f64; 2]>,
    cum: Vec<f64>,
}

impl Alignment {
    /// An axis through `pts`; repeated points are dropped. `None` when fewer than two remain.
    pub fn new(pts: &[[f64; 2]]) -> Option<Alignment> {
        let mut v: Vec<[f64; 2]> = Vec::new();
        for p in pts.iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
            if v.last().is_none_or(|l| (l[0] - p[0]).hypot(l[1] - p[1]) > 1e-9) {
                v.push(*p);
            }
        }
        if v.len() < 2 {
            return None;
        }
        let mut cum = vec![0.0];
        for w in v.windows(2) {
            let last = cum.last().copied().unwrap_or(0.0);
            cum.push(last + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
        }
        Some(Alignment { pts: v, cum })
    }

    pub fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// The point at distance `d` from the start (clamped to the axis) and the direction there.
    pub fn at(&self, d: f64) -> ([f64; 2], f64) {
        let d = d.clamp(0.0, self.length());
        let i = self.cum.partition_point(|c| *c <= d).clamp(1, self.pts.len() - 1);
        let (a, b) = (self.pts[i - 1], self.pts[i]);
        let seg = (self.cum[i] - self.cum[i - 1]).max(f64::MIN_POSITIVE);
        let t = (d - self.cum[i - 1]) / seg;
        ([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])], (b[1] - a[1]).atan2(b[0] - a[0]))
    }

    /// Marks at the start, at every multiple of `interval` counted from station 0, and at the
    /// end. `start` is the station of the first point (for an axis that continues another).
    pub fn stations(&self, interval: f64, start: f64) -> Result<Vec<Mark>, String> {
        if !(interval.is_finite() && interval > 0.0) {
            return Err("the station interval must be a number above zero".into());
        }
        let len = self.length();
        if len / interval > 200_000.0 {
            return Err("too many stations: use a larger interval".into());
        }
        let mut ds = vec![0.0];
        let mut k = (start / interval).floor() + 1.0;
        loop {
            let d = k * interval - start;
            if d >= len - 1e-9 {
                break;
            }
            if d > 1e-9 {
                ds.push(d);
            }
            k += 1.0;
        }
        ds.push(len);
        Ok(ds
            .into_iter()
            .map(|d| {
                let (p, angle) = self.at(d);
                Mark { station: start + d, x: p[0], y: p[1], angle }
            })
            .collect())
    }

    /// The ground profile: elevations at every station of `step`, plus the vertices of the axis.
    pub fn profile(&self, tin: &Tin, step: f64, start: f64) -> Result<Vec<ProfilePoint>, String> {
        let mut marks = self.stations(step, start)?;
        for (p, c) in self.pts.iter().zip(&self.cum) {
            marks.push(Mark { station: start + c, x: p[0], y: p[1], angle: 0.0 });
        }
        marks.sort_by(|a, b| a.station.total_cmp(&b.station));
        marks.dedup_by(|a, b| (a.station - b.station).abs() < 1e-6);
        Ok(marks.into_iter().map(|m| ProfilePoint { station: m.station, x: m.x, y: m.y, ground: tin.z_at(m.x, m.y) }).collect())
    }
}

/// A station as surveyors write it: kilometres, a plus sign and the metres, as in `1+250.00`.
pub fn format_station(s: f64) -> String {
    let km = (s / 1000.0).floor();
    let m = s - km * 1000.0;
    // Rounding can carry 999.995 up to 1000.00: carry into the kilometre.
    if (m * 100.0).round() >= 100_000.0 { format!("{}+{:06.2}", km + 1.0, 0.0) } else { format!("{km}+{m:06.2}") }
}

/// Polyline vertices `(x, y, bulge)` as plain points, with each arc segment (non-zero bulge)
/// replaced by points no more than 5° apart.
pub fn flatten_bulges(vs: &[(f64, f64, f64)], closed: bool) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let n = vs.len();
    for (i, &(x, y, bulge)) in vs.iter().enumerate() {
        out.push([x, y]);
        let next = if i + 1 < n {
            vs.get(i + 1)
        } else if closed {
            vs.first()
        } else {
            None
        };
        let Some(&(x2, y2, _)) = next else { continue };
        if bulge.abs() < 1e-12 || !bulge.is_finite() {
            continue;
        }
        let (dx, dy) = (x2 - x, y2 - y);
        let l = dx.hypot(dy);
        if l < 1e-12 {
            continue;
        }
        let theta = 4.0 * bulge.atan();
        let off = (l / 2.0) / (theta / 2.0).tan();
        let (cx, cy) = ((x + x2) / 2.0 - dy / l * off, (y + y2) / 2.0 + dx / l * off);
        let steps = (theta.abs() / (std::f64::consts::PI / 36.0)).ceil().max(1.0) as usize;
        for k in 1..steps {
            let a = theta * k as f64 / steps as f64;
            let (s, c) = a.sin_cos();
            let (rx, ry) = (x - cx, y - cy);
            out.push([cx + rx * c - ry * s, cy + rx * s + ry * c]);
        }
    }
    if closed && let Some(f) = out.first().copied() {
        out.push(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::SurveyPoint;

    fn axis() -> Alignment {
        Alignment::new(&[[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]]).unwrap()
    }

    #[test]
    fn stations_fall_on_multiples_and_include_both_ends() {
        let m = axis().stations(20.0, 0.0).unwrap();
        let st: Vec<f64> = m.iter().map(|m| m.station).collect();
        assert_eq!(st, vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 140.0, 150.0]);
        assert_eq!((m[6].x, m[6].y), (100.0, 20.0));
        assert!((m[1].angle).abs() < 1e-12 && (m[6].angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn a_starting_station_shifts_the_multiples() {
        let m = axis().stations(20.0, 5.0).unwrap();
        assert_eq!(m.iter().map(|m| m.station).take(3).collect::<Vec<_>>(), vec![5.0, 20.0, 40.0]);
        assert!((m[1].x - 15.0).abs() < 1e-12);
    }

    #[test]
    fn stations_are_written_with_kilometres_and_metres() {
        assert_eq!(format_station(0.0), "0+000.00");
        assert_eq!(format_station(1250.5), "1+250.50");
        assert_eq!(format_station(999.999), "1+000.00");
    }

    #[test]
    fn a_half_circle_bulge_becomes_points_on_the_circle() {
        let pts = flatten_bulges(&[(0.0, 0.0, 1.0), (10.0, 0.0, 0.0)], false);
        assert!(pts.len() > 30);
        for p in &pts {
            assert!(((p[0] - 5.0).hypot(p[1]) - 5.0).abs() < 1e-9);
        }
        assert!(pts.iter().all(|p| p[1] <= 1e-9), "a positive bulge is a counter-clockwise arc: this one dips below the chord");
    }

    #[test]
    fn the_profile_reads_the_ground_along_the_axis() {
        let pts: Vec<SurveyPoint> = (0..=12)
            .flat_map(|i| {
                (0..=7).map(move |j| SurveyPoint {
                    id: String::new(),
                    x: f64::from(i) * 10.0,
                    y: f64::from(j) * 10.0,
                    z: f64::from(i),
                    code: String::new(),
                })
            })
            .collect();
        let tin = Tin::build(&pts, None).unwrap();
        let p = Alignment::new(&[[5.0, 5.0], [95.0, 5.0], [200.0, 5.0]]).unwrap().profile(&tin, 10.0, 0.0).unwrap();
        assert!((p[0].ground.unwrap() - 0.5).abs() < 1e-9);
        assert!(p.last().unwrap().ground.is_none(), "the end of the axis is past the survey");
        assert!(p.windows(2).all(|w| w[0].station < w[1].station));
    }

    #[test]
    fn a_degenerate_axis_or_interval_is_refused() {
        assert!(Alignment::new(&[[1.0, 1.0], [1.0, 1.0]]).is_none());
        assert!(axis().stations(0.0, 0.0).is_err());
    }
}
