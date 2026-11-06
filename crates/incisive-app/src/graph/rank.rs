//! Ranks a paper from its design line (and PubMed publication types as a
//! fallback). Pure code: an ordered, case-insensitive match.

/// Evidence rank, strongest first. `as_str` matches the `rank` SQL check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Rct,
    Meta,
    Prospective,
    Retrospective,
    CaseSeries,
    Guideline,
    Model,
    Unclassified,
}

impl Rank {
    pub fn as_str(self) -> &'static str {
        match self {
            Rank::Rct => "rct",
            Rank::Meta => "meta",
            Rank::Prospective => "prospective",
            Rank::Retrospective => "retrospective",
            Rank::CaseSeries => "case_series",
            Rank::Guideline => "guideline",
            Rank::Model => "model",
            Rank::Unclassified => "unclassified",
        }
    }

    /// Sort key: RCT lowest, unclassified highest.
    pub fn order(self) -> u8 {
        match self {
            Rank::Rct => 0,
            Rank::Meta => 1,
            Rank::Prospective => 2,
            Rank::Retrospective => 3,
            Rank::CaseSeries => 4,
            Rank::Guideline => 5,
            Rank::Model => 6,
            Rank::Unclassified => 7,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    pub rank: Rank,
    pub label: &'static str,
    /// Meta-analysis or review that pools more than randomised trials.
    pub includes_non_rct: bool,
}

fn ranked(rank: Rank, label: &'static str) -> Ranked {
    Ranked {
        rank,
        label,
        includes_non_rct: false,
    }
}

/// "rct" / "rcts" as a whole word (not inside e.g. "arctic").
fn has_word_rct(t: &str) -> bool {
    t.split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "rct" || w == "rcts")
}

/// Classify one lower-cased text; `None` when nothing matches.
fn classify(t: &str) -> Option<Ranked> {
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["guideline", "consensus"]) {
        return Some(ranked(Rank::Guideline, "Guideline"));
    }
    if has(&[
        "markov",
        "cost-utility",
        "cost utility",
        "decision model",
        "decision-analytic",
    ]) {
        return Some(ranked(Rank::Model, "Model"));
    }
    if has(&["meta-analysis", "meta analysis", "systematic review"]) {
        let mut r = ranked(Rank::Meta, "Meta-analysis");
        r.includes_non_rct = has(&[
            "non-randomi",
            "nonrandomi",
            "non randomi",
            "prospective",
            "cohort",
            "observational",
            "retrospective",
        ]);
        return Some(r);
    }
    if has(&[
        "non-randomi",
        "nonrandomi",
        "non randomi",
        "single-arm",
        "single arm",
        "quasi-randomi",
        "quasi randomi",
        "quasirandomi",
    ]) {
        if t.contains("retrospective") {
            return Some(ranked(Rank::Retrospective, "Retrospective"));
        }
        return Some(ranked(Rank::Prospective, "Prospective"));
    }
    if has_word_rct(t) || has(&["randomised", "randomized"]) {
        return Some(ranked(Rank::Rct, "RCT"));
    }
    if has(&["case series", "case report"]) {
        return Some(ranked(Rank::CaseSeries, "Case series"));
    }
    if t.contains("prospective") {
        return Some(ranked(Rank::Prospective, "Prospective"));
    }
    if has(&["retrospective", "claims", "database"]) {
        return Some(ranked(Rank::Retrospective, "Retrospective"));
    }
    if has(&["cohort", "observational"]) {
        return Some(ranked(Rank::Retrospective, "Retrospective or unstated"));
    }
    None
}

pub fn rank_paper(design: &str, pub_types: &[String]) -> Ranked {
    classify(&design.to_lowercase())
        .or_else(|| pub_types.iter().find_map(|p| classify(&p.to_lowercase())))
        .unwrap_or_else(|| ranked(Rank::Unclassified, "Unclassified"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(d: &str) -> Rank {
        rank_paper(d, &[]).rank
    }

    #[test]
    fn meta_and_non_rct_flag() {
        assert_eq!(r("Meta-analysis of 3 RCTs"), Rank::Meta);
        assert!(!rank_paper("Network meta-analysis of 9 RCTs", &[]).includes_non_rct);
        assert!(rank_paper("Meta-analysis of 20 observational series", &[]).includes_non_rct);
        assert!(rank_paper("Meta-analysis, mostly observational", &[]).includes_non_rct);
        assert!(
            rank_paper(
                "Meta-analysis of randomised and non-randomised studies (≥24 months)",
                &[]
            )
            .includes_non_rct
        );
        assert_eq!(
            r("Systematic review of 31 studies, no meta-analysis"),
            Rank::Meta
        );
    }

    #[test]
    fn rct_family() {
        assert_eq!(r("5-year follow-up of an RCT"), Rank::Rct);
        assert_eq!(r("Secondary analysis of RCT"), Rank::Rct);
        assert_eq!(r("Multicentre randomised non-inferiority trial"), Rank::Rct);
    }

    #[test]
    fn other_groups() {
        assert_eq!(r("Markov cost-utility model"), Rank::Model);
        assert_eq!(r("Retrospective HCUP cohort"), Rank::Retrospective);
        assert_eq!(r("Prospective cohort"), Rank::Prospective);
        assert_eq!(r("Single-arm trial"), Rank::Prospective);
        assert_eq!(r("Non-randomised trial"), Rank::Prospective); // must not read as randomised
        assert_eq!(r("Case series of 12 patients"), Rank::CaseSeries);
        assert_eq!(r("Clinical practice guideline"), Rank::Guideline);
        let c = rank_paper("Multicentre cohort", &[]);
        assert_eq!(
            (c.rank, c.label),
            (Rank::Retrospective, "Retrospective or unstated")
        );
    }

    #[test]
    fn retrospective_and_quasi_do_not_overstate() {
        assert_eq!(
            rank_paper("Retrospective non-randomised comparative study", &[]).label,
            "Retrospective"
        );
        assert_eq!(
            r("Retrospective non-randomised comparative study"),
            Rank::Retrospective
        );
        assert_eq!(r("Retrospective single-arm series"), Rank::Retrospective);
        assert_eq!(r("Quasi-randomised trial"), Rank::Prospective);
        assert_eq!(r("Quasi-randomized trial"), Rank::Prospective);
        assert_eq!(
            r("Retrospective quasi-randomised study"),
            Rank::Retrospective
        );
        let c = rank_paper("Arctic cohort", &[]);
        assert_eq!(
            (c.rank, c.label),
            (Rank::Retrospective, "Retrospective or unstated")
        );
        assert_eq!(r("Pooled RCTs"), Rank::Rct);
        assert_eq!(r("Open-label (RCT)"), Rank::Rct);
    }

    #[test]
    fn fallbacks() {
        assert_eq!(
            rank_paper("", &["Randomized Controlled Trial".into()]).rank,
            Rank::Rct
        );
        assert_eq!(r("Narrative essay"), Rank::Unclassified);
        assert_eq!(r(""), Rank::Unclassified);
    }

    #[test]
    fn labels_and_order() {
        assert_eq!(rank_paper("RCT", &[]).label, "RCT");
        assert_eq!(rank_paper("", &[]).label, "Unclassified");
        assert_eq!(Rank::CaseSeries.as_str(), "case_series");
        assert_eq!(Rank::Rct.order(), 0);
        assert!(Rank::Meta.order() < Rank::Prospective.order());
        assert!(Rank::Model.order() < Rank::Unclassified.order());
    }

    #[test]
    fn fixture_studies_are_all_classified() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/temporal/full");
        let mut seen = 0;
        for name in ["achalasia", "hernia", "cholecystitis"] {
            let text = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            for s in v["answer"]["studies"].as_array().unwrap() {
                let design = s["design"].as_str().unwrap_or("");
                assert_ne!(r(design), Rank::Unclassified, "{name}: {design:?}");
                seen += 1;
            }
        }
        assert!(seen > 0);
    }
}
