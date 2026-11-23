//! Platforms (pads): a closed outline at one elevation with side slopes that meet the ground.
//!
//! Outside the outline the finished surface rises (where the ground is above the platform, a
//! cut) or falls (where it is below, a fill) at the given slope, `H` metres horizontal for each
//! metre of height. Optionally the slope is cut into benches (bermas): after every `bench_height`
//! of height a level berm of `berm_width` is left. The finished surface at a point outside the
//! outline depends on its distance to the outline, so corners are rounded, as a slope cut by a
//! rolling cone would be.
//!
//! From it come the daylight line (where the slopes meet the ground), the zero line (the ground
//! contour at the platform elevation), and the volumes of cut and fill by sections at regular
//! intervals, by the average end area or the prismoidal formula.

use crate::contour::contours;
use crate::earthwork::section_areas;
use crate::tin::Tin;

/// A platform.
#[derive(Clone, Debug)]
pub struct Pad {
    ring: Vec<[f64; 2]>,
    pub elevation: f64,
    /// Horizontal metres per metre of height, in cut.
    pub cut_slope: f64,
    /// The same in fill.
    pub fill_slope: f64,
    /// Height between berms; 0 for a slope without them.
    pub bench_height: f64,
    pub berm_width: f64,
}

/// A point of the daylight line with the platform edge point it comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayPoint {
    pub edge: [f64; 2],
    pub pt: [f64; 2],
    pub z: f64,
    pub cut: bool,
}

/// How the volumes are found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Method {
    Average,
    #[default]
    Prismoidal,
}

#[derive(Clone, Copy, Debug)]
pub struct VolumeOptions {
    /// Spacing of the sections.
    pub interval: f64,
    /// Direction the sections advance in, degrees from +X counterclockwise.
    pub angle: f64,
    /// Sampling step along a section.
    pub step: f64,
    pub method: Method,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volumes {
    pub cut: f64,
    pub fill: f64,
    pub sections: usize,
}

impl Pad {
    pub fn new(ring: &[[f64; 2]], elevation: f64, cut_slope: f64, fill_slope: f64, bench_height: f64, berm_width: f64) -> Result<Pad, String> {
        let mut r: Vec<[f64; 2]> = ring.to_vec();
        while r.len() > 3 && r.first() == r.last() {
            r.pop();
        }
        if r.len() < 3 {
            return Err("a platform needs a closed outline of at least 3 vertices".into());
        }
        if r.iter().any(|p| !(p[0].is_finite() && p[1].is_finite())) || !elevation.is_finite() {
            return Err("the platform has a value that is not a number".into());
        }
        if !(cut_slope > 0.0 && fill_slope > 0.0 && cut_slope.is_finite() && fill_slope.is_finite()) {
            return Err("the slopes (horizontal per vertical) must be above zero".into());
        }
        if bench_height < 0.0 || berm_width < 0.0 || !bench_height.is_finite() || !berm_width.is_finite() {
            return Err("the bench height and berm width cannot be negative".into());
        }
        // Counterclockwise, so the outside is to the right of every edge.
        let area: f64 = (0..r.len()).map(|i| r[i][0] * r[(i + 1) % r.len()][1] - r[(i + 1) % r.len()][0] * r[i][1]).sum();
        if area == 0.0 {
            return Err("the platform outline encloses no area".into());
        }
        if area < 0.0 {
            r.reverse();
        }
        Ok(Pad { ring: r, elevation, cut_slope, fill_slope, bench_height, berm_width })
    }

    pub fn ring(&self) -> &[[f64; 2]] {
        &self.ring
    }

    pub fn area(&self) -> f64 {
        let n = self.ring.len();
        (0..n).map(|i| self.ring[i][0] * self.ring[(i + 1) % n][1] - self.ring[(i + 1) % n][0] * self.ring[i][1]).sum::<f64>() / 2.0
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let n = self.ring.len();
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (self.ring[i], self.ring[j]);
            if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// Distance from the point to the outline.
    pub fn distance(&self, x: f64, y: f64) -> f64 {
        let n = self.ring.len();
        let mut best = f64::MAX;
        for i in 0..n {
            let (a, b) = (self.ring[i], self.ring[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - a[0]) * dx + (y - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            best = best.min((x - a[0] - t * dx).hypot(y - a[1] - t * dy));
        }
        best
    }

    /// The height of the slope plane at distance `d` from the outline, benches included.
    pub fn rise(&self, slope: f64, d: f64) -> f64 {
        if self.bench_height <= 0.0 || self.berm_width <= 0.0 {
            return d / slope;
        }
        let run = self.bench_height * slope;
        let period = run + self.berm_width;
        let k = (d / period).floor();
        let r = d - k * period;
        k * self.bench_height + (r / slope).min(self.bench_height)
    }

    /// The finished elevation where the ground is `ground`, at `(x, y)`.
    pub fn finished(&self, x: f64, y: f64, ground: f64) -> f64 {
        if self.contains(x, y) {
            return self.elevation;
        }
        let d = self.distance(x, y);
        let up = self.elevation + self.rise(self.cut_slope, d);
        if ground > up {
            return up;
        }
        let down = self.elevation - self.rise(self.fill_slope, d);
        if ground < down {
            return down;
        }
        ground
    }

    /// How far a slope can reach over `tin` before it is certainly above or below all the ground.
    fn reach(&self, tin: &Tin) -> f64 {
        let (lo, hi) = tin.z_range();
        let dz = (hi - self.elevation).abs().max((lo - self.elevation).abs()) + 1.0;
        let berm = if self.bench_height > 0.0 { self.berm_width / self.bench_height } else { 0.0 };
        dz * (self.cut_slope.max(self.fill_slope) + berm) + 1.0
    }

    /// The daylight line: for points every `along` metres of the outline, the place along the
    /// outward normal where the slope meets the ground. A run ends where there is no ground or
    /// no meeting within reach.
    pub fn daylight(&self, tin: &Tin, along: f64, march: f64) -> Vec<Vec<DayPoint>> {
        let reach = self.reach(tin);
        let n = self.ring.len();
        let mut runs: Vec<Vec<DayPoint>> = Vec::new();
        let mut run: Vec<DayPoint> = Vec::new();
        let mut samples: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for i in 0..n {
            let (a, b) = (self.ring[i], self.ring[(i + 1) % n]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len == 0.0 {
                continue;
            }
            // Counterclockwise outline: the outside is to the right of the direction.
            let nrm = [(b[1] - a[1]) / len, -(b[0] - a[0]) / len];
            let k = ((len / along).ceil() as usize).clamp(1, 20_000);
            for j in 0..k {
                let t = j as f64 / k as f64;
                samples.push(([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], nrm));
            }
        }
        for (p, nrm) in samples {
            match self.meet(tin, p, nrm, reach, march) {
                Some(d) => run.push(d),
                None => {
                    if run.len() > 1 {
                        runs.push(std::mem::take(&mut run));
                    }
                    run.clear();
                }
            }
        }
        if run.len() > 1 {
            runs.push(run);
        }
        runs
    }

    fn meet(&self, tin: &Tin, p: [f64; 2], nrm: [f64; 2], reach: f64, march: f64) -> Option<DayPoint> {
        let g0 = tin.z_at(p[0], p[1])?;
        let cut = g0 >= self.elevation;
        // Positive while the ground is on the far side of the slope plane (still to be cut or filled).
        let f = |d: f64| -> Option<f64> {
            let g = tin.z_at(p[0] + nrm[0] * d, p[1] + nrm[1] * d)?;
            Some(if cut { g - (self.elevation + self.rise(self.cut_slope, d)) } else { (self.elevation - self.rise(self.fill_slope, d)) - g })
        };
        let mut prev = (0.0, f(0.0)?);
        let mut d = 0.0;
        while d < reach {
            d = (d + march).min(reach);
            let v = f(d)?;
            if v <= 0.0 {
                let (mut lo, mut hi) = (prev.0, d);
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if f(mid)? > 0.0 { lo = mid } else { hi = mid }
                }
                let dd = (lo + hi) / 2.0;
                let q = [p[0] + nrm[0] * dd, p[1] + nrm[1] * dd];
                return Some(DayPoint { edge: p, pt: q, z: tin.z_at(q[0], q[1])?, cut });
            }
            prev = (d, v);
        }
        None
    }

    /// The zero line: where the ground is at the platform elevation, as polylines.
    pub fn zero_line(&self, tin: &Tin) -> Vec<(Vec<[f64; 2]>, bool)> {
        contours(tin, 1e9, self.elevation)
            .map(|cs| cs.into_iter().filter(|c| (c.level - self.elevation).abs() < 1e-6).map(|c| (c.pts, c.closed)).collect())
            .unwrap_or_default()
    }

    /// The volumes of cut and fill between the ground and the finished surface.
    pub fn volumes(&self, tin: &Tin, o: &VolumeOptions) -> Result<Volumes, String> {
        if !(o.interval > 0.0 && o.step > 0.0 && o.interval.is_finite() && o.step.is_finite()) {
            return Err("the section interval and step must be above zero".into());
        }
        let reach = self.reach(tin);
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &self.ring {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let th = o.angle.to_radians();
        let (u, v) = ([th.cos(), th.sin()], [-th.sin(), th.cos()]);
        // Extent of the outline along u and v from its centre, plus the reach of the slopes.
        let ext = |dir: [f64; 2]| self.ring.iter().map(|p| ((p[0] - c[0]) * dir[0] + (p[1] - c[1]) * dir[1]).abs()).fold(0.0, f64::max) + reach;
        let (eu, ev) = (ext(u), ext(v));
        let nsec = ((2.0 * eu / o.interval).ceil() as usize).max(1);
        let per = ((2.0 * ev / o.step).ceil() as usize).max(1);
        let work = (nsec + 1) * per * if o.method == Method::Prismoidal { 2 } else { 1 };
        if work > 4_000_000 {
            return Err(format!("{work} samples: use a larger section interval or step"));
        }
        let area = |t: f64| -> (f64, f64) {
            let ground_diff = |off: f64| -> Option<f64> {
                let (x, y) = (c[0] + u[0] * t + v[0] * off, c[1] + u[1] * t + v[1] * off);
                let g = tin.z_at(x, y)?;
                Some(g - self.finished(x, y, g))
            };
            section_areas(&ground_diff, 0.0, ev, 0.0, o.step)
        };
        let dt = 2.0 * eu / nsec as f64;
        let ts: Vec<f64> = (0..=nsec).map(|i| -eu + dt * i as f64).collect();
        let areas: Vec<(f64, f64)> = ts.iter().map(|t| area(*t)).collect();
        let (mut cut, mut fill) = (0.0, 0.0);
        for (i, w) in areas.windows(2).enumerate() {
            let (c1, f1) = match o.method {
                Method::Average => ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0),
                Method::Prismoidal => {
                    let m = area(ts[i] + dt / 2.0);
                    ((w[0].0 + 4.0 * m.0 + w[1].0) / 6.0, (w[0].1 + 4.0 * m.1 + w[1].1) / 6.0)
                }
            };
            cut += c1 * dt;
            fill += f1 * dt;
        }
        Ok(Volumes { cut, fill, sections: nsec + 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::SurveyPoint;

    fn flat(z: f64) -> Tin {
        let mut pts = Vec::new();
        for i in -30..=30 {
            for j in -30..=30 {
                pts.push(SurveyPoint {
                    id: String::new(),
                    x: f64::from(i) * 2.0 + f64::from(j) * 0.001,
                    y: f64::from(j) * 2.0 + f64::from(i) * 0.0007,
                    z,
                    code: String::new(),
                });
            }
        }
        Tin::build(&pts, None).unwrap()
    }

    fn square(a: f64) -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [a, 0.0], [a, a], [0.0, a]]
    }

    #[test]
    fn a_pad_above_flat_ground_has_the_volume_of_its_slopes_and_a_daylight_line_at_the_toe() {
        let pad = Pad::new(&square(20.0), 12.0, 1.0, 1.5, 0.0, 0.0).unwrap();
        let tin = flat(10.0);
        // Height h = 2, slope f = 1.5: the toe is 3 m out; corners are quarter cones.
        let (a, h, f) = (20.0, 2.0, 1.5);
        let exact = a * a * h + 2.0 * a * f * h * h + std::f64::consts::PI * f * f * h * h * h / 3.0;
        let v = pad.volumes(&tin, &VolumeOptions { interval: 1.0, angle: 0.0, step: 0.25, method: Method::Prismoidal }).unwrap();
        assert!(v.cut.abs() < 1e-6, "{}", v.cut);
        assert!((v.fill - exact).abs() / exact < 0.01, "fill {} vs {exact}", v.fill);
        let avg = pad.volumes(&tin, &VolumeOptions { interval: 1.0, angle: 30.0, step: 0.25, method: Method::Average }).unwrap();
        assert!((avg.fill - exact).abs() / exact < 0.02, "{}", avg.fill);
        let runs = pad.daylight(&tin, 2.0, 0.5);
        let pts: Vec<&DayPoint> = runs.iter().flatten().collect();
        assert!(pts.len() > 20 && pts.iter().all(|p| !p.cut));
        for p in pts {
            let d = pad.distance(p.pt[0], p.pt[1]);
            assert!((d - 3.0).abs() < 1e-3, "toe at {d}");
        }
    }

    #[test]
    fn a_pad_below_the_ground_is_a_cut_with_its_own_slope() {
        let pad = Pad::new(&square(20.0), 8.0, 1.0, 1.5, 0.0, 0.0).unwrap();
        let tin = flat(10.0);
        let (a, h, f) = (20.0, 2.0, 1.0);
        let exact = a * a * h + 2.0 * a * f * h * h + std::f64::consts::PI * f * f * h * h * h / 3.0;
        let v = pad.volumes(&tin, &VolumeOptions { interval: 1.0, angle: 0.0, step: 0.25, method: Method::Prismoidal }).unwrap();
        assert!(v.fill.abs() < 1e-6 && (v.cut - exact).abs() / exact < 0.01, "{} vs {exact}", v.cut);
        assert!(pad.daylight(&tin, 2.0, 0.5).iter().flatten().all(|p| p.cut));
    }

    #[test]
    fn berms_push_the_toe_out_and_a_sloping_site_splits_cut_from_fill() {
        let tin = flat(10.0);
        let plain = Pad::new(&square(20.0), 16.0, 1.0, 2.0, 0.0, 0.0).unwrap();
        let benched = Pad::new(&square(20.0), 16.0, 1.0, 2.0, 2.0, 3.0).unwrap();
        let toe = |p: &Pad| p.daylight(&tin, 5.0, 0.25).iter().flatten().map(|q| p.distance(q.pt[0], q.pt[1])).fold(f64::MAX, f64::min);
        // Height 6, 1:2 → 12 m. With 2 m benches (4 m of slope + 3 m berm) it is 4+3+4+3+4 = 18 m.
        assert!((toe(&plain) - 12.0).abs() < 1e-3 && (toe(&benched) - 18.0).abs() < 1e-3, "{} {}", toe(&plain), toe(&benched));
        assert!((benched.rise(2.0, 7.0) - 2.0).abs() < 1e-12 && (benched.rise(2.0, 4.0) - 2.0).abs() < 1e-12);
        // Ground rising 1 in 10 across the site: a pad at its middle level has both.
        let mut pts = Vec::new();
        for i in -20..=40 {
            for j in -20..=40 {
                pts.push(SurveyPoint {
                    id: String::new(),
                    x: f64::from(i) * 2.0 + f64::from(j) * 0.001,
                    y: f64::from(j) * 2.0,
                    z: 10.0 + f64::from(i) * 0.4 + f64::from(j) * 0.0002,
                    code: String::new(),
                });
            }
        }
        let tin = Tin::build(&pts, None).unwrap();
        let pad = Pad::new(&square(20.0), 12.0, 1.5, 1.5, 0.0, 0.0).unwrap();
        let v = pad.volumes(&tin, &VolumeOptions { interval: 1.0, angle: 0.0, step: 0.5, method: Method::Prismoidal }).unwrap();
        assert!(v.cut > 100.0 && v.fill > 100.0, "{v:?}");
        let zl = pad.zero_line(&tin);
        assert!(!zl.is_empty());
        assert!(zl.iter().flat_map(|(p, _)| p).all(|q| (tin.z_at(q[0], q[1]).unwrap() - 12.0).abs() < 1e-6));
    }

    #[test]
    fn bad_pads_and_oversized_jobs_are_refused() {
        assert!(Pad::new(&[[0.0, 0.0], [1.0, 1.0]], 0.0, 1.0, 1.0, 0.0, 0.0).is_err());
        assert!(Pad::new(&square(5.0), 0.0, 0.0, 1.0, 0.0, 0.0).is_err());
        assert!(Pad::new(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]], 0.0, 1.0, 1.0, 0.0, 0.0).is_err());
        let pad = Pad::new(&square(20.0), 12.0, 1.0, 1.0, 0.0, 0.0).unwrap();
        let o = VolumeOptions { interval: 0.0, angle: 0.0, step: 1.0, method: Method::Average };
        assert!(pad.volumes(&flat(10.0), &o).is_err());
        let big = VolumeOptions { interval: 0.001, angle: 0.0, step: 0.001, method: Method::Average };
        assert!(pad.volumes(&flat(10.0), &big).unwrap_err().contains("larger"));
        // Counterclockwise or clockwise input gives the same pad.
        let cw: Vec<[f64; 2]> = square(20.0).into_iter().rev().collect();
        assert!((Pad::new(&cw, 12.0, 1.0, 1.0, 0.0, 0.0).unwrap().area() - 400.0).abs() < 1e-9);
    }
}
