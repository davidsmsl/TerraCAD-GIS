//! A triangulated irregular network (TIN): the Delaunay triangulation of survey points, with the
//! ground elevation at any point inside it.
//!
//! The triangulation is built point by point (Bowyer-Watson): a point is located by walking
//! across neighbouring triangles, the triangles whose circumscribed circle holds it are removed,
//! and the hole is filled with triangles that join the point to the hole's edge. Points are
//! inserted in Hilbert-curve order so each walk is short, which keeps a survey of some hundred
//! thousand points in seconds.

use crate::points::SurveyPoint;

const NONE: usize = usize::MAX;

/// Why a TIN could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TinError(pub String);

impl std::fmt::Display for TinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TinError {}

/// The triangulation: distinct points `(x, y, z)` and triangles as counter-clockwise indices.
#[derive(Clone, Debug)]
pub struct Tin {
    pub pts: Vec<[f64; 3]>,
    pub tris: Vec<[usize; 3]>,
    grid: Grid,
}

#[derive(Clone, Debug, Default)]
struct Grid {
    min: [f64; 2],
    cell: [f64; 2],
    dims: [usize; 2],
    cells: Vec<Vec<u32>>,
}

#[derive(Clone, Copy)]
struct Tri {
    v: [usize; 3],
    /// `n[i]` is the neighbour across the edge opposite vertex `i`.
    n: [usize; 3],
    alive: bool,
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Positive when `p` lies inside the circle through the counter-clockwise triangle `a b c`.
fn in_circle(a: [f64; 2], b: [f64; 2], c: [f64; 2], p: [f64; 2]) -> f64 {
    let (ax, ay) = (a[0] - p[0], a[1] - p[1]);
    let (bx, by) = (b[0] - p[0], b[1] - p[1]);
    let (cx, cy) = (c[0] - p[0], c[1] - p[1]);
    (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay) + (cx * cx + cy * cy) * (ax * by - bx * ay)
}

/// The position of cell `(x, y)` along a Hilbert curve over a `2^16` square.
fn hilbert(mut x: u32, mut y: u32) -> u64 {
    let n: u32 = 1 << 16;
    let mut d = 0u64;
    let mut s = n / 2;
    while s > 0 {
        let rx = u32::from(x & s > 0);
        let ry = u32::from(y & s > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

struct Builder {
    p: Vec<[f64; 2]>,
    tris: Vec<Tri>,
    stamp: Vec<u32>,
    epoch: u32,
}

impl Builder {
    fn xy(&self, i: usize) -> [f64; 2] {
        self.p.get(i).copied().unwrap_or([0.0; 2])
    }

    fn tri(&self, t: usize) -> Tri {
        self.tris.get(t).copied().unwrap_or(Tri { v: [0; 3], n: [NONE; 3], alive: false })
    }

    /// A live triangle that contains `p`, walking from `from`.
    fn locate(&self, p: [f64; 2], from: usize) -> Option<usize> {
        let mut t = from;
        for _ in 0..self.tris.len().saturating_mul(2).max(64) {
            let tr = self.tri(t);
            if !tr.alive {
                break;
            }
            let mut moved = false;
            for i in 0..3 {
                let a = self.xy(tr.v[(i + 1) % 3]);
                let b = self.xy(tr.v[(i + 2) % 3]);
                if orient(a, b, p) < 0.0 {
                    let nb = tr.n[i];
                    if nb == NONE {
                        return None;
                    }
                    t = nb;
                    moved = true;
                    break;
                }
            }
            if !moved {
                return Some(t);
            }
        }
        // The walk cycled (nearly degenerate input): scan.
        (0..self.tris.len()).find(|&i| {
            let tr = self.tri(i);
            tr.alive && (0..3).all(|k| orient(self.xy(tr.v[(k + 1) % 3]), self.xy(tr.v[(k + 2) % 3]), p) >= 0.0)
        })
    }

    fn insert(&mut self, pi: usize, hint: usize) -> usize {
        let p = self.xy(pi);
        let Some(start) = self.locate(p, hint) else { return hint };
        self.epoch += 1;
        let epoch = self.epoch;
        let mut bad = vec![start];
        let mut stack = vec![start];
        if let Some(s) = self.stamp.get_mut(start) {
            *s = epoch;
        }
        while let Some(t) = stack.pop() {
            for nb in self.tri(t).n {
                if nb == NONE || self.stamp.get(nb).copied() == Some(epoch) {
                    continue;
                }
                let tr = self.tri(nb);
                if tr.alive && in_circle(self.xy(tr.v[0]), self.xy(tr.v[1]), self.xy(tr.v[2]), p) > 0.0 {
                    if let Some(s) = self.stamp.get_mut(nb) {
                        *s = epoch;
                    }
                    bad.push(nb);
                    stack.push(nb);
                }
            }
        }
        // The hole's edges, each with the triangle outside it.
        let mut fresh: Vec<usize> = Vec::new();
        let mut by_a: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        let mut by_b: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for &t in &bad {
            let tr = self.tri(t);
            for i in 0..3 {
                let nb = tr.n[i];
                if nb != NONE && self.stamp.get(nb).copied() == Some(epoch) && self.tri(nb).alive {
                    continue;
                }
                let (a, b) = (tr.v[(i + 1) % 3], tr.v[(i + 2) % 3]);
                let id = self.tris.len();
                self.tris.push(Tri { v: [pi, a, b], n: [nb, NONE, NONE], alive: true });
                self.stamp.push(0);
                if nb != NONE {
                    let outer = self.tri(nb);
                    if let Some(j) = (0..3).find(|&j| outer.n[j] == t)
                        && let Some(o) = self.tris.get_mut(nb)
                    {
                        o.n[j] = id;
                    }
                }
                by_a.insert(a, id);
                by_b.insert(b, id);
                fresh.push(id);
            }
        }
        for &id in &fresh {
            let tr = self.tri(id);
            let (a, b) = (tr.v[1], tr.v[2]);
            let across_b = by_a.get(&b).copied().unwrap_or(NONE); // shares edge b-p
            let across_a = by_b.get(&a).copied().unwrap_or(NONE); // shares edge p-a
            if let Some(m) = self.tris.get_mut(id) {
                m.n[1] = across_b;
                m.n[2] = across_a;
            }
        }
        for &t in &bad {
            if let Some(m) = self.tris.get_mut(t) {
                m.alive = false;
            }
        }
        fresh.first().copied().unwrap_or(hint)
    }
}

impl Tin {
    /// Triangulate `points`. Points at the same X, Y are one point (the first is kept). Triangles
    /// with a side longer than `max_edge` are dropped: they span gaps in the survey, such as a
    /// pond or a building, where there is no ground to interpolate.
    pub fn build(points: &[SurveyPoint], max_edge: Option<f64>) -> Result<Tin, TinError> {
        let mut seen = std::collections::HashSet::new();
        let mut pts: Vec<[f64; 3]> = Vec::new();
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in points {
            if p.x.is_finite() && p.y.is_finite() && p.z.is_finite() && seen.insert((p.x.to_bits(), p.y.to_bits())) {
                pts.push([p.x, p.y, p.z]);
                lo = [lo[0].min(p.x), lo[1].min(p.y)];
                hi = [hi[0].max(p.x), hi[1].max(p.y)];
            }
        }
        if pts.len() < 3 {
            return Err(TinError("a TIN needs at least 3 points with different X, Y".into()));
        }
        let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if extent <= 0.0 {
            return Err(TinError("the points are all at the same place".into()));
        }
        let n = pts.len();
        let m = extent * 1000.0;
        let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        let mut b = Builder { p: pts.iter().map(|p| [p[0], p[1]]).collect(), tris: Vec::new(), stamp: Vec::new(), epoch: 0 };
        b.p.extend([[cx - 2.0 * m, cy - m], [cx + 2.0 * m, cy - m], [cx, cy + 2.0 * m]]);
        b.tris.push(Tri { v: [n, n + 1, n + 2], n: [NONE; 3], alive: true });
        b.stamp.push(0);
        let mut order: Vec<(u64, usize)> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = |v: f64, l: f64| (((v - l) / extent) * 65535.0).clamp(0.0, 65535.0) as u32;
                (hilbert(q(p[0], lo[0]), q(p[1], lo[1])), i)
            })
            .collect();
        order.sort_unstable();
        let mut hint = 0;
        for (_, i) in order {
            hint = b.insert(i, hint);
        }
        let limit = max_edge.map(|e| e * e);
        let dist2 = |a: usize, c: usize| {
            let (p, q) = (b.xy(a), b.xy(c));
            (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)
        };
        let tris: Vec<[usize; 3]> = b
            .tris
            .iter()
            .filter(|t| t.alive && t.v.iter().all(|&v| v < n))
            .filter(|t| orient(b.xy(t.v[0]), b.xy(t.v[1]), b.xy(t.v[2])).abs() > 0.0)
            .filter(|t| limit.is_none_or(|l| dist2(t.v[0], t.v[1]) <= l && dist2(t.v[1], t.v[2]) <= l && dist2(t.v[2], t.v[0]) <= l))
            .map(|t| t.v)
            .collect();
        if tris.is_empty() {
            return Err(TinError("no triangles: the points are in a line, or `max_edge` is shorter than their spacing".into()));
        }
        let grid = Grid::build(&pts, &tris, lo, hi);
        Ok(Tin { pts, tris, grid })
    }

    /// The ground elevation at `(x, y)`, or `None` outside every triangle.
    pub fn z_at(&self, x: f64, y: f64) -> Option<f64> {
        let g = &self.grid;
        let cx = (((x - g.min[0]) / g.cell[0]).floor() as isize).clamp(0, g.dims[0] as isize - 1) as usize;
        let cy = (((y - g.min[1]) / g.cell[1]).floor() as isize).clamp(0, g.dims[1] as isize - 1) as usize;
        let tol = 1e-9;
        for &ti in g.cells.get(cy * g.dims[0] + cx)? {
            let t = self.tris.get(ti as usize)?;
            let (a, b, c) = (self.pts.get(t[0])?, self.pts.get(t[1])?, self.pts.get(t[2])?);
            let area = orient([a[0], a[1]], [b[0], b[1]], [c[0], c[1]]);
            let w0 = orient([b[0], b[1]], [c[0], c[1]], [x, y]) / area;
            let w1 = orient([c[0], c[1]], [a[0], a[1]], [x, y]) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= -tol && w1 >= -tol && w2 >= -tol {
                return Some(w0 * a[2] + w1 * b[2] + w2 * c[2]);
            }
        }
        None
    }

    /// The elevation range of the points.
    pub fn z_range(&self) -> (f64, f64) {
        self.pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p[2]), hi.max(p[2])))
    }
}

impl Grid {
    fn build(pts: &[[f64; 3]], tris: &[[usize; 3]], lo: [f64; 2], hi: [f64; 2]) -> Grid {
        let side = (tris.len() as f64).sqrt().ceil().clamp(1.0, 1024.0) as usize;
        let dims = [side, side];
        let cell = [((hi[0] - lo[0]) / side as f64).max(f64::MIN_POSITIVE), ((hi[1] - lo[1]) / side as f64).max(f64::MIN_POSITIVE)];
        let mut cells = vec![Vec::new(); side * side];
        let at = |v: f64, l: f64, c: f64| (((v - l) / c).floor() as isize).clamp(0, side as isize - 1) as usize;
        for (ti, t) in tris.iter().enumerate() {
            let xs: Vec<f64> = t.iter().filter_map(|&i| pts.get(i)).map(|p| p[0]).collect();
            let ys: Vec<f64> = t.iter().filter_map(|&i| pts.get(i)).map(|p| p[1]).collect();
            let (x0, x1) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
            let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
            for cy in at(y0, lo[1], cell[1])..=at(y1, lo[1], cell[1]) {
                for cx in at(x0, lo[0], cell[0])..=at(x1, lo[0], cell[0]) {
                    if let Some(c) = cells.get_mut(cy * side + cx) {
                        c.push(ti as u32);
                    }
                }
            }
        }
        Grid { min: lo, cell, dims, cells }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repeatable scatter of points on a tilted plane `z = 2x + 3y + 1`.
    fn scatter(n: usize) -> Vec<SurveyPoint> {
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..n)
            .map(|i| {
                let (x, y) = (next() * 100.0, next() * 80.0);
                SurveyPoint { id: i.to_string(), x, y, z: 2.0 * x + 3.0 * y + 1.0, code: String::new() }
            })
            .collect()
    }

    fn hull_len(pts: &[[f64; 3]]) -> usize {
        let mut p: Vec<[f64; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
        p.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut h: Vec<[f64; 2]> = Vec::new();
        for pass in 0..2 {
            let start = h.len();
            for &q in &p {
                while h.len() >= start + 2 && orient(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
                    h.pop();
                }
                h.push(q);
            }
            h.pop();
            p.reverse();
            let _ = pass;
        }
        h.len()
    }

    #[test]
    fn every_triangle_has_an_empty_circumcircle() {
        let tin = Tin::build(&scatter(300), None).unwrap();
        for t in &tin.tris {
            let xy = |i: usize| [tin.pts[i][0], tin.pts[i][1]];
            assert!(orient(xy(t[0]), xy(t[1]), xy(t[2])) > 0.0, "triangles are counter-clockwise");
            for (i, p) in tin.pts.iter().enumerate() {
                if !t.contains(&i) {
                    assert!(in_circle(xy(t[0]), xy(t[1]), xy(t[2]), [p[0], p[1]]) < 1e-6, "point {i} inside a circumcircle");
                }
            }
        }
    }

    #[test]
    fn triangle_count_follows_euler_for_points_in_general_position() {
        let tin = Tin::build(&scatter(500), None).unwrap();
        assert_eq!(tin.tris.len(), 2 * tin.pts.len() - 2 - hull_len(&tin.pts));
    }

    #[test]
    fn elevation_on_a_plane_is_exact_inside_and_absent_outside() {
        let tin = Tin::build(&scatter(400), None).unwrap();
        let z = tin.z_at(50.0, 40.0).unwrap();
        assert!((z - (2.0 * 50.0 + 3.0 * 40.0 + 1.0)).abs() < 1e-6);
        assert!(tin.z_at(-50.0, 40.0).is_none());
        assert!(tin.z_at(500.0, 500.0).is_none());
    }

    #[test]
    fn a_regular_grid_with_cocircular_points_still_triangulates() {
        let mut pts = Vec::new();
        for i in 0..12 {
            for j in 0..12 {
                pts.push(SurveyPoint { id: String::new(), x: f64::from(i) * 10.0, y: f64::from(j) * 10.0, z: f64::from(i + j), code: String::new() });
            }
        }
        let tin = Tin::build(&pts, None).unwrap();
        assert_eq!(tin.tris.len(), 2 * 11 * 11);
        assert!((tin.z_at(35.0, 45.0).unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn long_triangles_over_a_gap_are_dropped_by_max_edge() {
        let mut pts = scatter(0);
        for (x, y) in [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0), (500.0, 5.0)] {
            pts.push(SurveyPoint { id: String::new(), x, y, z: 0.0, code: String::new() });
        }
        let all = Tin::build(&pts, None).unwrap();
        let near = Tin::build(&pts, Some(30.0)).unwrap();
        assert!(near.tris.len() < all.tris.len());
        assert!(near.z_at(500.0, 5.0).is_none());
    }

    #[test]
    fn too_few_or_collinear_points_are_errors() {
        assert!(Tin::build(&scatter(2), None).is_err());
        let line: Vec<SurveyPoint> =
            (0..10).map(|i| SurveyPoint { id: String::new(), x: f64::from(i), y: f64::from(i), z: 0.0, code: String::new() }).collect();
        assert!(Tin::build(&line, None).is_err());
    }
}
