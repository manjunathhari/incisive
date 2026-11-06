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
    /// Meta-analysis or review that is stated to pool more than randomised
    /// trials. False means "not stated to include non-RCT designs", not
    /// "RCTs only".
    pub includes_non_rct: bool,
}

fn ranked(rank: Rank, label: &'static str) -> Ranked {
    Ranked {
        rank,
        label,
        includes_non_rct: false,
    }
}

/// Lower-case, with the Unicode hyphens and dashes (U+2010 to U+2015) as "-".
fn normalise(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('\u{2010}'..='\u{2015}').contains(&c) {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .to_lowercase()
}

fn words(t: &str) -> impl Iterator<Item = &str> {
    t.split(|c: char| !c.is_alphanumeric())
}

/// "rct" / "rcts" as a whole word (not inside e.g. "arctic").
fn has_word_rct(t: &str) -> bool {
    words(t).any(|w| w == "rct" || w == "rcts")
}

/// Not a human study of its own: a protocol, commentary, editorial, letter or
/// animal work.
fn is_excluded(t: &str) -> bool {
    [
        "study protocol",
        "trial protocol",
        "review protocol",
        "protocol for",
        "letter to the editor",
        "comment on",
        "in vitro",
    ]
    .iter()
    .any(|p| t.contains(p))
        || words(t).any(|w| {
            matches!(
                w,
                "editorial"
                    | "commentary"
                    | "porcine"
                    | "swine"
                    | "murine"
                    | "mouse"
                    | "mice"
                    | "rat"
                    | "rats"
                    | "rodent"
                    | "canine"
                    | "animal"
                    | "animals"
            )
        })
}

/// Byte offset of the first word that names a study design.
fn first_design(t: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut note = |i: usize| best = Some(best.map_or(i, |b| b.min(i)));
    for w in [
        "randomi",
        "trial",
        "cohort",
        "retrospective",
        "prospective",
        "case series",
        "case report",
        "case-control",
        "case control",
        "meta-analysis",
        "meta analysis",
        "systematic review",
        "observational",
        "comparative",
        "single-arm",
        "single arm",
        "claims",
        "database",
    ] {
        if let Some(i) = t.find(w) {
            note(i);
        }
    }
    for (i, _) in t.match_indices("rct") {
        let before = t[..i].chars().next_back();
        let after = t[i + 3..].chars().next();
        let after = if after == Some('s') {
            t[i + 4..].chars().next()
        } else {
            after
        };
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            note(i);
        }
    }
    best
}

/// "guideline" (not hyphenated, as in "guideline-directed") or "consensus
/// statement" comes before any design word.
fn is_guideline(t: &str) -> bool {
    let mut at: Option<usize> = None;
    for (i, _) in t.match_indices("guideline") {
        if !t[i + 9..].starts_with('-') {
            at = Some(i);
            break;
        }
    }
    if let Some(i) = t.find("consensus statement") {
        at = Some(at.map_or(i, |a| a.min(i)));
    }
    match (at, first_design(t)) {
        (Some(g), Some(d)) => g < d,
        (Some(_), None) => true,
        _ => false,
    }
}

enum Class {
    Hit(Ranked),
    /// Only mentions a study, or is not a human study: never ranked.
    MentionOnly,
    NoMatch,
}

/// Classify one normalised text.
fn classify(t: &str) -> Class {
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    let hit = |rank, label| Class::Hit(ranked(rank, label));
    if is_guideline(t) {
        return hit(Rank::Guideline, "Guideline");
    }
    if has(&[
        "markov",
        "cost-utility",
        "cost utility",
        "decision model",
        "decision-analytic",
    ]) {
        return hit(Rank::Model, "Model");
    }
    if is_excluded(t) {
        return Class::MentionOnly;
    }
    let scoping_or_narrative = t.contains("narrative review") || t.contains("scoping review");
    if has(&["meta-analysis", "meta analysis"])
        || (t.contains("systematic review") && !scoping_or_narrative)
    {
        let mut r = ranked(Rank::Meta, "Meta-analysis");
        r.includes_non_rct = has(&[
            "non-randomi",
            "nonrandomi",
            "non randomi",
            "non-rct",
            "prospective",
            "cohort",
            "observational",
            "retrospective",
            "case series",
            "case-control",
            "case control",
            "registry",
            "registries",
        ]);
        return Class::Hit(r);
    }
    if scoping_or_narrative {
        return Class::MentionOnly;
    }
    let retro = |label| Class::Hit(ranked(Rank::Retrospective, label));
    if t.contains("emulat") {
        return retro(if t.contains("retrospective") {
            "Retrospective"
        } else {
            "Retrospective or unstated"
        });
    }
    if has(&[
        "non-randomi",
        "nonrandomi",
        "non randomi",
        "not randomi",
        "unrandomi",
        "pseudo-randomi",
        "pseudo randomi",
        "non-rct",
        "single-arm",
        "single arm",
        "quasi-randomi",
        "quasi randomi",
        "quasirandomi",
    ]) {
        if t.contains("retrospective") {
            return retro("Retrospective");
        }
        if has(&["case series", "case report"]) {
            return hit(Rank::CaseSeries, "Case series");
        }
        if t.contains("trial")
            || has(&[
                "single-arm",
                "single arm",
                "prospective",
                "quasi-randomi",
                "quasi randomi",
                "quasirandomi",
            ])
        {
            return hit(Rank::Prospective, "Prospective");
        }
        return retro("Retrospective or unstated");
    }
    if has_word_rct(t) || has(&["randomised", "randomized"]) {
        return hit(Rank::Rct, "RCT");
    }
    if has(&["case series", "case report"]) {
        return hit(Rank::CaseSeries, "Case series");
    }
    if t.contains("retrospective") {
        return retro("Retrospective");
    }
    if t.contains("prospective") {
        return hit(Rank::Prospective, "Prospective");
    }
    if has(&["claims", "database"]) {
        return retro("Retrospective");
    }
    if has(&["cohort", "observational", "case-control", "case control"]) {
        return retro("Retrospective or unstated");
    }
    Class::NoMatch
}

pub fn rank_paper(design: &str, pub_types: &[String]) -> Ranked {
    match classify(&normalise(design)) {
        Class::Hit(r) => return r,
        Class::MentionOnly => return ranked(Rank::Unclassified, "Unclassified"),
        Class::NoMatch => {}
    }
    pub_types
        .iter()
        .find_map(|p| match classify(&normalise(p)) {
            Class::Hit(r) => Some(r),
            _ => None,
        })
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

    fn lab(d: &str) -> (Rank, &'static str) {
        let x = rank_paper(d, &[]);
        (x.rank, x.label)
    }

    #[test]
    fn negated_designs_are_not_rct() {
        for d in [
            "Not randomised comparative study",
            "Unrandomised comparison",
            "Non-RCT comparative study",
            "Pseudo-randomised trial",
            "Non\u{2011}randomised trial",
        ] {
            assert_ne!(r(d), Rank::Rct, "{d}");
            assert_ne!(r(d), Rank::Unclassified, "{d}");
        }
        assert_eq!(r("Pseudo-randomised trial"), Rank::Prospective);
        assert_eq!(r("Non\u{2011}randomised trial"), Rank::Prospective);
    }

    #[test]
    fn mere_mentions_of_an_rct_are_unclassified() {
        for d in [
            "Narrative review of RCTs",
            "Study protocol for a randomised trial",
            "Commentary on an RCT",
            "Randomised trial in a porcine model",
            "Editorial",
            "Letter to the editor",
            "Scoping review of randomised trials",
        ] {
            assert_eq!(r(d), Rank::Unclassified, "{d}");
        }
        assert_eq!(r("Systematic review of RCTs"), Rank::Meta);
    }

    #[test]
    fn emulations_are_retrospective() {
        for d in [
            "Target trial emulation of an RCT",
            "Propensity-matched cohort emulating a randomised trial",
        ] {
            assert_eq!(
                lab(d),
                (Rank::Retrospective, "Retrospective or unstated"),
                "{d}"
            );
        }
    }

    #[test]
    fn retrospective_beats_prospective_mentions() {
        assert_eq!(
            lab("Retrospective analysis of a prospectively maintained database"),
            (Rank::Retrospective, "Retrospective")
        );
        for d in [
            "Non-randomised cohort study",
            "Non-randomised comparative study",
        ] {
            assert_eq!(
                lab(d),
                (Rank::Retrospective, "Retrospective or unstated"),
                "{d}"
            );
        }
        assert_eq!(r("Non-randomised trial"), Rank::Prospective);
        assert_eq!(r("Single-arm trial"), Rank::Prospective);
        assert_eq!(r("Non-randomised prospective cohort"), Rank::Prospective);
        assert_eq!(r("Prospective multicentre database"), Rank::Prospective);
        assert_eq!(r("Retrospective non-randomised trial"), Rank::Retrospective);
    }

    #[test]
    fn guideline_word_inside_a_design_does_not_make_a_guideline() {
        assert_eq!(r("RCT of guideline-directed medical therapy"), Rank::Rct);
        assert_eq!(
            r("Retrospective cohort of guideline-concordant care"),
            Rank::Retrospective
        );
        assert_eq!(r("Clinical practice guideline"), Rank::Guideline);
        assert_eq!(r("Consensus statement"), Rank::Guideline);
    }

    #[test]
    fn meta_flags_more_non_rct_designs() {
        for d in [
            "Meta-analysis of 12 case series",
            "Meta-analysis of case-control studies",
            "Meta-analysis of registry data",
        ] {
            assert!(rank_paper(d, &[]).includes_non_rct, "{d}");
        }
    }

    #[test]
    fn case_control_is_retrospective_unstated() {
        assert_eq!(
            lab("Case-control study"),
            (Rank::Retrospective, "Retrospective or unstated")
        );
    }

    #[test]
    fn guideline_first_wins_and_design_first_keeps_design() {
        for d in [
            "Clinical practice guideline based on a systematic review and meta-analysis",
            "Guideline informed by RCTs",
        ] {
            assert_eq!(r(d), Rank::Guideline, "{d}");
        }
        assert_eq!(
            r("Cohort study of guideline adherence"),
            Rank::Retrospective
        );
        assert_eq!(r("RCT of guideline-directed medical therapy"), Rank::Rct);
    }

    #[test]
    fn protocols_and_animal_meta_are_unclassified() {
        for d in [
            "Systematic review protocol",
            "Meta-analysis in a murine model",
        ] {
            assert_eq!(r(d), Rank::Unclassified, "{d}");
        }
        assert_eq!(r("RCT of two surgical protocols"), Rank::Rct);
        assert_eq!(r("Letter to the editor"), Rank::Unclassified);
        assert_eq!(r("Comment on an RCT"), Rank::Unclassified);
        assert_eq!(r("Narrative review of RCTs"), Rank::Unclassified);
    }

    #[test]
    fn non_randomised_case_series_and_quasi_controlled() {
        assert_eq!(r("Non-randomised case series"), Rank::CaseSeries);
        assert_eq!(r("Quasi-randomised controlled study"), Rank::Prospective);
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
