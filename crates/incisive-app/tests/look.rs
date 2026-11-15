//! The look's colour rule: the page chrome's styles (`CHROME_STYLES`) take
//! every colour from a variable, so a look (and a night mode later) restates
//! the variables and nothing else. The ivory values are the ones the styles
//! held as literals before the move.

use std::collections::BTreeSet;

use incisive_app::app::{CHROME_STYLES, IVORY_STYLE, STIPPLE_STYLE};
use regex::Regex;

/// A colour written out: `#` and 3, 4, 6 or 8 hex digits, or a colour function.
fn colour_literal() -> Regex {
    Regex::new(r"#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b|\b(?:rgba?|hsla?)\(")
        .unwrap()
}

/// A line's pieces between `{`, `}` and `;`: one declaration (or a selector) each.
fn pieces(line: &str) -> impl Iterator<Item = &str> {
    line.split(['{', '}', ';']).map(str::trim)
}

/// The custom property a piece declares (`--name:value`), with its value.
fn definition(piece: &str) -> Option<(&str, &str)> {
    let (name, value) = piece.split_once(':')?;
    let name = name.trim();
    let ok = name.starts_with("--")
        && name[2..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| (name, value.trim()))
}

/// Every custom property a style declares, with its value.
fn definitions(css: &str) -> Vec<(&str, &str)> {
    css.lines()
        .flat_map(pieces)
        .filter_map(definition)
        .collect()
}

/// The variables a style reads (`var(--name)`).
fn used(css: &str) -> BTreeSet<String> {
    Regex::new(r"var\(\s*(--[A-Za-z0-9_-]+)")
        .unwrap()
        .captures_iter(css)
        .map(|c| c[1].to_string())
        .collect()
}

fn style(name: &str) -> &'static str {
    CHROME_STYLES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("{name} is not in CHROME_STYLES"))
        .1
}

/// The declarations of the ivory `:root` block.
fn ivory_root() -> Vec<(&'static str, &'static str)> {
    let start = IVORY_STYLE.find(":root {").expect("the ivory :root block");
    let block = &IVORY_STYLE[start..];
    let block = &block[..block.find('}').expect("the block's end")];
    definitions(block)
}

#[test]
fn the_list_names_every_chrome_style() {
    let names: Vec<&str> = CHROME_STYLES.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "IVORY_STYLE",
            "EXPLORE_STYLE",
            "TRACKER_STYLE",
            "INTRO_STYLE",
            "DRAFT_STYLE",
            "EXPERT_STYLE",
            "CHAT_STYLE",
            "STIPPLE_STYLE",
        ]
    );
    for (name, css) in CHROME_STYLES {
        assert!(!css.trim().is_empty(), "{name} is empty");
    }
    assert_eq!(style("IVORY_STYLE"), IVORY_STYLE);
}

#[test]
fn chrome_styles_take_every_colour_from_a_variable() {
    let literal = colour_literal();
    let mut bad = Vec::new();
    for (name, css) in CHROME_STYLES {
        for (i, line) in css.lines().enumerate() {
            for piece in pieces(line) {
                if literal.is_match(piece) && definition(piece).is_none() {
                    bad.push(format!(
                        "{name}, line {}: `{piece}` in `{}`",
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "colours written out instead of read from a variable:\n{}",
        bad.join("\n")
    );
}

/// The colour literals each chrome style held at `c0683b6` (before the move
/// to variables), sorted and unique. The last entry is the pill fill that
/// `run_tracker::fill_background` wrote into a `style` attribute.
const BEFORE: &[(&str, &[&str])] = &[
    (
        "IVORY_STYLE",
        &[
            "#1C1B19",
            "#223a6a",
            "#2C4A85",
            "#4A4741",
            "#6B675F",
            "#8A5A00",
            "#B42318",
            "#BDB6AA",
            "#D9D3C8",
            "#E7E3DC",
            "#E8E4DC",
            "#ECE8E1",
            "#ECEFF8",
            "#EFECE6",
            "#F5F3EF",
            "#FAF8F5",
            "#FDECEA",
            "#FDF6E7",
            "#FFFFFF",
            "#fff",
            "rgba(28,27,25,0.12)",
        ],
    ),
    (
        "EXPLORE_STYLE",
        &[
            "#1F7A6E",
            "#223a6a",
            "#635A8C",
            "#7A2E12",
            "#BDB6A8",
            "#C2552F",
            "#D49A1F",
            "#D97E58",
            "#E8A98D",
            "#E8E4DC",
            "#F2CDBB",
            "#FDECEA",
            "#fff",
            "rgba(0,0,0,0.18)",
            "rgba(255,255,255,0.86)",
        ],
    ),
    (
        "TRACKER_STYLE",
        &[
            "#557060", "#56688F", "#5E8A72", "#6B675F", "#A9B8D6", "#E8F1EC", "#EFF2F9",
        ],
    ),
    ("INTRO_STYLE", &[]),
    ("DRAFT_STYLE", &["#8B877F", "#F0E2C2"]),
    (
        "EXPERT_STYLE",
        &[
            "#2F6B4F", "#2f5d50", "#7a4a8c", "#E6F1EB", "#a4522c", "#dde6ef", "#e2e9e0", "#e3ede8",
            "#ece4cf", "#eee4f2", "#f3e2d8", "#fff",
        ],
    ),
    (
        "CHAT_STYLE",
        &["#6B4600", "#FDECEA", "#fff", "rgba(28,27,25,.42)"],
    ),
    ("TRACKER_STYLE", &["#E6ECF8", "#FFFFFF"]),
];

/// The ivory tokens as they were before the move: none may change.
const IVORY_TOKENS: &[(&str, &str)] = &[
    ("--iv-ground", "#F5F3EF"),
    ("--iv-card", "#FFFFFF"),
    ("--iv-border", "#E7E3DC"),
    ("--iv-ink", "#1C1B19"),
    ("--iv-body", "#4A4741"),
    ("--iv-muted", "#6B675F"),
    ("--iv-accent", "#2C4A85"),
    ("--iv-tint", "#ECEFF8"),
    ("--iv-alert", "#B42318"),
    ("--iv-rule", "#EFECE6"),
    ("--iv-soft", "#FAF8F5"),
    ("--iv-line", "#D9D3C8"),
    ("--iv-off", "#E8E4DC"),
    ("--iv-warn", "#8A5A00"),
    ("--iv-warn-bg", "#FDF6E7"),
];

#[test]
fn ivory_values_are_what_they_were() {
    let root = ivory_root();
    for (name, value) in IVORY_TOKENS {
        let found: Vec<&str> = root
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(found, [*value], "{name} in the ivory :root block");
    }
    // No ivory variable is declared twice: the value read is the one written.
    let names: BTreeSet<&str> = root.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names.len(),
        root.len(),
        "a variable is declared twice in :root"
    );

    let mut lost = Vec::new();
    for (name, literals) in BEFORE {
        // The ivory :root block, or the style's own variable declarations.
        let own = definitions(style(name));
        for literal in *literals {
            let held = root.iter().chain(own.iter()).any(|(_, v)| v == literal);
            if !held {
                lost.push(format!("{name}: {literal}"));
            }
        }
    }
    assert!(
        lost.is_empty(),
        "colours the ivory look had that no variable holds now:\n{}",
        lost.join("\n")
    );
}

/// Variables the chrome reads that no chrome style declares. `--m` is the
/// Explore map's dot size, set on each dot's own `style` attribute. (The
/// chrome borrows none of the answer styles' variables.)
const SET_ELSEWHERE: &[&str] = &["--m"];

/// Variables a style reads from another block, always with a fallback
/// (`var(--name, fallback)`), so the rule holds with or without that block.
/// `--rt-era-bg` is the Temporal board's era colour, set per era by
/// `EXPERT_STYLE` on the era card itself; the stipple rule repaints the
/// same card and only swaps the fallback for eras without a colour.
const READ_WITH_FALLBACK: &[(&str, &str)] = &[("STIPPLE_STYLE", "--rt-era-bg")];

/// A style reads only what the ivory `:root` block declares or what it
/// declares itself: the blocks are sent to the page separately (some only
/// with their own component), so a variable declared in another block may
/// not be there.
#[test]
fn no_variable_is_used_without_being_declared() {
    let root: BTreeSet<&str> = ivory_root().into_iter().map(|(n, _)| n).collect();
    let mut bad = Vec::new();
    for (name, css) in CHROME_STYLES {
        let own: BTreeSet<&str> = definitions(css).into_iter().map(|(n, _)| n).collect();
        for var in used(css) {
            let v = var.as_str();
            let declared = root.contains(v)
                || own.contains(v)
                || SET_ELSEWHERE.contains(&v)
                || READ_WITH_FALLBACK.contains(&(*name, v));
            if !declared {
                bad.push(format!("{name}: {var}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "variables read but declared neither in the ivory :root nor in the same style:\n{}",
        bad.join("\n")
    );
    for (name, v) in READ_WITH_FALLBACK {
        let bare = format!("var({v})");
        assert!(!style(name).contains(&bare), "{name} reads {v} bare");
        assert!(style(name).contains(&format!("var({v},")), "{name}: {v}");
    }
    for v in SET_ELSEWHERE {
        let anywhere = CHROME_STYLES
            .iter()
            .flat_map(|(_, css)| definitions(css))
            .any(|(n, _)| n == *v);
        assert!(!anywhere, "{v} is declared now: take it off the list");
    }
}

#[test]
fn the_pill_fill_reads_its_colours_from_variables() {
    let fill = incisive_app::run_tracker::fill_background(52);
    assert!(!colour_literal().is_match(&fill), "{fill}");
    let declared: BTreeSet<&str> = ivory_root().into_iter().map(|(n, _)| n).collect();
    for var in used(&fill) {
        assert!(
            declared.contains(var.as_str()),
            "{var} is not an ivory variable"
        );
    }
}

// ---------- the stipple look over the chrome styles ----------

const SCOPE: &str = "html[data-look=\"stipple\"] ";

/// One style rule: its selectors (a list split at the commas) and its
/// declarations (`name`, `value`).
struct Rule {
    selectors: Vec<String>,
    declarations: Vec<(String, String)>,
}

/// The rules of a style, those inside `@media` and `@supports` included.
/// `@font-face` and `@keyframes` blocks are skipped (no selectors there).
/// Comments are dropped. None of the chrome styles has a comma inside a
/// selector's brackets, so a list splits at every comma.
fn rules(css: &str) -> Vec<Rule> {
    let css = Regex::new(r"(?s)/\*.*?\*/").unwrap().replace_all(css, "");
    let mut out = Vec::new();
    // What a `}` closes: a group (`@media`), a skipped block, or a rule.
    let mut open: Vec<&str> = Vec::new();
    let mut text = String::new();
    let mut selectors: Vec<String> = Vec::new();
    for c in css.chars() {
        match c {
            '{' => {
                let head = text.trim().to_string();
                text.clear();
                if open.last() == Some(&"skip") || open.last() == Some(&"rule") {
                    open.push("skip");
                } else if head.starts_with("@media") || head.starts_with("@supports") {
                    open.push("group");
                } else if head.starts_with('@') {
                    open.push("skip");
                } else {
                    selectors = head
                        .split(',')
                        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                        .collect();
                    open.push("rule");
                }
            }
            '}' => {
                if open.pop() == Some("rule") {
                    let declarations = text
                        .split(';')
                        .filter_map(|d| d.split_once(':'))
                        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
                        .collect();
                    out.push(Rule {
                        selectors: std::mem::take(&mut selectors),
                        declarations,
                    });
                }
                text.clear();
            }
            _ => text.push(c),
        }
    }
    assert!(open.is_empty(), "unbalanced braces");
    out
}

/// The chrome styles a look restyles: every one but the stipple block.
fn restyled() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    CHROME_STYLES.iter().filter(|(n, _)| *n != "STIPPLE_STYLE")
}

fn heavy(value: &str) -> bool {
    let v = value.trim_end_matches("!important").trim();
    v == "bold" || v == "bolder" || v.parse::<u32>().is_ok_and(|w| w > 500)
}

/// The fonts of the stipple look stop at 500: a heavier weight is faked by
/// the browser. So every selector that asks for more in a chrome style (the
/// whole of each: Ask, the Library, Explore, the tracker, the introduction,
/// the draft, the Expert view and its board, the chat, the run page and
/// Activity) is said again under the look's scope, where it outranks the
/// original, at 300, 400 or 500.
#[test]
fn stipple_caps_every_heavy_weight() {
    let stipple = rules(STIPPLE_STYLE);
    let weight_of = |selector: &str| -> Option<String> {
        stipple
            .iter()
            .filter(|r| r.selectors.iter().any(|s| s == selector))
            .flat_map(|r| r.declarations.iter())
            .filter(|(n, _)| n == "font-weight")
            .map(|(_, v)| v.clone())
            .next_back()
    };
    let mut found = 0;
    let mut bad = Vec::new();
    for (name, css) in restyled() {
        for rule in rules(css) {
            let is_heavy = rule
                .declarations
                .iter()
                .any(|(n, v)| n == "font-weight" && heavy(v));
            // The shorthand could carry a weight too: none does.
            for (n, v) in &rule.declarations {
                assert!(
                    n != "font" || !(v.contains("bold") || v.contains("600") || v.contains("700")),
                    "{name}: a weight in a `font` shorthand: {v}"
                );
            }
            if !is_heavy {
                continue;
            }
            for selector in &rule.selectors {
                found += 1;
                let scoped = format!("{SCOPE}{selector}");
                match weight_of(&scoped).as_deref() {
                    Some("300" | "400" | "500") => {}
                    Some(other) => bad.push(format!("{name}: `{selector}` restated at {other}")),
                    None => bad.push(format!("{name}: `{selector}` is not restated")),
                }
            }
        }
    }
    // The reviewer's counts add up to more than a hundred.
    assert!(found > 110, "only {found} heavy selectors read");
    assert!(
        bad.is_empty(),
        "heavy weights the stipple look leaves:\n{}",
        bad.join("\n")
    );
    // And the look itself asks for nothing heavier.
    for rule in &stipple {
        for (n, v) in &rule.declarations {
            assert!(n != "font-weight" || !heavy(v), "{:?}: {v}", rule.selectors);
        }
    }
}

/// Boxes that may keep a solid ground-coloured background in the stipple
/// look, each with its reason. (`body` needs no entry: the look restates it
/// as transparent, so the dots show.)
const GROUND_ALLOWED: &[&str] = &[];

/// A box painted in the ground colour hides the dot layer behind it: a
/// dot-free patch on the dotted page. Every chrome rule that paints one is
/// given another background under the look's scope.
#[test]
fn stipple_paints_no_solid_ground_box() {
    let paints_ground = |r: &Rule| {
        r.declarations.iter().any(|(n, v)| {
            (n == "background" || n == "background-color") && v.contains("--iv-ground")
        })
    };
    let stipple = rules(STIPPLE_STYLE);
    let repainted = |selector: &str| {
        stipple.iter().any(|r| {
            r.selectors.iter().any(|s| s == selector)
                && !paints_ground(r)
                && r.declarations
                    .iter()
                    .any(|(n, _)| n == "background" || n == "background-color")
        })
    };
    let mut found = 0;
    let mut bad = Vec::new();
    for (name, css) in restyled() {
        for rule in rules(css).iter().filter(|r| paints_ground(r)) {
            for selector in &rule.selectors {
                found += 1;
                if GROUND_ALLOWED.contains(&selector.as_str()) {
                    continue;
                }
                if !repainted(&format!("{SCOPE}{selector}")) {
                    bad.push(format!("{name}: `{selector}`"));
                }
            }
        }
    }
    assert!(found >= 8, "only {found} ground boxes read");
    assert!(
        bad.is_empty(),
        "ground-coloured boxes the stipple look leaves:\n{}",
        bad.join("\n")
    );
    // The look paints the ground once, on the page root, under the dots.
    for rule in stipple.iter().filter(|r| paints_ground(r)) {
        assert_eq!(rule.selectors, ["html[data-look=\"stipple\"]"]);
    }
}

#[test]
fn the_rule_reader_reads_groups_lists_and_comments() {
    let read = rules(
        "/* c { x:y } */ a, b  c { color:red; font-weight:700 }\n\
         @media (max-width:1px) { d { e:f; } g { h:i } }\n\
         @keyframes k { 0%,100% { o:1 } 50% { o:0 } }\n z { q:r; }",
    );
    let got: Vec<(Vec<String>, usize)> = read
        .iter()
        .map(|r| (r.selectors.clone(), r.declarations.len()))
        .collect();
    let s = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        got,
        [
            (s(&["a", "b c"]), 2),
            (s(&["d"]), 1),
            (s(&["g"]), 1),
            (s(&["z"]), 1)
        ]
    );
    assert!(heavy("700") && heavy("600") && heavy("bold") && !heavy("500") && !heavy("400"));
}
