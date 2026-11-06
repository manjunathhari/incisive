//! Stage S1: reading a half-written reply (no Leptos, like
//! [`crate::answer_page`]; it runs in the browser too).
//!
//! [`read_prefix`] turns any part of a writer's reply into whatever is complete
//! so far, and notes where it was cut off. [`read_answer`] and [`read_opinion`]
//! lay that out as the page's sections, in the page's own order whatever order
//! they arrive in.
//!
//! This is a preview under a banner: none of the final page's checks have run,
//! so no marks or labels are drawn. Citations show as `[n]`: an answer's PMIDs
//! are numbered as they first appear in the reply (the references are built
//! after the writer, so these numbers can differ from the final page's); an
//! opinion's already are reference numbers. Nothing here changes stored text.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::agent::answer_v3::{Segment, split_citations};
use crate::runs::RunView;

/// The banner over a draft.
pub const BANNER: &str = "Being written: checks not yet run";
/// A section that has not arrived yet.
pub const COMING: &str = "Coming";
/// How many of the last words of an unfinished text are faded.
const FADED_WORDS: usize = 3;
/// Deeper than any reply can be.
const MAX_DEPTH: usize = 32;

// ---------- any part of a reply ----------

/// One step of a path into a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// What a part of a reply holds so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Partial {
    /// Everything complete so far, and the unfinished text. `Null` before the
    /// first `{`.
    pub value: Value,
    /// Where it was cut off: the path to the deepest value still open. `None`
    /// when the reply is whole.
    pub open: Option<Vec<Seg>>,
}

impl Partial {
    /// The value at `path` is still being written.
    fn is_open(&self, path: &[Seg]) -> bool {
        self.open.as_ref().is_some_and(|o| o.starts_with(path))
    }
}

/// Read any prefix of a reply. Never panics: an unfinished number, word or
/// escape is left out until it is whole, and text that cannot be part of JSON
/// ends the reading there.
pub fn read_prefix(text: &str) -> Partial {
    let Some(start) = text.find('{') else {
        return Partial {
            value: Value::Null,
            open: Some(Vec::new()),
        };
    };
    let mut r = Reader {
        b: text.as_bytes(),
        i: start,
        path: Vec::new(),
        open: None,
    };
    let value = r.value(0).unwrap_or(Value::Null);
    Partial {
        value,
        open: r.open,
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
    path: Vec<Seg>,
    open: Option<Vec<Seg>>,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    /// Note that the value at the current path is unfinished (the deepest wins:
    /// it is noted first, as the reading unwinds).
    fn cut(&mut self) {
        if self.open.is_none() {
            self.open = Some(self.path.clone());
        }
    }

    /// The next value; `None` when none of it can be used yet. Once anything
    /// is cut off, `open` is set and every caller stops reading.
    fn value(&mut self, depth: usize) -> Option<Value> {
        self.ws();
        let Some(&c) = self.b.get(self.i) else {
            self.cut();
            return None;
        };
        if depth > MAX_DEPTH {
            self.cut();
            return None;
        }
        match c {
            b'{' => Some(self.object(depth)),
            b'[' => Some(self.array(depth)),
            b'"' => {
                let (s, done) = self.string();
                if !done {
                    self.cut();
                }
                Some(Value::String(s))
            }
            b't' => self.word("true", Value::Bool(true)),
            b'f' => self.word("false", Value::Bool(false)),
            b'n' => self.word("null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => {
                self.cut();
                None
            }
        }
    }

    fn object(&mut self, depth: usize) -> Value {
        self.i += 1;
        let mut map = Map::new();
        loop {
            self.ws();
            match self.b.get(self.i) {
                Some(b'}') => {
                    self.i += 1;
                    return Value::Object(map);
                }
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                Some(b'"') => {}
                _ => {
                    self.cut();
                    return Value::Object(map);
                }
            }
            let (key, done) = self.string();
            if !done {
                self.cut();
                return Value::Object(map);
            }
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                self.cut();
                return Value::Object(map);
            }
            self.i += 1;
            self.path.push(Seg::Key(key.clone()));
            let v = self.value(depth + 1);
            self.path.pop();
            if let Some(v) = v {
                map.insert(key, v);
            }
            if self.open.is_some() {
                return Value::Object(map);
            }
        }
    }

    fn array(&mut self, depth: usize) -> Value {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.b.get(self.i) {
                Some(b']') => {
                    self.i += 1;
                    return Value::Array(items);
                }
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                None => {
                    self.cut();
                    return Value::Array(items);
                }
                _ => {}
            }
            self.path.push(Seg::Index(items.len()));
            let v = self.value(depth + 1);
            self.path.pop();
            if let Some(v) = v {
                items.push(v);
            }
            if self.open.is_some() {
                return Value::Array(items);
            }
        }
    }

    /// The string at the cursor, and whether its closing quote was reached. A
    /// cut-off escape is left out.
    fn string(&mut self) -> (String, bool) {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return (String::from_utf8_lossy(&out).into_owned(), false);
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return (String::from_utf8_lossy(&out).into_owned(), true);
                }
                b'\\' => match self.escape() {
                    Some(ch) => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    None => {
                        self.i = self.b.len();
                        return (String::from_utf8_lossy(&out).into_owned(), false);
                    }
                },
                _ => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// The escape at the cursor. `None`: cut off.
    fn escape(&mut self) -> Option<char> {
        let c = *self.b.get(self.i + 1)?;
        self.i += 2;
        Some(match c {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'u' => {
                let first = self.hex4()?;
                if (0xD800..0xDC00).contains(&first) {
                    // A surrogate pair: both halves, or nothing yet.
                    if self.b.get(self.i) != Some(&b'\\') || self.b.get(self.i + 1) != Some(&b'u') {
                        if self.i >= self.b.len().saturating_sub(1) {
                            return None;
                        }
                        return Some('\u{fffd}');
                    }
                    self.i += 2;
                    let second = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&second) {
                        return Some('\u{fffd}');
                    }
                    let v = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                    char::from_u32(v).unwrap_or('\u{fffd}')
                } else {
                    char::from_u32(first).unwrap_or('\u{fffd}')
                }
            }
            other => char::from(other),
        })
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        let v = std::str::from_utf8(h)
            .ok()
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(0xFFFD);
        self.i += 4;
        Some(v)
    }

    fn word(&mut self, word: &str, value: Value) -> Option<Value> {
        let end = self.i + word.len();
        if self.b.get(self.i..end) == Some(word.as_bytes()) {
            self.i = end;
            Some(value)
        } else {
            self.cut();
            None
        }
    }

    /// A number is whole only once something follows it.
    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        while self
            .b
            .get(self.i)
            .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.i += 1;
        }
        if self.i >= self.b.len() {
            self.cut();
            return None;
        }
        let v = std::str::from_utf8(&self.b[start..self.i])
            .ok()
            .and_then(|t| serde_json::from_str(t).ok());
        if v.is_none() {
            self.cut();
        }
        v
    }
}

// ---------- the page's sections ----------

/// A piece of drawn text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// `[1, 2]`.
    Cite(String),
}

/// A text, with its last words apart while it is still being written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DraftText {
    pub head: Vec<Piece>,
    /// Drawn faded.
    pub tail: Vec<Piece>,
}

/// How an item is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A paragraph, with its label in bold in front.
    Para,
    /// A list line.
    Bullet,
    /// A named row (an outcome, a trial, a decision), its name above it.
    Entry,
}

/// One thing in a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub label: Option<String>,
    pub text: DraftText,
    /// An unfinished last item: drawn faded whole.
    pub faded: bool,
}

/// One section of the page. No items: not arrived yet ([`COMING`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    /// "2 written so far", on a list still being written.
    pub note: Option<String>,
    pub items: Vec<Item>,
}

/// How a reply's citation markers read.
enum Cites {
    /// PMIDs, numbered as they first appear.
    Numbered(HashMap<String, usize>),
    /// Reference numbers already.
    AsWritten,
}

impl Cites {
    fn numbered(text: &str) -> Cites {
        let mut map = HashMap::new();
        for s in split_citations(text, |_| false) {
            if let Segment::Cite(ids) = s {
                for id in ids {
                    let next = map.len() + 1;
                    map.entry(id).or_insert(next);
                }
            }
        }
        Cites::Numbered(map)
    }

    fn pieces(&self, text: &str) -> Vec<Piece> {
        let known = matches!(self, Cites::AsWritten);
        split_citations(text, |_| known)
            .into_iter()
            .map(|s| match s {
                Segment::Text(t) => Piece::Text(t),
                Segment::Cite(ids) => {
                    let shown: Vec<String> = ids
                        .iter()
                        .map(|id| match self {
                            Cites::Numbered(m) => {
                                m.get(id).map_or_else(|| id.clone(), usize::to_string)
                            }
                            Cites::AsWritten => id.clone(),
                        })
                        .collect();
                    Piece::Cite(format!("[{}]", shown.join(", ")))
                }
            })
            .collect()
    }
}

/// `t` without a citation marker cut off at its end (`… [4292`).
fn without_cut_marker(t: &str) -> &str {
    let Some(i) = t.rfind('[') else { return t };
    let rest = &t[i + 1..];
    let marker_so_far = !rest.contains(']')
        && rest.chars().all(|c| {
            c.is_ascii_digit() || matches!(c, ';' | ',' | ' ' | ':' | 'P' | 'M' | 'I' | 'D' | 's')
        });
    if marker_so_far { t[..i].trim_end() } else { t }
}

/// `t` drawn: whole, or, while `open`, with its last words apart.
fn draft_text(t: &str, open: bool, cites: &Cites) -> DraftText {
    if !open {
        return DraftText {
            head: cites.pieces(t.trim()),
            tail: Vec::new(),
        };
    }
    let t = without_cut_marker(t.trim_end());
    let mut at = t.len();
    for _ in 0..FADED_WORDS {
        match t[..at].trim_end().rfind(char::is_whitespace) {
            Some(i) => at = i,
            None => {
                at = 0;
                break;
            }
        }
    }
    // Never inside a citation marker.
    if let Some(i) = t[..at].rfind('[')
        && !t[i..at].contains(']')
    {
        at = i;
    }
    DraftText {
        head: cites.pieces(&t[..at]),
        tail: cites.pieces(&t[at..]),
    }
}

fn is_blank(t: &DraftText) -> bool {
    let blank = |p: &[Piece]| {
        p.iter().all(|x| match x {
            Piece::Text(t) => t.trim().is_empty(),
            Piece::Cite(_) => false,
        })
    };
    blank(&t.head) && blank(&t.tail)
}

fn key(k: &str) -> Seg {
    Seg::Key(k.to_string())
}

/// Reads sections out of one reply.
struct Layout<'a> {
    p: &'a Partial,
    cites: Cites,
}

impl Layout<'_> {
    /// The value at `path`.
    fn get(&self, path: &[Seg]) -> Option<&Value> {
        let mut v = &self.p.value;
        for s in path {
            v = match s {
                Seg::Key(k) => v.get(k.as_str())?,
                Seg::Index(i) => v.get(*i)?,
            };
        }
        Some(v)
    }

    fn str_at(&self, path: &[Seg]) -> &str {
        self.get(path).and_then(Value::as_str).unwrap_or("")
    }

    /// The text at `path`, whole or with its last words apart.
    fn text(&self, path: &[Seg]) -> DraftText {
        draft_text(self.str_at(path), self.p.is_open(path), &self.cites)
    }

    /// A paragraph for the text at `path`, unless there is none yet.
    fn para(&self, label: Option<&str>, path: &[Seg]) -> Option<Item> {
        let text = self.text(path);
        (!is_blank(&text)).then(|| Item {
            kind: Kind::Para,
            label: label.map(str::to_string),
            text,
            faded: false,
        })
    }

    /// The lines of the flat text at `path`.
    fn lines(&self, path: &[Seg]) -> Vec<(String, bool)> {
        let raw = self.str_at(path);
        let open = self.p.is_open(path);
        let lines: Vec<&str> = raw
            .lines()
            .map(unbullet)
            .filter(|l| !l.is_empty())
            .collect();
        let n = lines.len();
        lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| (l.to_string(), open && i + 1 == n))
            .collect()
    }

    fn line_item(&self, kind: Kind, label: Option<&str>, line: &str, open: bool) -> Option<Item> {
        let text = draft_text(line, open, &self.cites);
        (!is_blank(&text)).then(|| Item {
            kind,
            label: label.map(str::to_string),
            text,
            faded: false,
        })
    }

    /// The items of the list at `path`, one by `item`. The one still being
    /// written is faded whole.
    fn list(&self, path: &[Seg], item: impl Fn(&Self, &[Seg]) -> Option<Item>) -> Vec<Item> {
        let n = self.get(path).and_then(Value::as_array).map_or(0, Vec::len);
        (0..n)
            .filter_map(|i| {
                let mut at = path.to_vec();
                at.push(Seg::Index(i));
                let open = self.p.is_open(&at);
                let mut it = item(self, &at)?;
                it.faded = open;
                if open {
                    // Faded whole: its words need no second fade.
                    let tail = std::mem::take(&mut it.text.tail);
                    it.text.head.extend(tail);
                }
                Some(it)
            })
            .collect()
    }

    /// "N written so far" while the list at `path` is still being written.
    fn so_far(&self, path: &[Seg]) -> Option<String> {
        let n = self.get(path).and_then(Value::as_array).map(Vec::len)?;
        let done = if self.p.is_open(path) {
            let last_open = self.p.open.as_ref().is_some_and(|o| o.len() > path.len());
            n.saturating_sub(usize::from(last_open))
        } else {
            return None;
        };
        (done > 0).then(|| format!("{done} written so far"))
    }
}

fn unbullet(l: &str) -> &str {
    let l = l.trim();
    l.strip_prefix("- ")
        .or_else(|| l.strip_prefix("* "))
        .or_else(|| l.strip_prefix("\u{2022} "))
        .map_or(l, str::trim_start)
}

/// `line` after any of `labels` and a colon (case ignored).
fn after_label<'a>(line: &'a str, labels: &[&str]) -> Option<&'a str> {
    let (head, rest) = line.split_once(':')?;
    labels
        .iter()
        .any(|l| head.trim().eq_ignore_ascii_case(l))
        .then(|| rest.trim_start())
}

fn section(title: &'static str, items: Vec<Item>) -> Section {
    Section {
        title,
        note: None,
        items,
    }
}

/// At a glance: the flat text ("Verdict", key numbers, "Main caveat"), or an
/// object as stored.
fn glance(l: &Layout<'_>) -> Vec<Item> {
    let at = [key("at_a_glance")];
    if l.get(&at).is_some_and(Value::is_object) {
        let v = [key("at_a_glance"), key("verdict")];
        let mut items: Vec<Item> = l.para(None, &v).into_iter().collect();
        items.extend(l.list(&[key("at_a_glance"), key("key_numbers")], |l, p| {
            let mut q = p.to_vec();
            q.push(key("value_text"));
            let text = l.text(&q);
            q.pop();
            q.push(key("label"));
            let label = l.str_at(&q).trim().to_string();
            (!is_blank(&text)).then(|| Item {
                kind: Kind::Bullet,
                label: (!label.is_empty()).then_some(label),
                text,
                faded: false,
            })
        }));
        let c = [key("at_a_glance"), key("main_caveat")];
        items.extend(l.para(Some("Main caveat."), &c));
        return items;
    }
    let mut items = Vec::new();
    for (i, (line, open)) in l.lines(&at).into_iter().enumerate() {
        let item = if i == 0 {
            let v = after_label(&line, &["Verdict", "At a glance"]).unwrap_or(&line);
            l.line_item(Kind::Para, None, v, open)
        } else if let Some(c) = after_label(&line, &["Main caveat", "Caveat"]) {
            l.line_item(Kind::Para, Some("Main caveat."), c, open)
        } else {
            let k = after_label(&line, &["Key number", "Key numbers"]).unwrap_or(&line);
            l.line_item(Kind::Bullet, None, k, open)
        };
        items.extend(item);
    }
    items
}

/// Clinical practice: the labelled flat text, or an object as stored.
fn practice(l: &Layout<'_>) -> Vec<Item> {
    const PARTS: [(&str, &[&str], &str); 4] = [
        ("Default choice.", &["Default choice"], "default_choice"),
        ("Who may benefit.", &["Who may benefit"], "who_may_benefit"),
        (
            "Counselling.",
            &["Counselling", "Counseling"],
            "counselling",
        ),
        ("Cost per case.", &["Cost per case"], "cost_per_case"),
    ];
    let at = [key("clinical_practice")];
    if l.get(&at).is_some_and(Value::is_object) {
        let mut items: Vec<Item> = PARTS
            .iter()
            .filter_map(|(label, _, f)| l.para(Some(label), &[key("clinical_practice"), key(f)]))
            .collect();
        items.extend(l.para(None, &[key("clinical_practice"), key("text")]));
        return items;
    }
    l.lines(&at)
        .into_iter()
        .filter_map(|(line, open)| {
            let part = PARTS
                .iter()
                .find_map(|(label, names, _)| after_label(&line, names).map(|b| (*label, b)));
            match part {
                Some((label, body)) => l.line_item(Kind::Para, Some(label), body, open),
                None => l.line_item(Kind::Para, None, &line, open),
            }
        })
        .collect()
}

/// Notable findings: the flat lines, or a list as stored.
fn notable(l: &Layout<'_>) -> Vec<Item> {
    let at = [key("notable_findings")];
    if l.get(&at).is_some_and(Value::is_array) {
        return l.list(&at, |l, p| {
            let mut q = p.to_vec();
            q.push(key("text"));
            l.para(None, &q).map(|mut i| {
                i.kind = Kind::Bullet;
                i
            })
        });
    }
    l.lines(&at)
        .into_iter()
        .filter_map(|(line, open)| l.line_item(Kind::Bullet, None, &line, open))
        .collect()
}

/// The answer's sections, in the order of the finished page
/// ([`crate::answer_page::SECTION_ORDER`] for those under Details).
pub fn read_answer(reply: &str) -> Vec<Section> {
    let p = read_prefix(reply);
    let l = Layout {
        p: &p,
        cites: Cites::numbered(reply),
    };
    let mut bottom: Vec<Item> = l
        .para(None, &[key("bottom_line"), key("text")])
        .into_iter()
        .collect();
    let certainty = [key("bottom_line"), key("certainty")];
    let reason = [key("bottom_line"), key("certainty_reason")];
    if !l.p.is_open(&certainty) && !l.str_at(&certainty).is_empty() {
        let label = format!("Certainty: {}.", l.str_at(&certainty));
        match l.para(Some(&label), &reason) {
            Some(i) => bottom.push(i),
            None => bottom.push(Item {
                kind: Kind::Para,
                label: Some(label),
                text: DraftText::default(),
                faded: false,
            }),
        }
    } else {
        bottom.extend(l.para(None, &reason));
    }
    let outcomes = [key("outcomes")];
    let studies = [key("studies")];
    let conflicts = [key("conflicts")];
    let would = [key("would_change")];
    vec![
        section("At a glance", glance(&l)),
        section("Bottom line", bottom),
        section("What this means for clinical practice", practice(&l)),
        Section {
            title: "Outcome by outcome",
            note: l.so_far(&outcomes),
            items: l.list(&outcomes, |l, p| {
                let mut q = p.to_vec();
                q.push(key("outcome"));
                let name = l.str_at(&q).trim().to_string();
                q.pop();
                q.push(key("effect_text"));
                let text = l.text(&q);
                (!name.is_empty() || !is_blank(&text)).then(|| Item {
                    kind: Kind::Entry,
                    label: (!name.is_empty()).then_some(name),
                    text,
                    faded: false,
                })
            }),
        },
        section(
            "Conflicting evidence",
            l.list(&conflicts, |l, p| {
                let mut q = p.to_vec();
                q.push(key("text"));
                l.para(None, &q).map(|mut i| {
                    i.kind = Kind::Bullet;
                    i
                })
            }),
        ),
        section("Applicability", {
            [
                ("Case mix.", "case_mix"),
                ("India and cost.", "india_cost_context"),
                ("Training.", "training"),
            ]
            .iter()
            .filter_map(|(label, f)| l.para(Some(label), &[key("applicability"), key(f)]))
            .collect()
        }),
        section(
            "What would change this answer",
            l.list(&would, |l, p| {
                l.para(None, p).map(|mut i| {
                    i.kind = Kind::Bullet;
                    i
                })
            }),
        ),
        Section {
            title: "The trials",
            note: l.so_far(&studies),
            items: l.list(&studies, |l, p| {
                let field = |f: &str| {
                    let mut q = p.to_vec();
                    q.push(key(f));
                    q
                };
                let name = l.str_at(&field("label")).trim().to_string();
                let text = l.text(&field("key_results"));
                (!name.is_empty() || !is_blank(&text)).then(|| Item {
                    kind: Kind::Entry,
                    label: (!name.is_empty()).then_some(name),
                    text,
                    faded: false,
                })
            }),
        },
        section("Other notable findings from this run", notable(&l)),
    ]
}

/// The opinion's sections, in the Expert view's order.
pub fn read_opinion(reply: &str) -> Vec<Section> {
    let p = read_prefix(reply);
    let l = Layout {
        p: &p,
        cites: Cites::AsWritten,
    };
    let glance_at = |f: &str| [key("at_a_glance"), key(f)];
    let glance: Vec<Item> = [
        ("Original", "original_quote"),
        ("Expert", "expert"),
        ("Today", "today"),
    ]
    .iter()
    .filter_map(|(label, f)| l.para(Some(label), &glance_at(f)))
    .collect();
    let decisions = [key("decisions")];
    vec![
        section(
            "Expert opinion",
            l.para(None, &[key("opinion"), key("verdict")])
                .into_iter()
                .collect(),
        ),
        section(
            "Expert view on why",
            l.para(None, &[key("opinion"), key("why")])
                .into_iter()
                .collect(),
        ),
        section("At a glance", glance),
        Section {
            title: "Decision by scenario",
            note: l.so_far(&decisions),
            items: l.list(&decisions, |l, p| {
                let field = |f: &str| {
                    let mut q = p.to_vec();
                    q.push(key(f));
                    q
                };
                let name = l.str_at(&field("scenario")).trim().to_string();
                let choice = l.str_at(&field("choice"));
                let reason = l.str_at(&field("reason"));
                let joined = format!("{choice} {reason}");
                let open = l.p.is_open(p);
                let text = draft_text(joined.trim(), open, &l.cites);
                (!name.is_empty() || !is_blank(&text)).then(|| Item {
                    kind: Kind::Entry,
                    label: (!name.is_empty()).then_some(name),
                    text,
                    faded: false,
                })
            }),
        },
    ]
}

/// What one poll means for a page showing a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollStep {
    /// Nothing changed: leave the page alone.
    Same,
    /// Only the draft moved on: redraw what it changes, and nothing else.
    Draft(String),
    /// Something else changed (a step line, the status, the answer) or there
    /// is no draft to show any more: draw the page again.
    Rebuild,
}

/// `now` against the page drawn from `drawn`, with `shown` as its draft.
pub fn poll_step(drawn: &RunView, shown: &str, now: &RunView) -> PollStep {
    let same = drawn.summary == now.summary
        && drawn.steps == now.steps
        && drawn.answer == now.answer
        && drawn.answer_version == now.answer_version
        && drawn.intro == now.intro;
    match &now.draft {
        Some(t) if same && has_content(&read_answer(t)) => {
            if t == shown {
                PollStep::Same
            } else {
                PollStep::Draft(t.clone())
            }
        }
        _ => PollStep::Rebuild,
    }
}

/// Anything to draw yet.
pub fn has_content(sections: &[Section]) -> bool {
    sections.iter().any(|s| !s.items.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn path(p: &[&str]) -> Vec<Seg> {
        p.iter().map(|k| key(k)).collect()
    }

    #[test]
    fn the_whole_reply_reads_as_it_parses() {
        let t = r#" {"a": {"b": [1, 2.5, -3e2, true, false, null, "x\n\"éé"]}, "c": ""} "#;
        let p = read_prefix(t);
        assert_eq!(p.open, None);
        assert_eq!(p.value, serde_json::from_str::<Value>(t).unwrap());
    }

    #[test]
    fn a_cut_string_keeps_its_text_and_marks_the_path() {
        let p = read_prefix(r#"{"a":{"b":"hello wor"#);
        assert_eq!(p.value, json!({"a":{"b":"hello wor"}}));
        assert_eq!(p.open, Some(path(&["a", "b"])));
    }

    #[test]
    fn unfinished_numbers_words_escapes_and_keys_are_left_out() {
        for (t, want) in [
            (r#"{"a":12"#, json!({})),
            (r#"{"a":tr"#, json!({})),
            (r#"{"a":"x\"#, json!({"a":"x"})),
            (r#"{"a":"x\u00"#, json!({"a":"x"})),
            (r#"{"a":"x\ud83d"#, json!({"a":"x"})),
            (r#"{"a":"x\ud83d\ude"#, json!({"a":"x"})),
            (r#"{"a":1,"b"#, json!({"a":1})),
            (r#"{"a":1,"b":"#, json!({"a":1})),
            (r#"{"a":[1,"#, json!({"a":[1]})),
            ("not json", Value::Null),
            ("", Value::Null),
        ] {
            assert_eq!(read_prefix(t).value, want, "{t}");
        }
    }

    fn run(draft: Option<&str>) -> RunView {
        serde_json::from_value(json!({
            "summary": {
                "id": "00000000-0000-0000-0000-000000000001",
                "mode": "focused",
                "status": "running",
                "created_at": "2026-11-05T10:00:00Z",
                "cost_paise": 5,
                "title": "q"
            },
            "steps": [],
            "answer": null,
            "answer_version": 2,
            "draft": draft
        }))
        .unwrap()
    }

    #[test]
    fn a_poll_redraws_only_what_changed() {
        let t = r#"{"bottom_line":{"text":"So far"#;
        let longer = r#"{"bottom_line":{"text":"So far and more"#;
        let drawn = run(Some(t));
        assert_eq!(poll_step(&drawn, t, &run(Some(t))), PollStep::Same);
        assert_eq!(
            poll_step(&drawn, t, &run(Some(longer))),
            PollStep::Draft(longer.to_string())
        );
        // The draft is what was shown last, not what the page was drawn from.
        assert_eq!(
            poll_step(&drawn, longer, &run(Some(longer))),
            PollStep::Same
        );
        // No draft any more (a retry started again), or nothing to draw yet.
        assert_eq!(poll_step(&drawn, t, &run(None)), PollStep::Rebuild);
        assert_eq!(poll_step(&drawn, t, &run(Some("{"))), PollStep::Rebuild);
        // Anything else: the whole page, as every poll did.
        let mut other = run(Some(t));
        other.summary.status = crate::runs::RunStatus::Done;
        assert_eq!(poll_step(&drawn, t, &other), PollStep::Rebuild);
        let mut other = run(Some(t));
        other.summary.cost_paise = 6;
        assert_eq!(poll_step(&drawn, t, &other), PollStep::Rebuild);
        let mut other = run(Some(t));
        other.answer = Some(json!({"version": 3}));
        assert_eq!(poll_step(&drawn, t, &other), PollStep::Rebuild);
    }

    #[test]
    fn a_broken_surrogate_pair_reads_as_a_replacement_character() {
        for (t, want) in [
            (r#"{"a":"\ud83d\u0041"}"#, "\u{fffd}"),
            (r#"{"a":"\ud83d\ud83d"}"#, "\u{fffd}"),
            (r#"{"a":"\ud83d\uffff"}"#, "\u{fffd}"),
            (r#"{"a":"\ud83dA"}"#, "\u{fffd}A"),
            (r#"{"a":"\ud83d\ude00"}"#, "\u{1f600}"),
        ] {
            assert_eq!(read_prefix(t).value, json!({"a": want}), "{t}");
        }
    }

    #[test]
    fn nonsense_never_panics_or_runs_away() {
        for t in [
            "{{{{",
            "{\"a\":]",
            "{\"a\"::",
            "{]",
            "[1",
            "{\"a\":1}}}",
            "{\"é",
            "\u{0}{",
        ] {
            let _ = read_prefix(t);
            let _ = read_answer(t);
            let _ = read_opinion(t);
        }
        let deep = "[".repeat(100_000);
        let _ = read_prefix(&format!("{{\"a\":{deep}"));
    }

    fn titled<'a>(s: &'a [Section], t: &str) -> &'a Section {
        s.iter().find(|x| x.title == t).unwrap()
    }

    fn text_of(p: &[Piece]) -> String {
        p.iter()
            .map(|x| match x {
                Piece::Text(t) | Piece::Cite(t) => t.as_str(),
            })
            .collect()
    }

    #[test]
    fn sections_stay_in_page_order_and_the_rest_are_coming() {
        let s = read_answer(r#"{"applicability":{"case_mix":"Mixed.","india"#);
        let titles: Vec<&str> = s.iter().map(|x| x.title).collect();
        assert_eq!(
            titles,
            [
                "At a glance",
                "Bottom line",
                "What this means for clinical practice",
                "Outcome by outcome",
                "Conflicting evidence",
                "Applicability",
                "What would change this answer",
                "The trials",
                "Other notable findings from this run"
            ]
        );
        assert!(titled(&s, "Bottom line").items.is_empty(), "coming");
        assert_eq!(titled(&s, "Applicability").items.len(), 1);
    }

    #[test]
    fn the_last_words_of_an_unfinished_text_are_faded() {
        let s = read_answer(
            r#"{"bottom_line":{"certainty":"Low","certainty_reason":"Few small trials.","text":"Trials show no benefit and usually take longer"#,
        );
        let b = &titled(&s, "Bottom line").items;
        assert_eq!(text_of(&b[0].text.head), "Trials show no benefit and");
        assert_eq!(text_of(&b[0].text.tail), " usually take longer");
        assert_eq!(b[1].label.as_deref(), Some("Certainty: Low."));
        assert_eq!(text_of(&b[1].text.head), "Few small trials.");
        assert!(b[1].text.tail.is_empty());
    }

    #[test]
    fn an_unfinished_certainty_word_is_not_shown() {
        let s = read_answer(r#"{"bottom_line":{"certainty":"Lo"#);
        assert!(titled(&s, "Bottom line").items.is_empty());
    }

    #[test]
    fn citations_are_numbered_as_they_first_appear_and_stay_put() {
        let early = r#"{"bottom_line":{"text":"A [41060645; 39979619]. B [41060645]."#;
        let s = read_answer(early);
        let b = &titled(&s, "Bottom line").items[0].text;
        let all: String = text_of(&b.head) + &text_of(&b.tail);
        assert_eq!(all, "A [1, 2]. B [1].");
        // A cut-off marker is left out until it is whole.
        let s = read_answer(r#"{"bottom_line":{"text":"A first part of text [4106"#);
        let b = &titled(&s, "Bottom line").items[0].text;
        assert_eq!(text_of(&b.head) + &text_of(&b.tail), "A first part of text");
        // Numbers in ordinary brackets stay as written.
        let s = read_answer(r#"{"bottom_line":{"text":"LOS 2 [1, 3] days."}}"#);
        let b = &titled(&s, "Bottom line").items[0].text;
        assert_eq!(text_of(&b.head), "LOS 2 [1, 3] days.");
    }

    #[test]
    fn a_list_shows_finished_items_and_fades_the_unfinished_one() {
        let s = read_answer(
            r#"{"outcomes":[{"certainty":"Low","effect_text":"No difference.","outcome":"Pain"},{"certainty":"Low","effect_text":"One recurrence with"#,
        );
        let o = titled(&s, "Outcome by outcome");
        assert_eq!(o.note.as_deref(), Some("1 written so far"));
        assert_eq!(o.items.len(), 2);
        assert_eq!(o.items[0].label.as_deref(), Some("Pain"));
        assert!(!o.items[0].faded);
        assert!(o.items[1].faded);
        assert!(o.items[1].text.tail.is_empty());
        // Finished: no note.
        let s = read_answer(r#"{"outcomes":[{"effect_text":"x","outcome":"Pain"}],"#);
        assert_eq!(titled(&s, "Outcome by outcome").note, None);
    }

    #[test]
    fn the_flat_at_a_glance_and_practice_read_by_their_labels() {
        let s = read_answer(concat!(
            r#"{"at_a_glance":"Verdict: Robotic is no better.\n- Pain: 4 vs 5 [41060645]\nMain caveat: Few trials.","#,
            r#""clinical_practice":"Default choice: Laparoscopic.\nCounselling: Similar pain"#
        ));
        let g = &titled(&s, "At a glance").items;
        assert_eq!(text_of(&g[0].text.head), "Robotic is no better.");
        assert_eq!(g[1].kind, Kind::Bullet);
        assert_eq!(text_of(&g[1].text.head), "Pain: 4 vs 5 [1]");
        assert_eq!(g[2].label.as_deref(), Some("Main caveat."));
        let p = &titled(&s, "What this means for clinical practice").items;
        assert_eq!(p[0].label.as_deref(), Some("Default choice."));
        assert_eq!(p[1].label.as_deref(), Some("Counselling."));
    }

    #[test]
    fn the_opinion_keeps_the_expert_views_order_and_its_reference_numbers() {
        let s = read_opinion(
            r#"{"at_a_glance":{"expert":"Selective use [3]."},"decisions":[{"choice":"Laparoscopic","reason":"Cheaper [1].","scenario":"Primary hernia"},{"choice":"Rob"#,
        );
        let titles: Vec<&str> = s.iter().map(|x| x.title).collect();
        assert_eq!(
            titles,
            [
                "Expert opinion",
                "Expert view on why",
                "At a glance",
                "Decision by scenario"
            ]
        );
        assert!(s[0].items.is_empty());
        assert_eq!(text_of(&s[2].items[0].text.head), "Selective use [3].");
        assert_eq!(s[3].items[0].label.as_deref(), Some("Primary hernia"));
        assert!(s[3].items[1].faded);
        assert!(has_content(&s));
        assert!(!has_content(&read_opinion("{")));
    }
}
