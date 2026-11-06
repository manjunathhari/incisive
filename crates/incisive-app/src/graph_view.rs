//! The knowledge-graph map's shared types and layout (no Leptos, no
//! database): the server fills them from one user's links and the page draws
//! them.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DotKind {
    Organ,
    Disease,
    Run,
    Treatment,
    Outcome,
    Paper,
}

impl DotKind {
    /// The word a `MapFilter::hidden` entry uses for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            DotKind::Organ => "organ",
            DotKind::Disease => "disease",
            DotKind::Run => "run",
            DotKind::Treatment => "treatment",
            DotKind::Outcome => "outcome",
            DotKind::Paper => "paper",
        }
    }
}

/// One dot. `id` is `g:`/`d:`/`t:`/`o:` plus the concept id, `r:` plus the
/// run id, or `p:` plus the PMID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dot {
    pub id: String,
    pub kind: DotKind,
    pub label: String,
    pub sub: String,
    pub first_year: Option<i32>,
    pub r: f32,
    /// A paper's rank code (`rct`, `meta`, ...); `None` for other dots.
    pub rank: Option<String>,
    pub x: f32,
    pub y: f32,
    pub run_id: Option<Uuid>,
    pub concept_id: Option<i64>,
    pub pmid: Option<String>,
    pub retracted: bool,
}

/// A line between two dot ids; `paper` for a run–paper line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Line {
    pub a: String,
    pub b: String,
    pub paper: bool,
}

/// What the map shows. `organ` is an organ concept id; years bound papers;
/// `certainty` keeps runs with an outcome of that certainty; `hidden` holds
/// dot kinds (`paper`, `treatment`, ...) and paper ranks (`rct`, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapFilter {
    pub organ: Option<i64>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub certainty: Option<String>,
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapData {
    pub dots: Vec<Dot>,
    pub lines: Vec<Line>,
    /// Papers were left out because the map was too big and the user had
    /// hidden nothing.
    pub papers_hidden_by_size: bool,
    /// The user's runs on the map.
    pub runs: u32,
}

const STEPS: usize = 300;
const GOLDEN_ANGLE: f32 = 2.39996;
const PAD: f32 = 34.0;

/// Place the dots in a `w` by `h` frame: a fixed-step force simulation, so
/// the same input always gives the same picture. Sorts `dots` by id; lines to
/// an id that is not there are ignored.
pub fn layout(dots: &mut [Dot], lines: &[Line], w: f32, h: f32) {
    let (w, h) = (w.max(0.0), h.max(0.0));
    dots.sort_by(|a, b| a.id.cmp(&b.id));
    let n = dots.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        dots[0].x = w / 2.0;
        dots[0].y = h / 2.0;
        return;
    }
    let mut x: Vec<f32> = Vec::with_capacity(n);
    let mut y: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let (angle, radius) = (i as f32 * GOLDEN_ANGLE, 12.0 * ((i + 1) as f32).sqrt());
        x.push(angle.cos() * radius);
        y.push(angle.sin() * radius);
    }
    let r: Vec<f32> = dots.iter().map(|d| d.r).collect();
    // Springs by index; first match wins for a repeated id.
    let index = |id: &str| dots.iter().position(|d| d.id == id);
    let springs: Vec<(usize, usize, f32)> = lines
        .iter()
        .filter_map(|l| {
            let (a, b) = (index(&l.a)?, index(&l.b)?);
            (a != b).then_some((a, b, if l.paper { 42.0 } else { 55.0 }))
        })
        .collect();
    let (mut vx, mut vy) = (vec![0.0f32; n], vec![0.0f32; n]);

    for t in 0..STEPS {
        let a = 1.0 - t as f32 / STEPS as f32;
        for i in 0..n {
            for j in (i + 1)..n {
                let (dx, dy, d) = apart(&x, &y, i, j);
                let f = (60.0 + (r[i] + r[j]) * 6.0) * a / (d * d);
                let (fx, fy) = (f * dx / d, f * dy / d);
                vx[i] -= fx;
                vy[i] -= fy;
                vx[j] += fx;
                vy[j] += fy;
            }
        }
        for &(i, j, len) in &springs {
            let (dx, dy, d) = apart(&x, &y, i, j);
            let f = (d - len) * 0.3 * a / 2.0;
            let (fx, fy) = (f * dx / d, f * dy / d);
            vx[i] += fx;
            vy[i] += fy;
            vx[j] -= fx;
            vy[j] -= fy;
        }
        for i in 0..n {
            vx[i] -= x[i] * 0.04 * a;
            vy[i] -= y[i] * 0.04 * a;
            vx[i] *= 0.6;
            vy[i] *= 0.6;
            x[i] += vx[i];
            y[i] += vy[i];
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let min = r[i] + r[j] + 6.0;
                let (dx, dy, d) = apart(&x, &y, i, j);
                if d < min {
                    let push = (min - d) / 2.0;
                    let (px, py) = (push * dx / d, push * dy / d);
                    x[i] -= px;
                    y[i] -= py;
                    x[j] += px;
                    y[j] += py;
                }
            }
        }
    }
    let xs = fit(&x, w);
    let ys = fit(&y, h);
    for (i, d) in dots.iter_mut().enumerate() {
        d.x = xs[i];
        d.y = ys[i];
    }
}

/// The vector from dot `i` to dot `j` and its length, never zero: two dots
/// on one spot are split along a fixed direction that depends on the pair.
fn apart(x: &[f32], y: &[f32], i: usize, j: usize) -> (f32, f32, f32) {
    let (mut dx, mut dy) = (x[j] - x[i], y[j] - y[i]);
    let mut d2 = dx * dx + dy * dy;
    if d2.is_nan() || d2 <= 1e-6 {
        let angle = (i * 31 + j * 17) as f32 * GOLDEN_ANGLE;
        dx = angle.cos() * 0.01;
        dy = angle.sin() * 0.01;
        d2 = 1e-4;
    }
    (dx, dy, d2.sqrt())
}

/// Scale one axis into `0..size` with padding (less if the frame is small);
/// a flat axis sits in the middle.
fn fit(v: &[f32], size: f32) -> Vec<f32> {
    let pad = PAD.min(size / 2.0);
    let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = hi - lo;
    v.iter()
        .map(|&p| {
            let s = if span.is_finite() && span > 1e-6 && p.is_finite() {
                pad + (p - lo) / span * (size - 2.0 * pad)
            } else {
                size / 2.0
            };
            s.clamp(0.0, size)
        })
        .collect()
}
