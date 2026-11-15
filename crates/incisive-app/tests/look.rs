//! The look's colour rule: the page chrome's styles (`CHROME_STYLES`) take
//! every colour from a variable, so a look (and a night mode later) restates
//! the variables and nothing else. The ivory values are the ones the styles
//! held as literals before the move.

use std::collections::BTreeSet;

use incisive_app::app::{CHROME_STYLES, IVORY_STYLE};
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

#[test]
fn no_variable_is_used_without_being_declared() {
    // What ivory declares: every style but the stipple one.
    let ivory: BTreeSet<&str> = CHROME_STYLES
        .iter()
        .filter(|(n, _)| *n != "STIPPLE_STYLE")
        .flat_map(|(_, css)| definitions(css))
        .map(|(n, _)| n)
        .collect();
    let stipple: BTreeSet<&str> = definitions(style("STIPPLE_STYLE"))
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let mut bad = Vec::new();
    for (name, css) in CHROME_STYLES {
        for var in used(css) {
            let v = var.as_str();
            // The stipple style may also read what it declares itself.
            let declared = ivory.contains(v)
                || SET_ELSEWHERE.contains(&v)
                || (*name == "STIPPLE_STYLE" && stipple.contains(v));
            if !declared {
                bad.push(format!("{name}: {var}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "variables read but never declared:\n{}",
        bad.join("\n")
    );
    for v in SET_ELSEWHERE {
        assert!(
            !ivory.contains(v),
            "{v} is declared now: take it off the list"
        );
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
