//! Part A items 10 and 11: the Details (one continuous page, every section
//! open, empty sections hidden) and the study sheets.
//!
//! Each reference has one study sheet: a native `<dialog>` drawn on the
//! server (closed), opened with `showModal` from any chip, tile, number
//! group, forest row or trial card after hydration. Before hydration the
//! chips are `#ref-n` links to the References.

use std::collections::HashSet;

use leptos::prelude::*;

use super::page::{Ctx, Links, Mk, chips, mark_el, rich, sheet_click, sheet_id, unsourced_label};
use super::{COPY_AREA, CopyState, Mark, copy_answer, doi_link, pubmed_link, rupees};
use crate::agent::answer::duration_text;
use crate::agent::answer_v3::{
    ADDED_LABEL, EvidenceV3, FOUND_BY_SKEPTIC, NotableFinding, ReferenceV3, count_noun,
    not_cited_at_end, whole_marked,
};
use crate::answer_page::{self as ap, CitedIn, GuidelineSource, Section};

/// One Details section: heading, count, body. `None` when it is empty.
fn section(title: &'static str, count: String, body: AnyView) -> AnyView {
    view! {
        <section class="ap-sec" aria-label=title>
            <div class="ap-sec-head">
                <h3>{title}</h3>
                <span class="ap-count">{count}</span>
            </div>
            <div class="ap-sec-body">{body}</div>
        </section>
    }
    .into_any()
}

/// A small heading inside a section.
fn sub(title: String) -> AnyView {
    view! { <div class="ap-kicker">{title}</div> }.into_any()
}

/// A quote, in the earlier page's curly quotes.
fn quote_view(q: &str) -> AnyView {
    view! { <blockquote class="ap-quote">{format!("\u{201c}{q}\u{201d}")}</blockquote> }.into_any()
}

/// A study's reference chip, or a PubMed link when it has none (PMIDs of 1
/// to 9 digits only).
fn study_ref(c: &Ctx, pmid: &str) -> AnyView {
    let p = pmid.trim();
    if c.ref_of(p).is_some() {
        view! { " " {chips(&[p.to_string()], &c.refs, Links::Yes)} }.into_any()
    } else if (1..=9).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) {
        let href = format!("https://pubmed.ncbi.nlm.nih.gov/{p}/");
        view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> }
            .into_any()
    } else {
        ().into_any()
    }
}

/// The PMIDs whose skeptic leads are drawn on a trial (as today).
fn leads_on_rows(c: &Ctx) -> HashSet<String> {
    c.a.skeptic
        .iter()
        .flat_map(|k| &k.leads)
        .filter(|l| c.a.lead_on_row(&l.pmid))
        .map(|l| l.pmid.clone())
        .collect()
}

/// The Details: a heading and the sections in [`ap::SECTION_ORDER`].
pub(super) fn details_view(c: &Ctx) -> AnyView {
    let on_rows = leads_on_rows(c);
    let sections: Vec<AnyView> = ap::SECTION_ORDER
        .iter()
        .filter_map(|s| match s {
            Section::Conflicts => conflicts(c),
            Section::Guidelines => guidelines(c, &on_rows),
            Section::Applicability => applicability(c),
            Section::WouldChange => would_change(c),
            Section::Skeptic => skeptic(c, &on_rows),
            Section::Trials => trials(c),
            Section::Notable => notable(c),
            Section::Completeness => completeness(c),
            Section::Checks => checks(c),
            Section::References => Some(references(c)),
            Section::SearchTrail => search_trail(c),
            Section::Cost => Some(cost(c)),
        })
        .collect();
    view! {
        <div class="ap-details">
            <h3 class="ap-details-title">"Details"</h3>
            {sections}
        </div>
    }
    .into_any()
}

fn conflicts(c: &Ctx) -> Option<AnyView> {
    let w = c.w?;
    let added =
        c.a.completeness
            .as_ref()
            .map_or(&[][..], |x| x.added_conflicts.as_slice());
    let n = w.conflicts.len() + added.len();
    if n == 0 {
        return None;
    }
    let items = w
        .conflicts
        .iter()
        .enumerate()
        .map(|(i, x)| {
            view! { <li>{c.t(&x.text, &format!("conflicts.{i}.text"), c.wm())}</li> }.into_any()
        })
        .chain(added.iter().enumerate().map(|(k, x)| {
            view! {
                <li>
                    {c.t(&x.text, &format!("added_conflicts.{k}.text"), c.cm())}
                    {super::page::ref_chips(&not_cited_at_end(&x.text, &x.pmids), &c.refs)}
                    {c.added_label()}
                    {unsourced_label(x.unsourced)}
                </li>
            }
            .into_any()
        }))
        .collect::<Vec<_>>();
    Some(section(
        "Conflicting evidence",
        n.to_string(),
        view! { <ul>{items}</ul> }.into_any(),
    ))
}

fn guidelines(c: &Ctx, on_rows: &HashSet<String>) -> Option<AnyView> {
    let entries = ap::guideline_entries(c.a, on_rows);
    if entries.is_empty() {
        return None;
    }
    let n = entries.len();
    let mut body: Vec<AnyView> = Vec::new();
    let mut current: Option<String> = None;
    let mut items: Vec<AnyView> = Vec::new();
    let flush = |body: &mut Vec<AnyView>, items: &mut Vec<AnyView>| {
        if !items.is_empty() {
            body.push(view! { <ul>{std::mem::take(items)}</ul> }.into_any());
        }
    };
    for e in entries {
        if current.as_deref() != Some(e.body.as_str()) {
            flush(&mut body, &mut items);
            // The heading: a writer's body with its marks, else as named.
            let head = match e.source {
                GuidelineSource::Writer(i) => c.w.map_or_else(
                    || e.body.clone().into_any(),
                    |w| {
                        c.t(
                            &w.guidelines[i].body,
                            &format!("guidelines.{i}.body"),
                            c.wm(),
                        )
                    },
                ),
                _ => e.body.clone().into_any(),
            };
            body.push(view! { <div class="ap-kicker">{head}</div> }.into_any());
            current = Some(e.body.clone());
        }
        let item = match e.source {
            GuidelineSource::Writer(i) => {
                let Some(g) = c.w.map(|w| &w.guidelines[i]) else {
                    continue;
                };
                let n = g.pmid.as_ref().and_then(|p| c.ref_of(p));
                view! {
                    <li>
                        {g.year.map(|y| format!("({y}) "))}
                        {c.t(&g.statement, &format!("guidelines.{i}.statement"), c.wm())}
                        {n.map(|_| view! { " " {chips(&[g.pmid.clone().unwrap_or_default()], &c.refs, Links::Yes)} })}
                        {unsourced_label(g.pmid.is_none())}
                    </li>
                }
                .into_any()
            }
            GuidelineSource::Skeptic(i) => {
                let Some(g) = c.a.skeptic.as_ref().map(|k| &k.guidelines[i]) else {
                    continue;
                };
                view! {
                    <li>
                        {c.t(&g.text, &format!("guidelines.{i}.text"), c.sm())}
                        {g.quote.as_deref().map(quote_view)}
                        {study_ref(c, &g.pmid)}
                        {unsourced_label(g.pmid.trim().is_empty())}
                    </li>
                }
                .into_any()
            }
            GuidelineSource::Lead(i) => {
                let Some(l) = c.a.skeptic.as_ref().map(|k| &k.leads[i]) else {
                    continue;
                };
                view! { <li><span class="flag-label">{l.text()}</span> {study_ref(c, &l.pmid)}</li> }
                    .into_any()
            }
        };
        items.push(item);
    }
    flush(&mut body, &mut items);
    Some(section(
        "Guidelines and consensus statements",
        n.to_string(),
        body.into_any(),
    ))
}

fn applicability(c: &Ctx) -> Option<AnyView> {
    let a = &c.w?.applicability;
    let parts: Vec<AnyView> = [
        ("Case mix", &a.case_mix, "applicability.case_mix"),
        (
            "India and cost",
            &a.india_cost_context,
            "applicability.india_cost_context",
        ),
        ("Training", &a.training, "applicability.training"),
    ]
    .into_iter()
    .filter(|(_, t, _)| !t.trim().is_empty())
    .map(|(label, t, f)| view! { <dt>{label}</dt><dd>{c.t(t, f, c.wm())}</dd> }.into_any())
    .collect();
    if parts.is_empty() {
        return None;
    }
    let n = parts.len();
    Some(section(
        "Applicability",
        n.to_string(),
        view! { <dl class="ap-dl">{parts}</dl> }.into_any(),
    ))
}

fn would_change(c: &Ctx) -> Option<AnyView> {
    let w = c.w?;
    if w.would_change.is_empty() {
        return None;
    }
    let items: Vec<AnyView> = w
        .would_change
        .iter()
        .enumerate()
        .map(|(i, x)| view! { <li>{c.t(x, &format!("would_change.{i}"), c.wm())}</li> }.into_any())
        .collect();
    Some(section(
        "What would change this answer",
        w.would_change.len().to_string(),
        view! { <ul>{items}</ul> }.into_any(),
    ))
}

/// The skeptic's review, re-laid out (its guidelines are under Guidelines).
fn skeptic(c: &Ctx, on_rows: &HashSet<String>) -> Option<AnyView> {
    let k = c.a.skeptic.as_ref()?;
    let sm = c.sm();
    let moved = ap::against_moved(c.a);
    // (field, text, quote, studies)
    type Line = (String, String, Option<String>, Vec<String>);
    let item = |path: String, i: &crate::agent::answer_v3::SkepticItem| -> Line {
        (path, i.text.clone(), i.quote.clone(), vec![i.pmid.clone()])
    };
    let groups: Vec<(&'static str, Vec<Line>)> = vec![
        (
            "Caveats",
            k.must_mention
                .iter()
                .enumerate()
                .map(|(i, m)| (format!("must_mention.{i}"), m.clone(), None, vec![]))
                .collect(),
        ),
        (
            "Not independent",
            k.overlaps
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    (
                        format!("overlaps.{i}.note"),
                        o.note.clone(),
                        None,
                        o.pmids.clone(),
                    )
                })
                .collect(),
        ),
        (
            "Trials the search missed",
            k.missed_rcts
                .iter()
                .enumerate()
                .map(|(i, x)| item(format!("missed_rcts.{i}.text"), x))
                .collect(),
        ),
        (
            "Picked for extraction (second wave)",
            k.extract_next
                .iter()
                .enumerate()
                .map(|(i, x)| item(format!("extract_next.{i}.text"), x))
                .collect(),
        ),
        (
            "What the meta-analyses pooled",
            k.ma_notes
                .iter()
                .enumerate()
                .map(|(i, x)| item(format!("ma_notes.{i}.text"), x))
                .collect(),
        ),
        (
            "Evidence against",
            k.contradicting
                .iter()
                .enumerate()
                .filter(|(i, _)| !moved.get(*i).copied().unwrap_or(false))
                .map(|(i, x)| item(format!("contradicting.{i}.text"), x))
                .collect(),
        ),
        (
            "Unverified leads",
            k.leads
                .iter()
                .filter(|l| !on_rows.contains(&l.pmid))
                .map(|l| (String::new(), l.text(), None, vec![l.pmid.clone()]))
                .collect(),
        ),
    ];
    let body: Vec<AnyView> = std::iter::once(
        view! { <p class="ap-verdict-line">{format!("Verdict: the conclusion {}.", k.verdict)}</p> }
            .into_any(),
    )
    .chain(groups.into_iter().filter(|(_, xs)| !xs.is_empty()).map(|(title, xs)| {
        let items: Vec<AnyView> = xs
            .into_iter()
            .map(|(path, t, q, ps)| {
                let text = if path.is_empty() {
                    view! { <span class="flag-label">{t}</span> }.into_any()
                } else {
                    rich(&t, &path, sm, &c.refs, Links::Yes)
                };
                view! {
                    <li>
                        {text}
                        {q.as_deref().map(quote_view)}
                        {ps.iter().map(|p| study_ref(c, p)).collect_view()}
                    </li>
                }
                .into_any()
            })
            .collect();
        view! { {sub(title.to_string())} <ul>{items}</ul> }.into_any()
    }))
    .collect();
    let n = u32::try_from(k.must_mention.len()).unwrap_or(u32::MAX);
    Some(section(
        "Challenged by the skeptic",
        count_noun(n, "caveat", "caveats"),
        body.into_any(),
    ))
}

/// The evidence badges of a study (cards and sheets).
fn badges(c: &Ctx, pmid: &str, e: Option<&EvidenceV3>, sheet: bool) -> Vec<AnyView> {
    let mut v: Vec<AnyView> = Vec::new();
    let tag = |t: String, class: &'static str| view! { <span class=class>{t}</span> }.into_any();
    if let Some(e) = e {
        let read = if e.text == "not extracted" {
            "not extracted (abstract)".to_string()
        } else {
            e.text.clone()
        };
        v.push(tag(
            read,
            if e.text == "full text" {
                "ap-tag blue"
            } else {
                "ap-tag"
            },
        ));
        v.push(tag(
            count_noun(e.verified_quotes, "verified quote", "verified quotes"),
            "ap-tag",
        ));
        if e.numbers_unverified > 0 {
            v.push(tag(
                format!(
                    "{} not verified",
                    count_noun(e.numbers_unverified, "number", "numbers")
                ),
                "ap-tag amber",
            ));
        }
        if e.found_by_skeptic {
            v.push(tag(
                format!("{FOUND_BY_SKEPTIC}; {}", e.second_wave()),
                "ap-tag purple",
            ));
        }
    }
    if !c.a.leads_on_row(pmid).is_empty() && !sheet {
        v.push(tag("skeptic lead".to_string(), "ap-tag purple"));
    }
    if sheet {
        v.push(tag(
            ap::src_type(
                c.w.and_then(|w| ap::study_of(w, pmid))
                    .map_or("", |s| s.design.as_str()),
            )
            .label()
            .to_string(),
            "ap-tag",
        ));
    }
    v
}

/// The trials: one card per study, opening its sheet.
fn trials(c: &Ctx) -> Option<AnyView> {
    let w = c.w?;
    if w.studies.is_empty() {
        return None;
    }
    let cards: Vec<AnyView> = w
        .studies
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let n = c.ref_of(&s.pmid);
            let e = c.a.evidence.iter().find(|e| e.pmid == s.pmid.trim());
            let head = c.t_plain(&s.label, &format!("studies.{i}.label"), c.wm());
            let head = match n {
                Some(r) => view! { <a href=format!("#ref-{r}") rel="external" on:click=sheet_click(r)>{head}</a> }.into_any(),
                None => head,
            };
            let line = view! {
                {c.t(&s.design, &format!("studies.{i}.design"), c.wm())}
                {s.n.map(|n| format!(" \u{b7} n = {}", ap::fmt_value(n as f64)))}
                {whole_marked(&format!("studies.{i}.n"), &c.a.untraced).then(|| mark_el(Mark::NotTraced, format!("w:studies.{i}.n")))}
                " \u{b7} "
                {c.t(&s.comparison, &format!("studies.{i}.comparison"), c.wm())}
            };
            let tags = badges(c, s.pmid.trim(), e, false);
            let body = view! {
                <div class="ap-trial-head">
                    <span class="ap-trial-label">{head}</span>
                    {n.map(|n| view! { <span class="ap-ref-n">{format!("[{n}]")}</span> })}
                </div>
                <div class="ap-sub">{line}</div>
                {(!tags.is_empty()).then(|| view! { <div class="ap-tags">{tags}</div> })}
            };
            match n {
                Some(r) => view! { <div class="ap-trial tappable" on:click=sheet_click(r)>{body}</div> }.into_any(),
                None => view! { <div class="ap-trial">{body}</div> }.into_any(),
            }
        })
        .collect();
    let n = cards.len();
    Some(section(
        "The trials",
        n.to_string(),
        view! {
            <p class="ap-muted">"Tap a study for its appraisal, funding and PubMed link."</p>
            {cards}
        }
        .into_any(),
    ))
}

fn notable(c: &Ctx) -> Option<AnyView> {
    let writer: &[NotableFinding] = c.w.map_or(&[], |w| w.notable_findings.as_slice());
    // Stage 4b item 5: the skeptic's repeats stay in its section only.
    let code: Vec<(usize, &NotableFinding)> =
        c.a.notable_code
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.repeats_skeptic())
            .collect();
    let n = code.len() + writer.len();
    if n == 0 {
        return None;
    }
    let items: Vec<AnyView> = code
        .into_iter()
        .map(|(k, x)| {
            let ns = format!("n{k}:");
            let mk = Mk {
                spans: &x.spans,
                mark: Mark::NotVerified,
                ns: &ns,
                suppress: false,
            };
            view! {
                <li>
                    {rich(&x.text, "text", mk, &c.refs, Links::Yes)}
                    {super::page::ref_chips(&not_cited_at_end(&x.text, &x.pmids), &c.refs)}
                </li>
            }
            .into_any()
        })
        .chain(writer.iter().enumerate().map(|(i, x)| {
            view! {
                <li>
                    {c.t(&x.text, &format!("notable_findings.{i}.text"), c.wm())}
                    {super::page::ref_chips(&not_cited_at_end(&x.text, &x.pmids), &c.refs)}
                </li>
            }
            .into_any()
        }))
        .collect();
    Some(section(
        "Other notable findings from this run",
        n.to_string(),
        view! { <ul>{items}</ul> }.into_any(),
    ))
}

fn completeness(c: &Ctx) -> Option<AnyView> {
    let x = c.a.completeness.as_ref()?;
    let notes: Vec<AnyView> = x
        .notes
        .iter()
        .enumerate()
        .map(|(k, n)| {
            view! {
                <li>
                    {c.t(n, &format!("notes.{k}"), c.cm())}
                    " " <span class="flag-label added">{ADDED_LABEL}</span>
                </li>
            }
            .into_any()
        })
        .collect();
    let not_added: Vec<AnyView> = x
        .not_added
        .iter()
        .map(|n| {
            let none = Mk {
                spans: &[],
                mark: Mark::NotTraced,
                ns: "c:",
                suppress: false,
            };
            view! {
                <li>
                    {rich(&n.gap, "", none, &c.refs, Links::PubMed)}
                    " \u{2014} "
                    {rich(&n.reason, "", none, &c.refs, Links::PubMed)}
                </li>
            }
            .into_any()
        })
        .collect();
    let n_not = not_added.len();
    let count = x.notes.len() + n_not + x.added();
    Some(section(
        "Completeness review",
        count.to_string(),
        view! {
            <p class="ap-muted">{x.line()}</p>
            {(!notes.is_empty()).then(|| view! { {sub("Notes".to_string())} <ul>{notes}</ul> })}
            {(n_not > 0).then(|| view! {
                {sub(format!("Judged not relevant ({n_not})"))}
                <ul>{not_added}</ul>
            })}
        }
        .into_any(),
    ))
}

/// The integrity lines and notes (and how many registration-ID marks were
/// not drawn).
fn checks(c: &Ctx) -> Option<AnyView> {
    let mut lines: Vec<String> = c.a.notes.clone();
    lines.extend(c.a.trail_notes());
    let hidden = ap::suppressed_registration_marks(c.a);
    if hidden > 0 {
        lines.push(format!(
            "{} on a trial or review registration ID not drawn on this page (the earlier layout below shows {}).",
            count_noun(
                u32::try_from(hidden).unwrap_or(u32::MAX),
                "\u{201c}not traced\u{201d} mark",
                "\u{201c}not traced\u{201d} marks"
            ),
            if hidden == 1 { "it" } else { "them" }
        ));
    }
    if lines.is_empty() {
        return None;
    }
    let n = lines.len();
    Some(section(
        "Checks",
        n.to_string(),
        view! { <ul>{lines.into_iter().map(|l| view! { <li>{l}</li> }).collect_view()}</ul> }
            .into_any(),
    ))
}

fn reference_item(r: &ReferenceV3) -> AnyView {
    let mut tail = String::new();
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => tail.push_str(&format!("{j}, {y}.")),
        (Some(j), None) => tail.push_str(&format!("{j}.")),
        (None, Some(y)) => tail.push_str(&format!("{y}.")),
        (None, None) => {}
    }
    let pmid = pubmed_link(&r.pmid).map(|href| (r.pmid.clone(), href));
    let doi = r
        .doi
        .clone()
        .and_then(|d| doi_link(&d).map(|href| (d, href)));
    let n = r.n;
    let mut head = r.title.trim_end_matches('.').to_string();
    head.push('.');
    view! {
        <li id=format!("ref-{n}")>
            <span class="ap-ref-n">{format!("[{n}] ")}</span>
            {r.retracted.then(|| view! { <span class="retracted">"RETRACTED \u{2014} do not rely on this paper. "</span> })}
            <a class="ap-ref-title" href=format!("#ref-{n}") rel="external" on:click=sheet_click(n)>{head}</a>
            {(!tail.is_empty()).then(|| format!(" {tail}"))}
            {pmid.map(|(p, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> })}
            {doi.map(|(d, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("DOI {d}")}</a> })}
            {r.found_by_skeptic.then(|| view! { " " <span class="flag-label added">{FOUND_BY_SKEPTIC}</span> })}
        </li>
    }
    .into_any()
}

fn references(c: &Ctx) -> AnyView {
    let refs = &c.a.references;
    let body = if refs.is_empty() {
        view! { <p>"None."</p> }.into_any()
    } else {
        view! { <ul class="ap-refs">{refs.iter().map(reference_item).collect_view()}</ul> }
            .into_any()
    };
    section("References", refs.len().to_string(), body)
}

fn search_trail(c: &Ctx) -> Option<AnyView> {
    let s = &c.a.searches;
    if s.is_empty() {
        return None;
    }
    let n = u32::try_from(s.len()).unwrap_or(u32::MAX);
    Some(section(
        "Search trail",
        count_noun(n, "search", "searches"),
        view! {
            <ul>
                {s.iter()
                    .map(|x| view! { <li>{format!("{}: ", x.source)} <code>{x.query.clone()}</code> {format!(" \u{2014} {} hits", x.hits)}</li> })
                    .collect_view()}
            </ul>
        }
        .into_any(),
    ))
}

/// Cost and time, and Copy as text (Part A: p printed once, item 12).
fn cost(c: &Ctx) -> AnyView {
    let a = c.a;
    let text = a.copy_text();
    let copy = RwSignal::new(CopyState::Ready);
    let line = format!(
        "{} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    );
    section(
        "Cost and time",
        rupees(a.cost_paise),
        view! {
            <p>{line}</p>
            <button class="secondary small" on:click=move |_| copy_answer(&text, copy)>
                {move || match copy.get() {
                    CopyState::Ready => "Copy as text",
                    CopyState::Copied => "Copied",
                    CopyState::Manual => "Select the text below and copy it",
                }}
            </button>
            // Empty in the markup; the text is set as its value on click.
            <textarea
                id=COPY_AREA
                class=move || if copy.get() == CopyState::Manual { "copy-area shown" } else { "copy-area" }
                readonly=true
                aria-hidden=move || (copy.get() != CopyState::Manual).to_string()
                tabindex=move || if copy.get() == CopyState::Manual { "0" } else { "-1" }
            ></textarea>
        }
        .into_any(),
    )
}

// ---------- 11. the study sheets ----------

/// One study sheet per reference (closed `<dialog>`s).
pub(super) fn sheets_view(c: &Ctx) -> AnyView {
    c.a.references
        .iter()
        .map(|r| sheet(c, r))
        .collect_view()
        .into_any()
}

fn sheet(c: &Ctx, r: &ReferenceV3) -> AnyView {
    let n = r.n;
    let pmid = r.pmid.trim();
    let w = c.w;
    let si = w.and_then(|w| w.studies.iter().position(|s| s.pmid.trim() == pmid));
    let study = si.and_then(|i| w.map(|w| (i, &w.studies[i])));
    let e = c.a.evidence.iter().find(|e| e.pmid == pmid);
    let head = match study {
        Some((i, s)) => c.t_plain(&s.label, &format!("studies.{i}.label"), c.wm()),
        None => {
            let t = match (&r.journal, r.year) {
                (Some(j), Some(y)) => format!("{j} {y}"),
                (Some(j), None) => j.clone(),
                (None, Some(y)) => y.to_string(),
                (None, None) => format!("PMID {pmid}"),
            };
            t.into_any()
        }
    };
    let journal = match (&r.journal, r.year) {
        (Some(j), Some(y)) => Some(format!("{j}, {y}")),
        (Some(j), None) => Some(j.clone()),
        (None, Some(y)) => Some(y.to_string()),
        (None, None) => None,
    };
    let row = |k: &'static str, v: AnyView| {
        view! { <div class="ap-sheet-row"><div class="ap-kicker">{k}</div><div>{v}</div></div> }
            .into_any()
    };
    let mut rows: Vec<AnyView> = Vec::new();
    if let Some((i, s)) = study {
        rows.push(row(
            "Design",
            view! {
                {c.t(&s.design, &format!("studies.{i}.design"), c.wm())}
                {s.n.map(|n| format!(" \u{b7} n = {}", ap::fmt_value(n as f64)))}
                {whole_marked(&format!("studies.{i}.n"), &c.a.untraced).then(|| mark_el(Mark::NotTraced, format!("w:studies.{i}.n")))}
                {(!s.setting.trim().is_empty()).then(|| view! { " \u{b7} " {c.t(&s.setting, &format!("studies.{i}.setting"), c.wm())} })}
            }
            .into_any(),
        ));
        rows.push(row(
            "Comparison",
            c.t(&s.comparison, &format!("studies.{i}.comparison"), c.wm()),
        ));
        rows.push(row(
            "Key results",
            c.t(&s.key_results, &format!("studies.{i}.key_results"), c.wm()),
        ));
        rows.push(row(
            "Appraisal",
            c.t(&s.appraisal, &format!("studies.{i}.appraisal"), c.wm()),
        ));
    } else {
        let guideline = w
            .is_some_and(|w| w.guidelines.iter().any(|g| g.pmid.as_deref() == Some(pmid)))
            || c.a
                .skeptic
                .as_ref()
                .is_some_and(|k| k.guidelines.iter().any(|g| g.pmid == pmid));
        rows.push(row(
            "Role",
            if guideline {
                "Guideline or consensus statement."
            } else {
                "Cited reference; no trial row stored for it."
            }
            .into_any(),
        ));
    }
    // Funding, only when it was looked for (Stage 4 item 3).
    if let Some(e) = e.filter(|e| e.funding_checked) {
        let ns = format!("e:{pmid}:");
        let v = match &e.funding {
            Some(f) => rich(
                f,
                "funding",
                Mk {
                    spans: &e.spans,
                    mark: Mark::NotVerified,
                    ns: &ns,
                    suppress: false,
                },
                &c.refs,
                Links::Yes,
            ),
            None => format!(
                "Not stated in the {}",
                if e.text == "full text" {
                    "text"
                } else {
                    "abstract"
                }
            )
            .into_any(),
        };
        rows.push(row("Funding", v));
    }
    if let Some(reg) = e.and_then(|e| e.trial_registration_unverified.clone()) {
        rows.push(row(
            "Registration",
            view! { {reg} " " <span class="flag-label">"named in the record; the source text does not repeat it"</span> }
                .into_any(),
        ));
    }
    let tags = badges(c, pmid, e, true);
    // A neutral box for the skeptic's unverified leads on this study.
    let leads: Vec<String> =
        c.a.skeptic
            .iter()
            .flat_map(|k| &k.leads)
            .filter(|l| l.pmid == pmid)
            .map(|l| l.text())
            .collect();
    let unverified: Vec<String> = e
        .map(|e| {
            e.unverified_details
                .iter()
                .map(|u| format!("{}: {}", u.outcome, u.effect_text))
                .collect()
        })
        .unwrap_or_default();
    let cited: Vec<AnyView> = ap::cited_in(c.a, pmid)
        .into_iter()
        .filter_map(|x| {
            let (id, name) = match x {
                CitedIn::Outcome(i) => (format!("outcome-{i}"), w?.outcomes[i].outcome.clone()),
                CitedIn::Added(k) => (
                    format!("outcome-added-{k}"),
                    c.a.completeness.as_ref()?.added_outcomes[k].outcome.clone(),
                ),
            };
            let target = id.clone();
            Some(
                view! {
                    <a class="ap-pill small" href=format!("#{id}") rel="external" on:click=move |_| go_to_outcome(n, &target)>{name}</a>
                }
                .into_any(),
            )
        })
        .collect();
    let pubmed = pubmed_link(pmid);
    let doi = r.doi.as_deref().and_then(doi_link);
    let hid = format!("{}-h", sheet_id(n));
    let labelled_by = hid.clone();
    view! {
        <dialog class="ap-sheet" id=sheet_id(n) aria-labelledby=labelled_by>
            <div class="ap-sheet-top">
                <div>
                    <div class="ap-mono ap-sheet-ref">{format!("Reference [{n}] \u{b7} PMID {pmid}")}</div>
                    <h3 id=hid>{head}</h3>
                </div>
                <form method="dialog">
                    <button class="ap-close" aria-label="Close">"\u{2715}"</button>
                </form>
            </div>
            {r.retracted.then(|| view! { <p class="retracted">"RETRACTED \u{2014} do not rely on this paper."</p> })}
            <p class="ap-sheet-title">{r.title.clone()}</p>
            {journal.map(|j| view! { <p class="ap-muted">{j}</p> })}
            {(!tags.is_empty()).then(|| view! { <div class="ap-tags">{tags}</div> })}
            {rows}
            {(!leads.is_empty()).then(|| view! {
                <div class="ap-box">
                    <strong>"Skeptic lead (unverified): "</strong>
                    {leads.join("; ")}
                </div>
            })}
            {(!unverified.is_empty()).then(|| view! {
                <div class="ap-sheet-row">
                    <div class="ap-kicker amber">"Not verified against the paper's wording"</div>
                    <ul class="unverified">
                        {unverified
                            .into_iter()
                            .map(|u| view! { <li>{u} " " <span class="flag-label">"not verified against the paper's wording"</span></li> })
                            .collect_view()}
                    </ul>
                </div>
            })}
            {(!cited.is_empty()).then(|| view! {
                <div class="ap-sheet-row">
                    <div class="ap-kicker">"Cited in"</div>
                    <div class="ap-pills wrap">{cited}</div>
                </div>
            })}
            <div class="ap-sheet-links">
                {pubmed.map(|href| view! { <a class="ap-btn dark" href=href rel="noopener noreferrer" target="_blank">"Open in PubMed"</a> })}
                {doi.map(|href| view! { <a class="ap-btn" href=href rel="noopener noreferrer" target="_blank">"DOI"</a> })}
            </div>
        </dialog>
    }
    .into_any()
}

/// "Cited in": close sheet `n` and open the outcome `id` (browser only;
/// the link then scrolls to it).
fn go_to_outcome(n: u16, id: &str) {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use web_sys::js_sys::{Function, Reflect};
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Some(d) = doc.get_element_by_id(&sheet_id(n))
            && let Some(f) = Reflect::get(&d, &"close".into())
                .ok()
                .and_then(|f| f.dyn_into::<Function>().ok())
        {
            let _ = f.call0(&d);
        }
        if let Some(o) = doc.get_element_by_id(id) {
            let _ = o.set_attribute("open", "");
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (n, id);
    }
}
