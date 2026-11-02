//! Passages: the short texts Gemma may read, built from a stored Full answer
//! and its Expert Opinion. Pure.
//!
//! The answer's own citation markers (`[12345678]`, `[PMID 1; 2]`, `[3]`) are
//! stripped here and kept as `ref_ns`; a number the answer marked "not traced"
//! stays marked in words; a retracted paper's abstract is left out.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::comparison;
use crate::agent::answer_v3::{
    ADDED_LABEL, AnswerV3, NO_GUIDELINE_SOURCE, Segment, Span, marked_runs, number_text,
    split_citations, whole_marked,
};
use crate::memory::extract::parse_board;

/// What a passage was built from. Serialises to the DB `kind` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassageKind {
    BottomLine,
    Outcome,
    Evidence,
    Conflict,
    Applicability,
    WhatWouldChange,
    Guideline,
    ClinicalPractice,
    Notable,
    Opinion,
    Abstract,
    Quote,
    Reference,
    OwnerQa,
    Belief,
}

impl PassageKind {
    /// The value stored in `chat_passages.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            PassageKind::BottomLine => "bottom_line",
            PassageKind::Outcome => "outcome",
            PassageKind::Evidence => "evidence",
            PassageKind::Conflict => "conflict",
            PassageKind::Applicability => "applicability",
            PassageKind::WhatWouldChange => "what_would_change",
            PassageKind::Guideline => "guideline",
            PassageKind::ClinicalPractice => "clinical_practice",
            PassageKind::Notable => "notable",
            PassageKind::Opinion => "opinion",
            PassageKind::Abstract => "abstract",
            PassageKind::Quote => "quote",
            PassageKind::Reference => "reference",
            PassageKind::OwnerQa => "owner_qa",
            PassageKind::Belief => "belief",
        }
    }
}

/// A passage ready to store. `seq` counts from 0 within its kind.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPassage {
    pub kind: PassageKind,
    pub seq: i32,
    pub ref_ns: Vec<i32>,
    pub pmid: Option<String>,
    pub comparisons: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
    BadAnswer,
    NoAnswer,
    BadBoard,
}

impl BuildError {
    pub fn code(&self) -> &'static str {
        match self {
            BuildError::BadAnswer => "bad_answer",
            BuildError::NoAnswer => "no_answer",
            BuildError::BadBoard => "bad_board",
        }
    }
}

/// Longest abstract piece, in words.
pub const ABSTRACT_WORDS: usize = 120;

const NOT_TRACED: &str = " (not traced)";

struct Ctx<'a> {
    /// PMID to reference number.
    refs: HashMap<String, i32>,
    max_ref: i32,
    pairs: &'a [(String, String)],
    out: Vec<NewPassage>,
}

impl Ctx<'_> {
    /// `text` with citation markers removed (their reference numbers go to
    /// `ns`) and runs marked on `field` by `spans` followed by "(not
    /// traced)". Whitespace is collapsed.
    fn clean(&self, text: &str, field: &str, spans: &[Span], ns: &mut Vec<i32>) -> String {
        let mut out = String::with_capacity(text.len());
        for (run, marked) in marked_runs(text, field, spans) {
            for seg in split_citations(&run, |_| true) {
                match seg {
                    Segment::Text(t) => out.push_str(&t),
                    Segment::Cite(ids) => {
                        for id in &ids {
                            if let Some(n) = self.ref_n(id) {
                                ns.push(n);
                            }
                        }
                        let keep = out.trim_end_matches([' ', '\t']).len();
                        out.truncate(keep);
                    }
                }
            }
            if marked {
                out.push_str(NOT_TRACED);
            }
        }
        if whole_marked(field, spans) {
            out.push_str(NOT_TRACED);
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The reference number a marker id stands for: a PMID with a
    /// reference, else a short id that is itself a reference number.
    fn ref_n(&self, id: &str) -> Option<i32> {
        if let Some(n) = self.refs.get(id) {
            return Some(*n);
        }
        let n: i32 = id.parse().ok()?;
        (id.len() < 5 && (1..=self.max_ref).contains(&n)).then_some(n)
    }

    fn pmid_ns(&self, pmids: &[String], ns: &mut Vec<i32>) {
        for p in pmids {
            if let Some(n) = self.refs.get(p.trim()) {
                ns.push(*n);
            }
        }
    }

    /// The record's only pair, if it has exactly one.
    fn sole_pair(&self) -> Option<String> {
        match self.pairs {
            [(a, b)] => Some(comparison::key(a, b)),
            _ => None,
        }
    }

    /// Tags from `text`; a record with one pair gives that pair to a text
    /// that names none.
    fn tags_or_sole(&self, text: &str) -> Vec<String> {
        let t = comparison::tags(text, self.pairs);
        if t.is_empty() {
            self.sole_pair().into_iter().collect()
        } else {
            t
        }
    }

    fn push(
        &mut self,
        kind: PassageKind,
        mut ns: Vec<i32>,
        pmid: Option<String>,
        comparisons: Vec<String>,
        text: String,
    ) {
        if text.trim().is_empty() {
            return;
        }
        ns.sort_unstable();
        ns.dedup();
        let seq = self.out.iter().filter(|p| p.kind == kind).count() as i32;
        self.out.push(NewPassage {
            kind,
            seq,
            ref_ns: ns,
            pmid,
            comparisons,
            text,
        });
    }
}

fn join_nonempty(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn unsourced_note(flag: bool) -> String {
    if flag {
        format!("({NO_GUIDELINE_SOURCE})")
    } else {
        String::new()
    }
}

/// Build every passage of a record. `pairs` are the record's normalised
/// intervention-comparator pairs; `abstracts` are `(pmid, abstract)`.
pub fn build(
    body: &Value,
    board: &Value,
    pairs: &[(String, String)],
    abstracts: &[(String, String)],
) -> Result<Vec<NewPassage>, BuildError> {
    let a: AnswerV3 = serde_json::from_value(body.clone()).map_err(|_| BuildError::BadAnswer)?;
    let w = a.answer.as_ref().ok_or(BuildError::NoAnswer)?;
    let board = parse_board(board).map_err(|_| BuildError::BadBoard)?;

    let refs: HashMap<String, i32> = a
        .references
        .iter()
        .map(|r| (r.pmid.clone(), i32::from(r.n)))
        .collect();
    let max_ref = refs.values().copied().max().unwrap_or(0);
    let mut c = Ctx {
        refs,
        max_ref,
        pairs,
        out: Vec::new(),
    };
    let u = &a.untraced;
    let no_spans: Vec<Span> = Vec::new();
    let cu = a.completeness.as_ref().map_or(&no_spans, |x| &x.untraced);
    // Tags of each study, by PMID.
    let study_tags = |c: &Ctx, pmid: &str| -> Vec<String> {
        w.studies
            .iter()
            .find(|s| s.pmid == pmid)
            .map(|s| comparison::tags(&s.comparison, c.pairs))
            .unwrap_or_default()
    };
    let tags_of_pmids = |c: &Ctx, pmids: &[String]| -> Vec<String> {
        let mut t: Vec<String> = Vec::new();
        for p in pmids {
            for k in study_tags(c, p.trim()) {
                if !t.contains(&k) {
                    t.push(k);
                }
            }
        }
        t
    };
    let sole_if_empty = |c: &Ctx, t: Vec<String>| -> Vec<String> {
        if t.is_empty() {
            c.sole_pair().into_iter().collect()
        } else {
            t
        }
    };
    let single = |pmids: &[String]| -> Option<String> {
        match pmids {
            [p] if !p.trim().is_empty() => Some(p.trim().to_string()),
            _ => None,
        }
    };

    // ---- bottom line and at a glance ----
    {
        let mut ns = Vec::new();
        let b = &w.bottom_line;
        let text = join_nonempty(&[
            c.clean(&b.text, "bottom_line.text", u, &mut ns),
            format!("Certainty: {}.", b.certainty.as_str()),
            c.clean(
                &b.certainty_reason,
                "bottom_line.certainty_reason",
                u,
                &mut ns,
            ),
        ]);
        let tags = c.tags_or_sole(&text);
        c.push(PassageKind::BottomLine, ns, None, tags, text);
    }
    if let Some(g) = &w.at_a_glance {
        let mut ns = Vec::new();
        let mut parts = vec![c.clean(&g.verdict, "at_a_glance.verdict", u, &mut ns)];
        for (i, k) in g.key_numbers.iter().enumerate() {
            let label = c.clean(
                &k.label,
                &format!("at_a_glance.key_numbers.{i}.label"),
                u,
                &mut ns,
            );
            let value = c.clean(
                &k.value_text,
                &format!("at_a_glance.key_numbers.{i}.value_text"),
                u,
                &mut ns,
            );
            c.pmid_ns(std::slice::from_ref(&k.pmid), &mut ns);
            parts.push(if label.is_empty() {
                value
            } else {
                format!("{label}: {value}")
            });
        }
        if !g.main_caveat.is_empty() {
            let cv = c.clean(&g.main_caveat, "at_a_glance.main_caveat", u, &mut ns);
            parts.push(format!("Main caveat: {cv}"));
        }
        let text = join_nonempty(&parts);
        let tags = c.tags_or_sole(&text);
        c.push(PassageKind::BottomLine, ns, None, tags, text);
    }

    // ---- outcomes ----
    for (i, o) in w.outcomes.iter().enumerate() {
        let mut ns = Vec::new();
        let mut parts = vec![
            format!(
                "Outcome: {}.",
                c.clean(&o.outcome, &format!("outcomes.{i}.outcome"), u, &mut ns)
            ),
            c.clean(
                &o.effect_text,
                &format!("outcomes.{i}.effect_text"),
                u,
                &mut ns,
            ),
        ];
        let rct = c.clean(
            &o.rct_evidence,
            &format!("outcomes.{i}.rct_evidence"),
            u,
            &mut ns,
        );
        if !rct.is_empty() {
            parts.push(format!("Randomised trial evidence: {rct}"));
        }
        if let Some(l) = o.evidence_type.label() {
            parts.push(format!("Evidence type: {l}."));
        }
        for (j, n) in o.numbers.iter().enumerate() {
            let mark = if whole_marked(&format!("outcomes.{i}.numbers.{j}"), u) {
                NOT_TRACED
            } else {
                ""
            };
            c.pmid_ns(std::slice::from_ref(&n.pmid), &mut ns);
            let label = n.label.trim();
            let line = if label.is_empty() {
                number_text(n)
            } else {
                format!("{label}: {}", number_text(n))
            };
            parts.push(format!("Number: {line}{mark}."));
        }
        parts.push(format!("Certainty: {}.", o.certainty.as_str()));
        let why = c.clean(&o.why, &format!("outcomes.{i}.why"), u, &mut ns);
        if !why.is_empty() {
            parts.push(format!("Why: {why}"));
        }
        if let Some(comp) = &a.completeness {
            for (k, add) in comp.additions_to(i) {
                let t = c.clean(
                    &add.text,
                    &format!("outcome_additions.{k}.text"),
                    cu,
                    &mut ns,
                );
                c.pmid_ns(&add.pmids, &mut ns);
                parts.push(format!(
                    "Added on completeness review: {t} {}",
                    unsourced_note(add.unsourced)
                ));
            }
        }
        c.pmid_ns(&o.pmids, &mut ns);
        // Own words and arms first; the studies' comparisons only when the
        // row names no pair; then the record's sole pair.
        let mut own = vec![
            o.outcome.clone(),
            o.effect_text.clone(),
            o.rct_evidence.clone(),
            o.why.clone(),
        ];
        own.extend(o.numbers.iter().filter_map(|n| n.arm.clone()));
        if let Some(comp) = &a.completeness {
            own.extend(comp.additions_to(i).map(|(_, add)| add.text.clone()));
        }
        let mut tags = comparison::tags(&own.join(" , "), pairs);
        if tags.is_empty() {
            tags = tags_of_pmids(&c, &o.pmids);
        }
        let text = join_nonempty(&parts);
        if tags.is_empty() {
            tags = c.tags_or_sole(&text);
        }
        c.push(PassageKind::Outcome, ns, single(&o.pmids), tags, text);
    }
    if let Some(comp) = &a.completeness {
        for (k, x) in comp.added_outcomes.iter().enumerate() {
            let mut ns = Vec::new();
            let name = c.clean(
                &x.outcome,
                &format!("added_outcomes.{k}.outcome"),
                cu,
                &mut ns,
            );
            let effect = c.clean(
                &x.effect_text,
                &format!("added_outcomes.{k}.effect_text"),
                cu,
                &mut ns,
            );
            let why = c.clean(&x.why, &format!("added_outcomes.{k}.why"), cu, &mut ns);
            c.pmid_ns(&x.pmids, &mut ns);
            let cert = x.certainty.map_or("not graded", |c| c.as_str());
            let text = join_nonempty(&[
                format!("Outcome: {name} ({ADDED_LABEL})."),
                effect,
                format!("Certainty: {cert}."),
                if why.is_empty() {
                    String::new()
                } else {
                    format!("Why: {why}")
                },
                unsourced_note(x.unsourced),
            ]);
            let own = [x.outcome.as_str(), x.effect_text.as_str(), x.why.as_str()].join(" , ");
            let mut tags = comparison::tags(&own, pairs);
            if tags.is_empty() {
                tags = tags_of_pmids(&c, &x.pmids);
            }
            if tags.is_empty() {
                tags = c.tags_or_sole(&text);
            }
            c.push(PassageKind::Outcome, ns, single(&x.pmids), tags, text);
        }
    }

    // ---- evidence table ----
    for (i, s) in w.studies.iter().enumerate() {
        let mut ns = Vec::new();
        c.pmid_ns(std::slice::from_ref(&s.pmid), &mut ns);
        let f = |c: &Ctx, t: &str, name: &str, ns: &mut Vec<i32>| {
            c.clean(t, &format!("studies.{i}.{name}"), u, ns)
        };
        let label = f(&c, &s.label, "label", &mut ns);
        let design = f(&c, &s.design, "design", &mut ns);
        let setting = f(&c, &s.setting, "setting", &mut ns);
        let comp = f(&c, &s.comparison, "comparison", &mut ns);
        let key_results = f(&c, &s.key_results, "key_results", &mut ns);
        let appraisal = f(&c, &s.appraisal, "appraisal", &mut ns);
        let n = s.n.map(|n| format!("n = {n}")).unwrap_or_default();
        let text = join_nonempty(&[
            format!("Study: {label}."),
            format!("Design: {design}."),
            n,
            if setting.is_empty() {
                String::new()
            } else {
                format!("Setting: {setting}.")
            },
            format!("Comparison: {comp}."),
            format!("Key results: {key_results}"),
            format!("Appraisal: {appraisal}"),
        ]);
        let tags = sole_if_empty(&c, comparison::tags(&s.comparison, pairs));
        let pmid = (!s.pmid.trim().is_empty()).then(|| s.pmid.trim().to_string());
        c.push(PassageKind::Evidence, ns, pmid, tags, text);
    }

    // ---- conflicts ----
    let conflict =
        |c: &mut Ctx, raw: &str, field: &str, spans: &[Span], pmids: &[String], extra: &str| {
            let mut ns = Vec::new();
            let t = c.clean(raw, field, spans, &mut ns);
            c.pmid_ns(pmids, &mut ns);
            let text = join_nonempty(&[format!("Conflicting evidence: {t}"), extra.to_string()]);
            let mut tags = tags_of_pmids(c, pmids);
            if tags.is_empty() {
                tags = c.tags_or_sole(&text);
            }
            c.push(PassageKind::Conflict, ns, single(pmids), tags, text);
        };
    for (i, x) in w.conflicts.iter().enumerate() {
        conflict(
            &mut c,
            &x.text,
            &format!("conflicts.{i}.text"),
            u,
            &x.pmids,
            "",
        );
    }
    if let Some(comp) = &a.completeness {
        for (k, x) in comp.added_conflicts.iter().enumerate() {
            let extra = format!("({ADDED_LABEL}) {}", unsourced_note(x.unsourced));
            conflict(
                &mut c,
                &x.text,
                &format!("added_conflicts.{k}.text"),
                cu,
                &x.pmids,
                extra.trim(),
            );
        }
    }

    // ---- applicability, what would change ----
    {
        let mut ns = Vec::new();
        let ap = &w.applicability;
        let mut parts = Vec::new();
        for (label, t, f) in [
            ("Case mix", &ap.case_mix, "case_mix"),
            (
                "Cost in India",
                &ap.india_cost_context,
                "india_cost_context",
            ),
            ("Training", &ap.training, "training"),
        ] {
            let t = c.clean(t, &format!("applicability.{f}"), u, &mut ns);
            if !t.is_empty() {
                parts.push(format!("{label}: {t}"));
            }
        }
        let text = join_nonempty(&parts);
        let tags = c.tags_or_sole(&text);
        c.push(PassageKind::Applicability, ns, None, tags, text);
    }
    for (i, t) in w.would_change.iter().enumerate() {
        let mut ns = Vec::new();
        let t = c.clean(t, &format!("would_change.{i}"), u, &mut ns);
        let text = format!("What would change this answer: {t}");
        let tags = c.tags_or_sole(&text);
        c.push(PassageKind::WhatWouldChange, ns, None, tags, text);
    }

    // ---- guidelines, clinical practice, notable findings ----
    for (i, g) in w.guidelines.iter().enumerate() {
        let mut ns = Vec::new();
        let body = c.clean(&g.body, &format!("guidelines.{i}.body"), u, &mut ns);
        let st = c.clean(
            &g.statement,
            &format!("guidelines.{i}.statement"),
            u,
            &mut ns,
        );
        let pmid = g.pmid.as_deref().map(str::trim).filter(|p| !p.is_empty());
        if let Some(p) = pmid {
            c.pmid_ns(&[p.to_string()], &mut ns);
        }
        let year = g.year.map(|y| format!(" ({y})")).unwrap_or_default();
        let text = join_nonempty(&[
            format!("Guideline: {body}{year}."),
            st,
            if pmid.is_none() {
                format!("({NO_GUIDELINE_SOURCE})")
            } else {
                String::new()
            },
        ]);
        let tags = c.tags_or_sole(&text);
        c.push(
            PassageKind::Guideline,
            ns,
            pmid.map(str::to_string),
            tags,
            text,
        );
    }
    if let Some(p) = &w.clinical_practice {
        let mut ns = Vec::new();
        let mut parts = Vec::new();
        for (label, t, f) in [
            ("Default choice", &p.default_choice, "default_choice"),
            ("Who may benefit", &p.who_may_benefit, "who_may_benefit"),
            ("Counselling", &p.counselling, "counselling"),
            ("Cost per case", &p.cost_per_case, "cost_per_case"),
        ] {
            let t = c.clean(t, &format!("clinical_practice.{f}"), u, &mut ns);
            if !t.is_empty() {
                parts.push(format!("{label}: {t}"));
            }
        }
        parts.push(c.clean(&p.text, "clinical_practice.text", u, &mut ns));
        let text = join_nonempty(&parts);
        let tags = c.tags_or_sole(&text);
        c.push(PassageKind::ClinicalPractice, ns, None, tags, text);
    }
    for (i, n) in w.notable_findings.iter().enumerate() {
        let mut ns = Vec::new();
        let t = c.clean(&n.text, &format!("notable_findings.{i}.text"), u, &mut ns);
        c.pmid_ns(&n.pmids, &mut ns);
        let mut tags = tags_of_pmids(&c, &n.pmids);
        if tags.is_empty() {
            tags = c.tags_or_sole(&t);
        }
        c.push(PassageKind::Notable, ns, single(&n.pmids), tags, t);
    }

    // ---- Expert Opinion ----
    {
        let none: Vec<Span> = Vec::new();
        let sole: Vec<String> = c.sole_pair().into_iter().collect();
        let op = |c: &mut Ctx, text: String, ns: Vec<i32>| {
            let mut tags = comparison::tags(&text, pairs);
            if tags.is_empty() {
                tags = sole.clone();
            }
            c.push(PassageKind::Opinion, ns, None, tags, text);
        };
        let o = &board.opinion;
        let mut ns = Vec::new();
        let t = c.clean(&o.verdict, "", &none, &mut ns);
        if !t.is_empty() {
            op(&mut c, format!("Expert verdict: {t}"), ns);
        }
        let mut ns = Vec::new();
        let t = c.clean(&o.why, "", &none, &mut ns);
        if !t.is_empty() {
            op(&mut c, format!("My view on why: {t}"), ns);
        }
        let g = &board.at_a_glance;
        for (label, block) in [
            ("At a glance, the original view", &g.original_quote),
            ("At a glance, the expert view", &g.expert),
            ("At a glance, today", &g.today),
        ] {
            for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let mut ns = Vec::new();
                let t = c.clean(line, "", &none, &mut ns);
                if !t.is_empty() {
                    op(&mut c, format!("{label}: {t}"), ns);
                }
            }
        }
        for e in &board.eras {
            let mut ns = Vec::new();
            let era = e.era_id.trim();
            let mut parts = Vec::new();
            for (label, t) in [
                ("Going in", &e.going_in),
                ("What it showed", &e.showed),
                ("Knowledge after", &e.knowledge_after),
            ] {
                let t = c.clean(t, "", &none, &mut ns);
                if !t.is_empty() {
                    parts.push(format!("{label}: {t}"));
                }
            }
            let head = if era.is_empty() {
                "Era note.".to_string()
            } else {
                format!("Era {era}.")
            };
            if !parts.is_empty() {
                op(&mut c, format!("{head} {}", parts.join(" ")), ns);
            }
        }
        for d in &board.decisions {
            let mut ns = Vec::new();
            let mut parts = Vec::new();
            for (label, t) in [
                ("Scenario", &d.scenario),
                ("Choice", &d.choice),
                ("Reason", &d.reason),
            ] {
                let t = c.clean(t, "", &none, &mut ns);
                if !t.is_empty() {
                    parts.push(format!("{label}: {t}"));
                }
            }
            if !parts.is_empty() {
                op(&mut c, format!("Decision. {}", parts.join(" ")), ns);
            }
        }
    }

    // ---- references and abstracts ----
    let design_of = |pmid: &str| -> String {
        w.studies
            .iter()
            .find(|s| s.pmid == pmid)
            .map(|s| s.design.clone())
            .unwrap_or_default()
    };
    for r in &a.references {
        let mut ns = vec![i32::from(r.n)];
        let title = c.clean(&r.title, "", &no_spans, &mut ns);
        let venue = match (r.journal.as_deref().map(str::trim), r.year) {
            (Some(j), Some(y)) if !j.is_empty() => format!("{j}, {y}"),
            (Some(j), None) if !j.is_empty() => j.to_string(),
            (_, Some(y)) => y.to_string(),
            _ => String::new(),
        };
        let design = c.clean(&design_of(&r.pmid), "", &no_spans, &mut ns);
        let text = join_nonempty(&[
            format!("Reference {}: {title}", r.n),
            if venue.is_empty() {
                String::new()
            } else {
                format!("({venue}).")
            },
            if design.is_empty() {
                String::new()
            } else {
                format!("Design: {design}.")
            },
            if r.retracted {
                "RETRACTED: this paper has been retracted and should not be relied on.".to_string()
            } else {
                String::new()
            },
        ]);
        let tags = sole_if_empty(&c, study_tags(&c, &r.pmid));
        c.push(
            PassageKind::Reference,
            ns,
            Some(r.pmid.clone()).filter(|p| !p.is_empty()),
            tags,
            text,
        );
    }
    for r in a.references.iter().filter(|r| !r.retracted) {
        let Some((_, abs)) = abstracts.iter().find(|(p, _)| *p == r.pmid) else {
            continue;
        };
        let tags = sole_if_empty(&c, study_tags(&c, &r.pmid));
        for piece in split_abstract(abs, ABSTRACT_WORDS) {
            let mut ns = vec![i32::from(r.n)];
            let text = c.clean(&piece, "", &no_spans, &mut ns);
            c.push(
                PassageKind::Abstract,
                ns,
                Some(r.pmid.clone()),
                tags.clone(),
                text,
            );
        }
    }

    // ---- skeptic quotes ----
    if let Some(k) = &a.skeptic {
        let lists: [(&str, &str, &Vec<crate::agent::answer_v3::SkepticItem>); 5] = [
            ("contradicting", "Contradicting evidence", &k.contradicting),
            (
                "missed_rcts",
                "A randomised trial the search missed",
                &k.missed_rcts,
            ),
            ("guidelines", "Guideline", &k.guidelines),
            ("ma_notes", "What a meta-analysis pooled", &k.ma_notes),
            (
                "extract_next",
                "Study picked for a second look",
                &k.extract_next,
            ),
        ];
        for (field, label, items) in lists {
            for (i, it) in items.iter().enumerate() {
                let Some(q) = it.quote.as_deref().map(str::trim).filter(|q| !q.is_empty()) else {
                    continue;
                };
                let mut ns = Vec::new();
                let note = c.clean(&it.text, &format!("{field}.{i}.text"), &k.spans, &mut ns);
                let quote = c.clean(q, "", &no_spans, &mut ns);
                c.pmid_ns(std::slice::from_ref(&it.pmid), &mut ns);
                let text = join_nonempty(&[
                    format!("{label}: {note}"),
                    format!("Quoted from the paper: \"{quote}\""),
                ]);
                let tags = sole_if_empty(&c, study_tags(&c, it.pmid.trim()));
                let pmid = Some(it.pmid.trim().to_string()).filter(|p| !p.is_empty());
                c.push(PassageKind::Quote, ns, pmid, tags, text);
            }
        }
    }

    Ok(c.out)
}

/// An abstract in pieces of at most `max_words` words, cut at sentence ends.
/// A single sentence longer than the limit is cut at the limit.
pub fn split_abstract(text: &str, max_words: usize) -> Vec<String> {
    let mut sentences: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let toks: Vec<&str> = text.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        cur.push(t);
        let next = toks.get(i + 1);
        if ends_sentence(t) && next.is_none_or(|n| starts_sentence(n)) {
            sentences.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        sentences.push(cur);
    }
    let max_words = max_words.max(1);
    let mut pieces: Vec<Vec<&str>> = Vec::new();
    let mut piece: Vec<&str> = Vec::new();
    for s in sentences {
        if s.len() > max_words {
            if !piece.is_empty() {
                pieces.push(std::mem::take(&mut piece));
            }
            pieces.extend(s.chunks(max_words).map(<[&str]>::to_vec));
            continue;
        }
        if piece.len() + s.len() > max_words {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.extend(s);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces.into_iter().map(|p| p.join(" ")).collect()
}

fn ends_sentence(tok: &str) -> bool {
    let t = tok.trim_end_matches(['"', '\'', ')', ']']);
    if !t.ends_with(['.', '?', '!']) {
        return false;
    }
    let word = t.trim_end_matches(['.', '?', '!']).to_lowercase();
    // A single letter or a known abbreviation ends no sentence.
    let abbrev = [
        "vs", "al", "e.g", "i.e", "fig", "approx", "no", "dr", "ca", "cf", "st",
    ];
    !(word.chars().count() == 1 && t.ends_with('.'))
        && !(t.ends_with('.') && abbrev.contains(&word.as_str()))
}

fn starts_sentence(tok: &str) -> bool {
    tok.trim_start_matches(['"', '\'', '(', '['])
        .chars()
        .next()
        .is_none_or(|c| c.is_uppercase() || c.is_ascii_digit())
}

/// Three questions from the first outcomes of an answer, for the empty chat.
pub fn starters(body: &Value) -> Vec<String> {
    let Ok(a) = serde_json::from_value::<AnswerV3>(body.clone()) else {
        return Vec::new();
    };
    let Some(w) = a.answer else {
        return Vec::new();
    };
    w.outcomes
        .iter()
        .map(|o| o.outcome.trim())
        .filter(|o| !o.is_empty())
        .take(3)
        .map(|o| format!("What does the evidence show about {}?", lower_first(o)))
        .collect()
}

/// Lower-case the first letter of an outcome name unless it opens with an
/// acronym ("HRQoL", "ICU stay").
fn lower_first(s: &str) -> String {
    let mut ch = s.chars();
    let (Some(a), second) = (ch.next(), ch.next()) else {
        return String::new();
    };
    if second.is_some_and(char::is_uppercase) {
        return s.to_string();
    }
    a.to_lowercase().chain(s.chars().skip(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temporal::{TemporalInput, build as build_table};
    use regex::Regex;
    use serde_json::json;

    const FIXTURES: [&str; 3] = ["achalasia", "hernia", "cholecystitis"];

    fn dir() -> String {
        format!("{}/tests/fixtures/temporal", env!("CARGO_MANIFEST_DIR"))
    }
    fn read(p: String) -> Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }
    fn body(f: &str) -> Value {
        read(format!("{}/full/{f}.json", dir()))
    }
    fn board(f: &str) -> Value {
        read(format!("{}/boards/{f}-board.json", dir()))["board"].clone()
    }
    fn pairs(f: &str) -> Vec<(String, String)> {
        let s = std::fs::read_to_string(format!("{}/{f}.json", dir())).unwrap();
        let t = build_table(&TemporalInput::from_json(&s).unwrap());
        let mut p: Vec<(String, String)> = t
            .outcomes
            .iter()
            .flat_map(|o| &o.groups)
            .map(|g| {
                (
                    crate::memory::extract::normalise_arm(&g.intervention),
                    crate::memory::extract::normalise_arm(&g.comparator),
                )
            })
            .collect();
        p.sort();
        p.dedup();
        p
    }
    fn built(f: &str) -> Vec<NewPassage> {
        build(&body(f), &board(f), &pairs(f), &[]).unwrap()
    }
    fn of(ps: &[NewPassage], k: PassageKind) -> Vec<&NewPassage> {
        ps.iter().filter(|p| p.kind == k).collect()
    }
    fn len(v: &Value) -> usize {
        v.as_array().map_or(0, Vec::len)
    }

    #[test]
    fn kinds_serialise_as_snake_case_strings() {
        for k in [
            PassageKind::BottomLine,
            PassageKind::WhatWouldChange,
            PassageKind::ClinicalPractice,
            PassageKind::OwnerQa,
            PassageKind::Belief,
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
        }
        assert_eq!(PassageKind::WhatWouldChange.as_str(), "what_would_change");
    }

    #[test]
    fn counts_by_kind_match_fixture() {
        for f in FIXTURES {
            let b = body(f);
            let ps = built(f);
            let c = &b["completeness"];
            assert_eq!(
                of(&ps, PassageKind::Outcome).len(),
                len(&b["answer"]["outcomes"]) + len(&c["added_outcomes"]),
                "{f} outcome"
            );
            assert_eq!(
                of(&ps, PassageKind::Evidence).len(),
                len(&b["answer"]["studies"]),
                "{f}"
            );
            assert_eq!(
                of(&ps, PassageKind::Reference).len(),
                len(&b["references"]),
                "{f}"
            );
            assert_eq!(
                of(&ps, PassageKind::Conflict).len(),
                len(&b["answer"]["conflicts"]) + len(&c["added_conflicts"]),
                "{f} conflict"
            );
            let quotes: usize = [
                "contradicting",
                "missed_rcts",
                "guidelines",
                "ma_notes",
                "extract_next",
            ]
            .iter()
            .map(|l| {
                b["skeptic"][*l].as_array().map_or(0, |a| {
                    a.iter()
                        .filter(|i| i["quote"].as_str().is_some_and(|q| !q.trim().is_empty()))
                        .count()
                })
            })
            .sum();
            assert!(quotes > 0, "{f} has skeptic quotes");
            assert_eq!(of(&ps, PassageKind::Quote).len(), quotes, "{f} quote");
            assert!(!of(&ps, PassageKind::Opinion).is_empty(), "{f}");
            assert!(!of(&ps, PassageKind::BottomLine).is_empty(), "{f}");
            assert_eq!(
                of(&ps, PassageKind::WhatWouldChange).len(),
                len(&b["answer"]["would_change"]),
                "{f}"
            );
            // seq counts from 0 within a kind and is unique.
            let mut seen = std::collections::HashSet::new();
            for p in &ps {
                assert!(seen.insert((p.kind, p.seq)), "{f} {:?} {}", p.kind, p.seq);
                assert!(!p.text.trim().is_empty());
            }
        }
    }

    #[test]
    fn outcome_additions_reach_their_outcome_passage() {
        let mut b = body("hernia");
        let first = b["answer"]["outcomes"][1]["outcome"]
            .as_str()
            .unwrap()
            .to_string();
        b["completeness"] = json!({
            "outcome_additions": [{
                "outcome": first, "row": 1, "text": "Zzyzx extra sentence [999999].", "pmids": []
            }]
        });
        let ps = build(&b, &board("hernia"), &pairs("hernia"), &[]).unwrap();
        let outs = of(&ps, PassageKind::Outcome);
        assert!(outs[1].text.contains("Zzyzx extra sentence."));
        assert!(outs[1].text.contains("Added on completeness review"));
        assert_eq!(outs.iter().filter(|p| p.text.contains("Zzyzx")).count(), 1);
    }

    #[test]
    fn citation_tokens_are_stripped() {
        let tok =
            Regex::new(r"\[\s*(?:PMIDs?:?\s*)?\d+(?:\s*[;,]\s*(?:PMIDs?:?\s*)?\d+)*\s*\]").unwrap();
        for f in FIXTURES {
            let b = body(f);
            // The stored answer really has markers, or the test proves nothing.
            assert!(
                tok.is_match(&serde_json::to_string(&b["answer"]).unwrap()),
                "{f}: fixture has no citation markers"
            );
            let ps = built(f);
            for p in &ps {
                assert!(!tok.is_match(&p.text), "{f} {:?}: {}", p.kind, p.text);
                assert!(!p.text.to_lowercase().contains("[pmid"), "{f}: {}", p.text);
            }
            // The markers' reference numbers are kept.
            let refs: Vec<i32> = b["references"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["n"].as_i64().unwrap() as i32)
                .collect();
            let with_ns: Vec<_> = ps.iter().filter(|p| !p.ref_ns.is_empty()).collect();
            assert!(with_ns.len() > 10, "{f}");
            assert!(
                with_ns
                    .iter()
                    .all(|p| p.ref_ns.iter().all(|n| refs.contains(n)))
            );
            // An evidence row's ref_ns is its own paper.
            for p in of(&ps, PassageKind::Evidence) {
                let r = b["references"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["pmid"].as_str() == p.pmid.as_deref());
                if let Some(r) = r {
                    assert!(p.ref_ns.contains(&(r["n"].as_i64().unwrap() as i32)));
                }
            }
        }
    }

    #[test]
    fn citation_stripping_cases() {
        let mut b = body("hernia");
        let pmid = b["references"][0]["pmid"].as_str().unwrap().to_string();
        let n = b["references"][0]["n"].as_i64().unwrap() as i32;
        b["answer"]["bottom_line"]["text"] = json!(format!(
            "Shorter stay [{pmid}] and less pain [PMID: {pmid}; 99999999], see [2] and [Dor] and [95% CI]."
        ));
        let ps = build(&b, &board("hernia"), &pairs("hernia"), &[]).unwrap();
        let t = &of(&ps, PassageKind::BottomLine)[0];
        assert!(
            t.text
                .starts_with("Shorter stay and less pain, see and [Dor] and [95% CI]."),
            "{}",
            t.text
        );
        assert!(t.ref_ns.contains(&n) && t.ref_ns.contains(&2));
    }

    #[test]
    fn not_traced_marks_are_kept_in_words() {
        // Fixture mark: achalasia studies.4.appraisal.
        let b = body("achalasia");
        let ps = built("achalasia");
        let ev = of(&ps, PassageKind::Evidence);
        assert!(ev[4].text.contains("(not traced)"));
        let s = &b["untraced"][0];
        let appraisal = b["answer"]["studies"][4]["appraisal"].as_str().unwrap();
        let (a, l) = (
            s["start"].as_u64().unwrap() as usize,
            s["len"].as_u64().unwrap() as usize,
        );
        let marked = &appraisal[a..a + l];
        assert!(
            ev[4].text.contains(&format!("{marked} (not traced)")),
            "{}",
            ev[4].text
        );
        assert!(
            ev.iter()
                .enumerate()
                .all(|(i, p)| i == 4 || !p.text.contains("(not traced)"))
        );
        // A structured number marked whole.
        let mut b = body("hernia");
        b["untraced"] = json!([{"field": "outcomes.0.numbers.0", "start": 0, "len": 0}]);
        let label = b["answer"]["outcomes"][0]["numbers"][0]["label"]
            .as_str()
            .unwrap()
            .to_string();
        let ps = build(&b, &board("hernia"), &pairs("hernia"), &[]).unwrap();
        let o = &of(&ps, PassageKind::Outcome)[0].text;
        assert!(o.contains("(not traced)"), "{o}");
        assert!(o.contains(&label));
        // A completeness addition's own marks.
        let mut b = body("cholecystitis");
        let eff = b["completeness"]["added_outcomes"][0]["effect_text"]
            .as_str()
            .unwrap()
            .to_string();
        let s0 = b["completeness"]["untraced"][0].clone();
        let (a, l) = (
            s0["start"].as_u64().unwrap() as usize,
            s0["len"].as_u64().unwrap() as usize,
        );
        let ps = build(&b, &board("cholecystitis"), &pairs("cholecystitis"), &[]).unwrap();
        let n_out = len(&b["answer"]["outcomes"]);
        let added = &of(&ps, PassageKind::Outcome)[n_out].text;
        assert!(
            added.contains(&format!("{} (not traced)", &eff[a..a + l])),
            "{added}"
        );
        b["completeness"]["untraced"] = json!([]);
        let ps = build(&b, &board("cholecystitis"), &pairs("cholecystitis"), &[]).unwrap();
        assert!(
            !of(&ps, PassageKind::Outcome)[n_out]
                .text
                .contains("(not traced)")
        );
    }

    fn abstract_for(f: &str, text: &str) -> (Vec<NewPassage>, String, i32) {
        let b = body(f);
        let pmid = b["references"][0]["pmid"].as_str().unwrap().to_string();
        let n = b["references"][0]["n"].as_i64().unwrap() as i32;
        let ps = build(
            &b,
            &board(f),
            &pairs(f),
            &[(pmid.clone(), text.to_string())],
        )
        .unwrap();
        (ps, pmid, n)
    }

    #[test]
    fn abstract_splits_at_sentences_to_120_words() {
        // 30 sentences of 11 words: pieces pack whole sentences up to 120.
        let sentence = "Patients were followed for twelve months after the operation was done.";
        let text = std::iter::repeat_n(sentence, 30)
            .collect::<Vec<_>>()
            .join(" ");
        let (ps, pmid, n) = abstract_for("hernia", &text);
        let abs = of(&ps, PassageKind::Abstract);
        assert!(abs.len() >= 3);
        for (i, p) in abs.iter().enumerate() {
            let w = p.text.split_whitespace().count();
            assert!(w <= 120, "{w}");
            assert!(p.text.ends_with('.'));
            assert_eq!(p.seq, i as i32);
            assert_eq!(p.pmid.as_deref(), Some(pmid.as_str()));
            assert_eq!(p.ref_ns, [n]);
            assert!(!p.comparisons.is_empty());
        }
        let all: Vec<&str> = abs.iter().flat_map(|p| p.text.split_whitespace()).collect();
        assert_eq!(all.join(" "), text);
        // 120 / 11 = 10 sentences = 110 words in the first piece.
        assert_eq!(abs[0].text.split_whitespace().count(), 110);
        // One endless sentence is cut at 120 words.
        let long = vec!["word"; 250].join(" ");
        let pieces = split_abstract(&long, 120);
        assert_eq!(
            pieces
                .iter()
                .map(|p| p.split_whitespace().count())
                .collect::<Vec<_>>(),
            [120, 120, 10]
        );
        // Abbreviations do not end a sentence.
        let s = split_abstract(
            "Robotic vs. laparoscopic repair was studied by Smith et al. in 2020. It was fast.",
            12,
        );
        assert_eq!(s.len(), 2);
        assert!(s[0].ends_with("2020."));
    }

    #[test]
    fn missing_abstract_gives_no_abstract_passage() {
        for f in FIXTURES {
            assert!(of(&built(f), PassageKind::Abstract).is_empty(), "{f}");
        }
        let (ps, _, _) = abstract_for("hernia", "   ");
        assert!(of(&ps, PassageKind::Abstract).is_empty());
        // An abstract for a paper outside the references is not used.
        let ps = build(
            &body("hernia"),
            &board("hernia"),
            &pairs("hernia"),
            &[("1".into(), "Some text.".into())],
        )
        .unwrap();
        assert!(of(&ps, PassageKind::Abstract).is_empty());
    }

    #[test]
    fn retracted_reference_is_labelled_and_its_abstract_left_out() {
        let mut b = body("hernia");
        let pmid = b["references"][0]["pmid"].as_str().unwrap().to_string();
        let other = b["references"][1]["pmid"].as_str().unwrap().to_string();
        b["references"][0]["retracted"] = json!(true);
        let abs = [
            (pmid.clone(), "Retracted claim.".to_string()),
            (other.clone(), "Kept claim.".to_string()),
        ];
        let ps = build(&b, &board("hernia"), &pairs("hernia"), &abs).unwrap();
        let r = of(&ps, PassageKind::Reference);
        assert!(r[0].text.contains("RETRACTED"));
        assert!(!r[1].text.contains("RETRACTED"));
        let a = of(&ps, PassageKind::Abstract);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].pmid.as_deref(), Some(other.as_str()));
        assert!(
            !ps.iter()
                .any(|p| p.kind == PassageKind::Abstract && p.text.contains("Retracted claim"))
        );
    }

    #[test]
    fn opinion_passages_cover_verdict_why_glance_eras_decisions() {
        for f in FIXTURES {
            let bd = board(f);
            let ps = built(f);
            let op = of(&ps, PassageKind::Opinion);
            assert!(
                op.iter().any(|p| p.text.starts_with("Expert verdict:")),
                "{f}"
            );
            assert!(
                op.iter().any(|p| p.text.starts_with("My view on why:")),
                "{f}"
            );
            let lines: usize = ["original_quote", "expert", "today"]
                .iter()
                .map(|k| {
                    bd["at_a_glance"][*k]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .count()
                })
                .sum();
            assert_eq!(
                op.iter()
                    .filter(|p| p.text.starts_with("At a glance"))
                    .count(),
                lines,
                "{f}"
            );
            assert_eq!(
                op.iter().filter(|p| p.text.starts_with("Era ")).count(),
                len(&bd["eras"]),
                "{f}"
            );
            assert_eq!(
                op.iter()
                    .filter(|p| p.text.starts_with("Decision."))
                    .count(),
                len(&bd["decisions"]),
                "{f}"
            );
            assert!(
                len(&bd["decisions"]) > 0 && len(&bd["eras"]) > 0 && lines > 0,
                "{f}"
            );
        }
        // The board may arrive wrapped.
        let wrapped = json!({"board": board("hernia")});
        let a = build(&body("hernia"), &wrapped, &pairs("hernia"), &[]).unwrap();
        assert_eq!(a, built("hernia"));
        // A bad board is a fixed code.
        let e = build(&body("hernia"), &json!({"topic": 5}), &pairs("hernia"), &[]).unwrap_err();
        assert_eq!(e.code(), "bad_board");
        assert_eq!(
            build(&json!({"version": 3}), &board("hernia"), &[], &[])
                .unwrap_err()
                .code(),
            "bad_answer"
        );
        let mut none = body("hernia");
        none["answer"] = Value::Null;
        assert_eq!(
            build(&none, &board("hernia"), &[], &[]).unwrap_err().code(),
            "no_answer"
        );
    }

    #[test]
    fn multi_arm_study_gets_both_tags() {
        let ps = built("achalasia");
        let ev = of(&ps, PassageKind::Evidence);
        let b = body("achalasia");
        let idx = b["answer"]["studies"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["comparison"] == "POEM vs LHM vs PD")
            .unwrap();
        for k in [
            comparison::key("poem", "lhm"),
            comparison::key("poem", "pd"),
        ] {
            assert!(
                ev[idx].comparisons.contains(&k),
                "{:?}",
                ev[idx].comparisons
            );
        }
        // A single-pair study carries only its pair.
        let idx = b["answer"]["studies"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["comparison"] == "POEM vs pneumatic dilation (30/35 mm)")
            .unwrap();
        assert_eq!(ev[idx].comparisons, [comparison::key("poem", "pd")]);
        // Every outcome passage's tags are the union of its studies' tags.
        let outs = of(&ps, PassageKind::Outcome);
        for (i, o) in b["answer"]["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            // Own words and number arms first; the studies' union otherwise.
            let mut own = vec![
                o["outcome"].to_string(),
                o["effect_text"].to_string(),
                o["rct_evidence"].to_string(),
                o["why"].to_string(),
            ];
            own.extend(
                o["numbers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|n| n["arm"].as_str().map(str::to_string)),
            );
            let mut want: Vec<String> = comparison::tags(&own.join(" , "), &pairs("achalasia"));
            let from_own = !want.is_empty();
            for p in o["pmids"].as_array().unwrap() {
                let s = b["answer"]["studies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["pmid"] == *p);
                if let (true, Some(s)) = (!from_own, s) {
                    for k in
                        comparison::tags(s["comparison"].as_str().unwrap(), &pairs("achalasia"))
                    {
                        if !want.contains(&k) {
                            want.push(k);
                        }
                    }
                }
            }
            if !want.is_empty() {
                assert_eq!(outs[i].comparisons, want, "outcome {i}");
            }
        }
    }

    #[test]
    fn robotic_vs_open_only_reaches_passages_about_an_open_comparison() {
        // Cholecystitis has one pair (robotic vs laparoscopic). A robotic-vs-
        // open question may reach only passages that rest on a study whose
        // comparison names open, or whose own text does.
        let f = "cholecystitis";
        let pr = pairs(f);
        let asked = comparison::asked(
            "If I have robotic instead of open, will I go home sooner?",
            &pr,
        )
        .unwrap();
        let b = body(f);
        let open_pmids: Vec<String> = b["answer"]["studies"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| {
                comparison::arms_named(s["comparison"].as_str().unwrap(), &pr)
                    .contains(&"open".to_string())
            })
            .map(|s| s["pmid"].as_str().unwrap().to_string())
            .collect();
        assert!(!open_pmids.is_empty(), "fixture has a study naming open");
        let open_ns: Vec<i32> = b["references"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| open_pmids.iter().any(|p| r["pmid"] == p.as_str()))
            .map(|r| r["n"].as_i64().unwrap() as i32)
            .collect();
        let rl = comparison::key("robotic", "laparoscopic");
        let mut reached = 0;
        for p in built(f) {
            assert!(!p.comparisons.is_empty(), "{:?} is untagged", p.kind);
            if p.comparisons.contains(&asked) {
                reached += 1;
                if p.kind == PassageKind::Outcome {
                    // An outcome row reached by robotic-vs-open names open
                    // itself (text or number arms), or names no pair at all.
                    let rows = b["answer"]["outcomes"].as_array().unwrap();
                    let row = rows.iter().find(|o| {
                        p.text
                            .contains(&format!("Outcome: {}.", o["outcome"].as_str().unwrap()))
                    });
                    if let Some(row) = row {
                        let mut own = vec![
                            row["outcome"].to_string(),
                            row["effect_text"].to_string(),
                            row["rct_evidence"].to_string(),
                            row["why"].to_string(),
                        ];
                        own.extend(
                            row["numbers"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter_map(|n| n["arm"].as_str().map(str::to_string)),
                        );
                        let own = own.join(" , ");
                        assert!(
                            comparison::arms_named(&own, &pr).contains(&"open".to_string())
                                || comparison::tags(&own, &pr).is_empty(),
                            "{}",
                            p.text
                        );
                    }
                }
                assert!(
                    p.ref_ns.iter().any(|n| open_ns.contains(n))
                        || comparison::arms_named(&p.text, &pr).contains(&"open".to_string()),
                    "{:?}: {}",
                    p.kind,
                    p.text
                );
            } else {
                assert!(
                    p.comparisons.contains(&rl),
                    "{:?}: {:?}",
                    p.kind,
                    p.comparisons
                );
            }
        }
        assert!(
            reached < built(f).len() / 2,
            "{reached} passages reach an open question"
        );
    }

    #[test]
    fn outcome_row_prefers_its_own_arms_over_study_union() {
        let mut b = body("cholecystitis");
        let pr = pairs("cholecystitis");
        let multi = b["answer"]["studies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| {
                s["comparison"]
                    .as_str()
                    .unwrap()
                    .contains("laparoscopic or open")
            })
            .expect("multi-arm study in fixture")["pmid"]
            .clone();
        let open = comparison::key("robotic", "open");
        let rl = comparison::key("robotic", "laparoscopic");
        // Row 0 names robotic and laparoscopic; row 1 names no arms.
        for (i, eff) in [
            (0, "Robotic took 5 min longer than laparoscopic."),
            (1, "Stay was shorter."),
        ] {
            let o = &mut b["answer"]["outcomes"][i];
            o["outcome"] = json!("Stay");
            o["effect_text"] = json!(eff);
            o["rct_evidence"] = json!("");
            o["why"] = json!("");
            o["numbers"] = json!([]);
            o["pmids"] = json!([multi.clone()]);
        }
        let ps = build(&b, &board("cholecystitis"), &pr, &[]).unwrap();
        let outs = of(&ps, PassageKind::Outcome);
        assert_eq!(outs[0].comparisons, std::slice::from_ref(&rl));
        assert!(!outs[0].comparisons.contains(&open));
        // No arms of its own: falls back to the study union.
        assert!(
            outs[1].comparisons.contains(&open),
            "{:?}",
            outs[1].comparisons
        );
        assert!(outs[1].comparisons.contains(&rl));
        // Number arms count as the row's own words.
        b["answer"]["outcomes"][1]["numbers"] = json!([
            {"label": "x", "value": 1.0, "unit": null, "arm": "robotic", "ci_low": null, "ci_high": null, "p": null, "pmid": multi},
            {"label": "x", "value": 2.0, "unit": null, "arm": "laparoscopic", "ci_low": null, "ci_high": null, "p": null, "pmid": multi}
        ]);
        let ps = build(&b, &board("cholecystitis"), &pr, &[]).unwrap();
        assert_eq!(of(&ps, PassageKind::Outcome)[1].comparisons, [rl]);
    }

    #[test]
    fn untagged_when_unknown() {
        // Two pairs and a text naming one arm: no tag from the record.
        let mut b = body("achalasia");
        b["answer"]["applicability"]["case_mix"] = json!("Patients were mostly adults.");
        b["answer"]["applicability"]["india_cost_context"] = json!("");
        b["answer"]["applicability"]["training"] = json!("");
        let ps = build(&b, &board("achalasia"), &pairs("achalasia"), &[]).unwrap();
        assert!(
            of(&ps, PassageKind::Applicability)[0]
                .comparisons
                .is_empty()
        );
    }

    #[test]
    fn starters_are_three_questions_from_first_outcomes() {
        for f in FIXTURES {
            let b = body(f);
            let s = starters(&b);
            assert_eq!(s.len(), 3);
            let first = b["answer"]["outcomes"][0]["outcome"].as_str().unwrap();
            assert!(
                s[0].to_lowercase().contains(&first.to_lowercase()),
                "{}",
                s[0]
            );
            assert!(s.iter().all(|q| q.ends_with('?')));
        }
        assert!(starters(&json!({})).is_empty());
        assert_eq!(lower_first("ICU stay"), "ICU stay");
        assert_eq!(lower_first("Operative time"), "operative time");
    }
}
