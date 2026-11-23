//! Contour lines (curvas de nivel) over a TIN.
//!
//! Each triangle that a level crosses gives one short segment between two points on its sides;
//! the segments of a level are then chained into polylines. Segment ends are identified by the
//! TIN side they lie on, so neighbouring triangles join exactly, with no distance tolerance.

use std::collections::HashMap;

use crate::points::SurveyPoint;
use crate::tin::Tin;

/// The most levels one call may produce, so an interval of a millimetre over a mountain is
/// refused rather than run for minutes.
pub const MAX_LEVELS: usize = 5000;

/// One contour polyline at one elevation.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub level: f64,
    pub pts: Vec<[f64; 2]>,
    pub closed: bool,
}

type Side = (usize, usize);

/// Contours every `interval`, on the multiples of `interval` counted from `base`.
pub fn contours(tin: &Tin, interval: f64, base: f64) -> Result<Vec<Contour>, String> {
    if !(interval.is_finite() && interval > 0.0) {
        return Err("the contour interval must be a number above zero".into());
    }
    let (zmin, zmax) = tin.z_range();
    let first = ((zmin - base) / interval).ceil();
    let last = ((zmax - base) / interval).floor();
    if last < first {
        return Ok(Vec::new());
    }
    let count = (last - first) as usize + 1;
    if count > MAX_LEVELS {
        return Err(format!("{count} contour levels: use a larger interval (the limit is {MAX_LEVELS})"));
    }
    let level = |k: usize| base + (first + k as f64) * interval;
    let eps = interval * 1e-9;
    let mut segs: Vec<Vec<(Side, Side)>> = vec![Vec::new(); count];
    let mut pos: Vec<HashMap<Side, [f64; 2]>> = vec![HashMap::new(); count];
    for t in &tin.tris {
        let v: Vec<[f64; 3]> = t.iter().filter_map(|&i| tin.pts.get(i).copied()).collect();
        if v.len() != 3 {
            continue;
        }
        let (lo, hi) = v.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p[2]), h.max(p[2])));
        let k0 = (((lo - base) / interval).ceil() - first).max(0.0);
        let k1 = (((hi - base) / interval).floor() - first).min(count as f64 - 1.0);
        if k1 < k0 {
            continue;
        }
        for k in (k0 as usize)..=(k1 as usize) {
            let lv = level(k);
            // A vertex exactly on the level counts as just above it, so each triangle has 0 or 2 crossings.
            let above = |z: f64| (if z == lv { z + eps } else { z }) > lv;
            let mut ends: Vec<(Side, [f64; 2])> = Vec::new();
            for e in 0..3 {
                let (a, b) = (e, (e + 1) % 3);
                let (pa, pb) = (v[a], v[b]);
                if above(pa[2]) != above(pb[2]) {
                    let s = (lv - pa[2]) / (pb[2] - pa[2]);
                    let side = (t[a].min(t[b]), t[a].max(t[b]));
                    ends.push((side, [pa[0] + s * (pb[0] - pa[0]), pa[1] + s * (pb[1] - pa[1])]));
                }
            }
            if let [(sa, pa), (sb, pb)] = ends[..] {
                if let Some(m) = pos.get_mut(k) {
                    m.insert(sa, pa);
                    m.insert(sb, pb);
                }
                if let Some(s) = segs.get_mut(k) {
                    s.push((sa, sb));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (k, (s, p)) in segs.iter().zip(pos.iter()).enumerate() {
        for (sides, closed) in chain(s) {
            let pts: Vec<[f64; 2]> = sides.iter().filter_map(|k| p.get(k).copied()).collect();
            if pts.len() >= 2 {
                out.push(Contour { level: level(k), pts, closed });
            }
        }
    }
    Ok(out)
}

/// Join segments that share an end into chains; `true` when a chain closes on itself.
fn chain(segs: &[(Side, Side)]) -> Vec<(Vec<Side>, bool)> {
    let mut at: HashMap<Side, Vec<usize>> = HashMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        at.entry(*a).or_default().push(i);
        at.entry(*b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    let mut starts: Vec<usize> = (0..segs.len())
        .filter(|&i| segs.get(i).is_some_and(|(a, b)| at.get(a).map_or(0, Vec::len) == 1 || at.get(b).map_or(0, Vec::len) == 1))
        .collect();
    starts.extend(0..segs.len());
    for i in starts {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(&(a, b)) = segs.get(i) else { continue };
        // Begin at the free end of an open chain, or anywhere on a loop.
        let (mut from, mut to) = if at.get(&a).map_or(0, Vec::len) == 1 {
            (a, b)
        } else if at.get(&b).map_or(0, Vec::len) == 1 {
            (b, a)
        } else {
            (a, b)
        };
        let mut seq = vec![from, to];
        used[i] = true;
        let mut closed = false;
        loop {
            let next = at.get(&to).and_then(|l| l.iter().copied().find(|&j| !used.get(j).copied().unwrap_or(true)));
            let Some(j) = next else {
                break;
            };
            used[j] = true;
            let Some(&(c, d)) = segs.get(j) else { break };
            from = to;
            to = if c == from { d } else { c };
            seq.push(to);
            if to == seq[0] {
                closed = true;
                break;
            }
        }
        if closed {
            seq.pop();
        }
        out.push((seq, closed));
    }
    out
}

/// Points along a break line (a ridge, a ditch, a road edge), so the TIN follows it.
///
/// `line` holds the vertices with their elevation when they have one; the others take the ground
/// elevation of `ground` at that place (a vertex outside the TIN takes the nearest known one).
/// The line is cut into pieces no longer than `step`, with the elevation interpolated along it.
/// The TIN built from these points together with the survey has an edge close to every piece, so
/// the contours bend at the line instead of cutting across it.
pub fn break_points(ground: &Tin, line: &[(f64, f64, Option<f64>)], step: f64) -> Vec<SurveyPoint> {
    let mut v: Vec<[f64; 3]> = Vec::new();
    let mut last = None;
    for &(x, y, z) in line {
        let z = z.or_else(|| ground.z_at(x, y)).or(last);
        if let Some(z) = z {
            last = Some(z);
            v.push([x, y, z]);
        }
    }
    let step = if step.is_finite() && step > 0.0 { step } else { return Vec::new() };
    let mut out = Vec::new();
    let mut add = |q: [f64; 3]| out.push(SurveyPoint { id: String::new(), x: q[0], y: q[1], z: q[2], code: "BREAK".into() });
    for w in v.windows(2) {
        let len = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        let n = ((len / step).ceil() as usize).clamp(1, 100_000);
        for k in 0..n {
            let t = k as f64 / n as f64;
            add([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t, w[0][2] + (w[1][2] - w[0][2]) * t]);
        }
    }
    if let Some(l) = v.last() {
        add(*l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::SurveyPoint;

    /// A cone: elevation falls with distance from the centre, so contours are closed rings.
    fn cone() -> Tin {
        let mut pts = Vec::new();
        for i in -10..=10 {
            for j in -10..=10 {
                let (x, y) = (f64::from(i) * 5.0 + f64::from(j) * 0.013, f64::from(j) * 5.0 + f64::from(i) * 0.007);
                pts.push(SurveyPoint { id: String::new(), x, y, z: 100.0 - (x * x + y * y).sqrt(), code: String::new() });
            }
        }
        Tin::build(&pts, None).unwrap()
    }

    #[test]
    fn contours_of_a_cone_are_closed_rings_at_the_asked_levels() {
        let c = contours(&cone(), 10.0, 0.0).unwrap();
        assert!(!c.is_empty());
        for ring in c.iter().filter(|c| c.level > 55.0) {
            assert!(ring.closed, "ring at {} should close", ring.level);
            let r = 100.0 - ring.level;
            for p in &ring.pts {
                assert!(((p[0] * p[0] + p[1] * p[1]).sqrt() - r).abs() < 1.5, "points lie near the ring of radius {r}");
            }
        }
        assert!(c.iter().all(|c| (c.level % 10.0).abs() < 1e-9));
    }

    #[test]
    fn a_contour_on_a_plane_is_one_open_straight_line() {
        let pts: Vec<SurveyPoint> = (0..8)
            .flat_map(|i| {
                (0..8).map(move |j| SurveyPoint { id: String::new(), x: f64::from(i), y: f64::from(j), z: f64::from(i), code: String::new() })
            })
            .collect();
        let tin = Tin::build(&pts, None).unwrap();
        let c = contours(&tin, 1.0, 0.5).unwrap();
        let at3 = c.iter().filter(|c| (c.level - 3.5).abs() < 1e-9).collect::<Vec<_>>();
        assert_eq!(at3.len(), 1);
        assert!(!at3[0].closed);
        assert!(at3[0].pts.iter().all(|p| (p[0] - 3.5).abs() < 1e-9));
    }

    #[test]
    fn a_bad_interval_or_too_many_levels_is_refused() {
        let tin = cone();
        assert!(contours(&tin, 0.0, 0.0).is_err());
        assert!(contours(&tin, f64::NAN, 0.0).is_err());
        assert!(contours(&tin, 1e-6, 0.0).unwrap_err().contains("larger interval"));
    }

    #[test]
    fn a_break_line_is_cut_into_short_pieces_with_the_elevation_interpolated() {
        let g = cone();
        let pts = break_points(&g, &[(0.0, 0.0, Some(10.0)), (10.0, 0.0, Some(20.0))], 2.5);
        assert_eq!(pts.len(), 5);
        assert!((pts[1].x - 2.5).abs() < 1e-9 && (pts[1].z - 12.5).abs() < 1e-9);
        assert!((pts[4].z - 20.0).abs() < 1e-9);
        // A vertex without elevation takes the ground's.
        let p = break_points(&g, &[(0.0, 0.0, None), (5.0, 0.0, None)], 5.0);
        assert!((p[0].z - g.z_at(0.0, 0.0).unwrap()).abs() < 1e-9);
        assert!(break_points(&g, &[(0.0, 0.0, Some(1.0)), (1.0, 0.0, Some(1.0))], 0.0).is_empty());
    }
}
