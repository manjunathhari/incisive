//! Step 6 maths: fixed-effect pooling, the dynamic (Kalman) model,
//! DerSimonian–Laird random effects and the predictive range. Plain f64
//! arithmetic on the pooling scale (log for ratios, linear for
//! differences); every function guards against empty or non-finite input
//! and returns `None` rather than panicking.

use serde::{Deserialize, Serialize};

/// 1.959964: the two-sided 95% normal quantile.
pub const Z95: f64 = 1.959_964;
/// The drift scales of the dynamic model, in units of [`Scale::tau_unit`]
/// (log units for ratios; the median row SE for differences).
pub const TAUS: [f64; 4] = [0.0, 0.25, 0.5, 1.0];
/// The dynamic model's starting prior: N(0, PRIOR_SD²) on the log scale
/// for ratios. Differences use [`Scale`] instead (raw units).
pub const PRIOR_MEAN: f64 = 0.0;
pub const PRIOR_SD: f64 = 1.0;
/// For differences: the prior SD is this many median row SEs.
pub const DIFF_PRIOR_SES: f64 = 10.0;

/// The dynamic model's scale for one group.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub prior_sd: f64,
    /// τ = TAUS × this.
    pub tau_unit: f64,
}

impl Scale {
    /// Ratios: N(0, 1²) and τ in log units. Differences: prior SD = 10 ×
    /// the median row SE and τ in median SEs, so the prior is weak in the
    /// data's own units. `None` for differences with no usable SE.
    pub fn for_family(ratio: bool, ses: &[f64]) -> Option<Scale> {
        if ratio {
            return Some(Scale {
                prior_sd: PRIOR_SD,
                tau_unit: 1.0,
            });
        }
        let m = median(ses.iter().copied().filter(|s| s.is_finite() && *s > 0.0))?;
        Some(Scale {
            prior_sd: DIFF_PRIOR_SES * m,
            tau_unit: m,
        })
    }
}

/// The median of `xs` (`None` when empty).
pub fn median(xs: impl IntoIterator<Item = f64>) -> Option<f64> {
    let mut v: Vec<f64> = xs.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// A finite stand-in for a non-finite result (NaN → 0, ±∞ → ±f64::MAX), so
/// every number the table stores serialises and reads back.
pub fn finite(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(-f64::MAX, f64::MAX)
    }
}
/// A typical within-study SE for a large study, added to the predictive
/// range (a log-scale constant; negligible for differences).
pub const PRED_SE: f64 = 0.15;
/// The posterior probability a direction needs to count as decided.
pub const ROBUST_P: f64 = 0.8;

/// The standard normal CDF (via a Chebyshev erfc fit, |error| < 1.2e-7).
pub fn norm_cdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// One estimate on the pooling scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Obs {
    pub y: f64,
    pub se: f64,
}

impl Obs {
    pub fn usable(self) -> bool {
        self.y.is_finite() && self.se.is_finite() && self.se > 0.0
    }
}

/// Fixed-effect (inverse-variance) pool of `obs`; unusable entries are
/// ignored. `None` when nothing is usable.
pub fn fixed_pool(obs: &[Obs]) -> Option<Obs> {
    let (mut sw, mut swy) = (0.0, 0.0);
    for o in obs.iter().filter(|o| o.usable()) {
        let w = 1.0 / (o.se * o.se);
        sw += w;
        swy += w * o.y;
    }
    (sw > 0.0 && sw.is_finite()).then(|| Obs {
        y: swy / sw,
        se: (1.0 / sw).sqrt(),
    })
}

/// The filtered posterior after one step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Posterior {
    pub mean: f64,
    pub var: f64,
}

impl Posterior {
    pub fn sd(self) -> f64 {
        self.var.sqrt()
    }
    /// P(θ < 0).
    pub fn p_below_zero(self) -> f64 {
        norm_cdf(-self.mean / self.sd())
    }
}

/// The local-level Kalman filter θ_k ~ N(θ_{k−1}, τ²), from N(0,
/// prior_sd²), over era steps (`None`: no data that era; the drift is
/// still added). Returns the posterior after each step and the
/// log-likelihood of the data.
pub fn kalman(steps: &[Option<Obs>], tau: f64, prior_sd: f64) -> (Vec<Posterior>, f64) {
    let tau2 = if tau.is_finite() { tau * tau } else { 0.0 };
    let prior_sd = if prior_sd.is_finite() && prior_sd > 0.0 {
        prior_sd
    } else {
        PRIOR_SD
    };
    let mut m = PRIOR_MEAN;
    let mut v = prior_sd * prior_sd;
    let mut ll = 0.0;
    let mut out = Vec::with_capacity(steps.len());
    for (i, o) in steps.iter().enumerate() {
        if i > 0 {
            v += tau2;
        }
        if let Some(o) = *o {
            let (p, l) = kalman_step(Posterior { mean: m, var: v }, o);
            m = p.mean;
            v = p.var;
            ll += l;
        }
        out.push(Posterior { mean: m, var: v });
    }
    (out, ll)
}

/// One conjugate update of `prior` with `o` (no drift): the posterior and
/// the observation's log predictive density.
pub fn kalman_step(prior: Posterior, o: Obs) -> (Posterior, f64) {
    if !o.usable() {
        return (prior, 0.0);
    }
    let pv = prior.var + o.se * o.se;
    let r = o.y - prior.mean;
    let ll = -0.5 * ((2.0 * std::f64::consts::PI * pv).ln() + r * r / pv);
    let k = prior.var / pv;
    (
        Posterior {
            mean: prior.mean + k * r,
            var: prior.var * (1.0 - k),
        },
        ll,
    )
}

/// Z of a new era estimate against what was believed before it.
pub fn z_vs_prior(era: Obs, prev: Posterior) -> Option<f64> {
    let d = era.se * era.se + prev.var;
    (era.usable() && d > 0.0 && prev.mean.is_finite())
        .then(|| (era.y - prev.mean) / d.sqrt())
        .filter(|z| z.is_finite())
}

/// A DerSimonian–Laird random-effects pool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReFit {
    pub k: usize,
    pub y: f64,
    pub se: f64,
    pub tau2: f64,
    pub q: f64,
    /// 0–1.
    pub i2: f64,
}

/// DerSimonian–Laird across `obs` (unusable entries ignored). One study:
/// itself, with τ² = 0 and I² = 0.
pub fn dersimonian_laird(obs: &[Obs]) -> Option<ReFit> {
    let xs: Vec<Obs> = obs.iter().copied().filter(|o| o.usable()).collect();
    let k = xs.len();
    let fe = fixed_pool(&xs)?;
    if k == 1 {
        return Some(ReFit {
            k,
            y: fe.y,
            se: fe.se,
            tau2: 0.0,
            q: 0.0,
            i2: 0.0,
        });
    }
    let w: Vec<f64> = xs.iter().map(|o| 1.0 / (o.se * o.se)).collect();
    let sw: f64 = w.iter().sum();
    let sw2: f64 = w.iter().map(|x| x * x).sum();
    let q: f64 = xs
        .iter()
        .zip(&w)
        .map(|(o, wi)| wi * (o.y - fe.y).powi(2))
        .sum();
    let c = sw - sw2 / sw;
    let df = (k - 1) as f64;
    let tau2 = if c > 0.0 {
        ((q - df) / c).max(0.0)
    } else {
        0.0
    };
    let re = fixed_pool(
        &xs.iter()
            .map(|o| Obs {
                y: o.y,
                se: (o.se * o.se + tau2).sqrt(),
            })
            .collect::<Vec<_>>(),
    )?;
    let i2 = if q > 0.0 {
        ((q - df) / q).max(0.0)
    } else {
        0.0
    };
    Some(ReFit {
        k,
        y: re.y,
        se: re.se,
        tau2,
        q,
        i2,
    })
}

/// The predictive SD for the next study: sqrt(var_re + τ² + 0.15²).
pub fn predictive_sd(re: &ReFit) -> f64 {
    (re.se * re.se + re.tau2 + PRED_SE * PRED_SE).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn cdf() {
        assert!(close(norm_cdf(0.0), 0.5, 1e-7));
        assert!(close(norm_cdf(1.959_964), 0.975, 1e-6));
        assert!(close(norm_cdf(-1.0), 0.158_655_25, 1e-6));
        assert!(close(norm_cdf(3.0), 0.998_650_1, 1e-6));
        assert!(norm_cdf(f64::NAN).is_nan());
    }

    #[test]
    fn fixed_pool_basics_and_order_invariance() {
        let a = Obs { y: 0.4, se: 0.1 };
        let b = Obs { y: -0.2, se: 0.2 };
        let c = Obs { y: 0.1, se: 0.3 };
        let p1 = fixed_pool(&[a, b, c]).unwrap();
        let p2 = fixed_pool(&[c, a, b]).unwrap();
        assert!(close(p1.y, p2.y, 1e-12) && close(p1.se, p2.se, 1e-12));
        let w = [100.0, 25.0, 1.0 / 0.09];
        let y = (0.4 * w[0] - 0.2 * w[1] + 0.1 * w[2]) / (w[0] + w[1] + w[2]);
        assert!(close(p1.y, y, 1e-12));
        // Guards.
        assert!(fixed_pool(&[]).is_none());
        assert!(
            fixed_pool(&[
                Obs { y: 1.0, se: 0.0 },
                Obs {
                    y: f64::NAN,
                    se: 0.1
                }
            ])
            .is_none()
        );
        assert_eq!(
            fixed_pool(&[a, Obs { y: 1.0, se: -1.0 }]).unwrap(),
            fixed_pool(&[a]).unwrap()
        );
    }

    #[test]
    fn kalman_at_tau_zero_is_order_invariant_and_matches_the_conjugate_pool() {
        let obs = [
            Some(Obs {
                y: 0.476,
                se: 0.118,
            }),
            None,
            Some(Obs {
                y: -0.386,
                se: 0.163,
            }),
            Some(Obs { y: 0.1, se: 0.4 }),
        ];
        let rev: Vec<Option<Obs>> = obs.iter().rev().copied().collect();
        let (a, lla) = kalman(&obs, 0.0, PRIOR_SD);
        let (b, llb) = kalman(&rev, 0.0, PRIOR_SD);
        let (fa, fb) = (a.last().unwrap(), b.last().unwrap());
        assert!(close(fa.mean, fb.mean, 1e-12) && close(fa.var, fb.var, 1e-12));
        assert!(close(lla, llb, 1e-9), "{lla} {llb}");
        // Same as pooling the prior as one more observation.
        let mut all: Vec<Obs> = obs.iter().flatten().copied().collect();
        all.push(Obs {
            y: PRIOR_MEAN,
            se: PRIOR_SD,
        });
        let p = fixed_pool(&all).unwrap();
        assert!(close(fa.mean, p.y, 1e-12) && close(fa.var, p.se * p.se, 1e-12));
    }

    #[test]
    fn kalman_drift_and_empty_steps() {
        let (p, ll) = kalman(&[None, None], 0.5, PRIOR_SD);
        assert_eq!(
            p[0],
            Posterior {
                mean: 0.0,
                var: 1.0
            }
        );
        assert!(close(p[1].var, 1.25, 1e-12));
        assert_eq!(ll, 0.0);
        let (p, _) = kalman(&[], 0.5, PRIOR_SD);
        assert!(p.is_empty());
        // With drift, the latest era counts more.
        let obs = [
            Some(Obs { y: 1.0, se: 0.1 }),
            Some(Obs { y: -1.0, se: 0.1 }),
        ];
        let (p0, _) = kalman(&obs, 0.0, PRIOR_SD);
        let (p1, _) = kalman(&obs, 1.0, PRIOR_SD);
        assert!(p1[1].mean < p0[1].mean);
    }

    #[test]
    fn dl_matches_hand_values() {
        // Two discordant studies: conversion in the cholecystitis run.
        let a = Obs {
            y: 1.61f64.ln(),
            se: (2.03f64.ln() - 1.28f64.ln()) / 3.92,
        };
        let b = Obs {
            y: 0.68f64.ln(),
            se: (0.93f64.ln() - 0.49f64.ln()) / 3.92,
        };
        let re = dersimonian_laird(&[a, b]).unwrap();
        let (wa, wb) = (1.0 / (a.se * a.se), 1.0 / (b.se * b.se));
        let fe = (wa * a.y + wb * b.y) / (wa + wb);
        let q = wa * (a.y - fe).powi(2) + wb * (b.y - fe).powi(2);
        let tau2 = (q - 1.0) / (wa + wb - (wa * wa + wb * wb) / (wa + wb));
        assert!(close(re.q, q, 1e-9) && close(re.tau2, tau2, 1e-9));
        assert!(re.i2 > 0.9);
        let rb = dersimonian_laird(&[b, a]).unwrap();
        assert!(close(re.y, rb.y, 1e-12));
        // One study: itself.
        let one = dersimonian_laird(&[a]).unwrap();
        assert!(close(one.y, a.y, 1e-12) && one.tau2 == 0.0 && one.i2 == 0.0);
        // Homogeneous: τ² = 0.
        let h = dersimonian_laird(&[a, a]).unwrap();
        assert_eq!(h.tau2, 0.0);
        assert!(dersimonian_laird(&[]).is_none());
        let pred = predictive_sd(&one);
        assert!(close(pred, (a.se * a.se + 0.0225).sqrt(), 1e-12));
    }

    #[test]
    fn z() {
        let z = z_vs_prior(
            Obs { y: 1.0, se: 0.3 },
            Posterior {
                mean: 0.0,
                var: 0.16,
            },
        )
        .unwrap();
        assert!(close(z, 2.0, 1e-12));
        assert!(
            z_vs_prior(
                Obs { y: 1.0, se: 0.0 },
                Posterior {
                    mean: 0.0,
                    var: 0.0
                }
            )
            .is_none()
        );
    }

    #[test]
    fn scale_for_differences_is_in_data_units() {
        let r = Scale::for_family(true, &[]).unwrap();
        assert_eq!((r.prior_sd, r.tau_unit), (PRIOR_SD, 1.0));
        let d = Scale::for_family(false, &[8.0, 2.0, 4.0]).unwrap();
        assert_eq!((d.prior_sd, d.tau_unit), (40.0, 4.0));
        assert!(Scale::for_family(false, &[]).is_none());
        assert_eq!(median([1.0, 3.0, 2.0, 4.0]), Some(2.5));
        assert_eq!(finite(f64::INFINITY), f64::MAX);
        assert_eq!(finite(f64::NAN), 0.0);
    }
}
