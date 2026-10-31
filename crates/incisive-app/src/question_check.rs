//! The Ask page's live Question check and PICO helper (ivory redesign),
//! with no Leptos in it. Advice only: it never blocks Send, and the
//! server's identifier check (Phase 1, `deid`) is unchanged and is what
//! decides. The rules follow the owner's mockup.

use std::sync::LazyLock;

use regex::Regex;

/// The four PICO parts as typed in "Shape with PICO".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pico {
    pub p: String,
    pub i: String,
    pub c: String,
    pub o: String,
}

/// The PICO fields: key, label, placeholder.
pub const PICO_FIELDS: [(&str, &str, &str); 4] = [
    ("P", "Patients", "adults with acute calculous cholecystitis"),
    ("I", "Intervention", "robotic cholecystectomy"),
    ("C", "Comparison", "laparoscopic cholecystectomy"),
    ("O", "Outcomes", "conversion, bile duct injury, cost"),
];

/// What the composed-question box says before P or I is filled.
pub const COMPOSE_HINT: &str = "Fill the four boxes and the question writes itself.";

impl Pico {
    /// The part for a key (`P`, `I`, `C`, `O`).
    pub fn get(&self, k: &str) -> &str {
        match k {
            "P" => &self.p,
            "I" => &self.i,
            "C" => &self.c,
            _ => &self.o,
        }
    }

    /// Whether "Use this question" has something to use (P or I filled).
    pub fn can_compose(&self) -> bool {
        !self.p.trim().is_empty() || !self.i.trim().is_empty()
    }

    /// The composed question, or [`COMPOSE_HINT`].
    pub fn composed(&self) -> String {
        if !self.can_compose() {
            return COMPOSE_HINT.to_string();
        }
        let or = |s: &str, d: &str| {
            let t = s.trim();
            if t.is_empty() {
                d.to_string()
            } else {
                t.to_string()
            }
        };
        format!(
            "In {}, does {} compared with {} improve {}?",
            or(&self.p, "[patients]"),
            or(&self.i, "[intervention]"),
            or(&self.c, "[comparison]"),
            or(&self.o, "[outcomes]")
        )
    }
}

/// A starter question: chip text and its PICO parts.
pub struct Starter {
    pub short: &'static str,
    pub pico: [&'static str; 4],
}

/// The two "Try" chips.
pub static STARTERS: [Starter; 2] = [
    Starter {
        short: "Robotic vs lap TME after neoadjuvant therapy",
        pico: [
            "adults with rectal cancer after neoadjuvant therapy",
            "robotic TME",
            "laparoscopic TME",
            "circumferential margin involvement and conversion",
        ],
    },
    Starter {
        short: "ICG angiography and anastomotic leak",
        pico: [
            "patients undergoing robotic low anterior resection",
            "ICG fluorescence angiography",
            "no ICG assessment",
            "anastomotic leak",
        ],
    },
];

impl Starter {
    pub fn to_pico(&self) -> Pico {
        Pico {
            p: self.pico[0].to_string(),
            i: self.pico[1].to_string(),
            c: self.pico[2].to_string(),
            o: self.pico[3].to_string(),
        }
    }
}

static HAS_P: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"patients?|adults?|children|infants?|elderly|women|men\b|y/o|year[- ]old|\bwith\b")
        .expect("valid")
});
static HAS_C: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bvs\.?\b|versus|compared (?:with|to)|\bthan\b|\bor\b").expect("valid")
});
static HAS_I: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"robotic|laparoscopic|endoscopic|open|icg|therapy|surgery|repair|myotomy|resection|stent|drug|dose")
        .expect("valid")
});
static HAS_O: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"leak|pain|recurrence|complication|mortality|survival|margin|conversion|cost|quality of life|qol|outcome|time|stay|readmission|infection|reflux|function")
        .expect("valid")
});
/// Possible identifiers: a 10-digit number (phone), a full date, MRN /
/// UHID / IP no / hospital id, or Mr/Mrs/Ms and a name.
static ID_HIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{10}\b|\b\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4}\b|\bmrn\b|\buhid\b|\bip ?no\b|hospital id|\b(?:mr|mrs|ms)\.? [a-z]{3,}")
        .expect("valid")
});

/// One line of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckItem {
    pub ok: bool,
    /// A possible identifier (drawn in red with `!`).
    pub warn: bool,
    pub label: &'static str,
    pub hint: String,
}

/// The whole check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub items: Vec<CheckItem>,
    pub score: usize,
    pub tip: &'static str,
    pub identifier: bool,
}

/// Run the check over the question box and the PICO parts.
pub fn check(question: &str, pico: &Pico) -> Check {
    let q = question.to_lowercase();
    let trimmed = question.trim();
    let words = trimmed.split_whitespace().count();
    let set = |s: &str| !s.trim().is_empty();
    let has_p = set(&pico.p) || HAS_P.is_match(&q);
    let has_c = set(&pico.c) || HAS_C.is_match(&q);
    let has_i = set(&pico.i) || HAS_I.is_match(&q);
    let has_o = set(&pico.o) || HAS_O.is_match(&q);
    let id_hit = ID_HIT.is_match(&q);
    let items = vec![
        CheckItem {
            ok: has_p,
            warn: false,
            label: "Patients named",
            hint: "Who \u{2014} e.g. adults after neoadjuvant therapy".into(),
        },
        CheckItem {
            ok: has_i,
            warn: false,
            label: "Intervention named",
            hint: "What you would do \u{2014} e.g. robotic TME".into(),
        },
        CheckItem {
            ok: has_c,
            warn: false,
            label: "Comparator named",
            hint: "Against what \u{2014} e.g. laparoscopic".into(),
        },
        CheckItem {
            ok: has_o,
            warn: false,
            label: "Outcomes that matter",
            hint: "e.g. leak, recurrence, cost".into(),
        },
        CheckItem {
            ok: words >= 10,
            warn: false,
            label: "Specific enough",
            hint: format!(
                "{words} {} \u{b7} aim for 10 or more",
                if words == 1 { "word" } else { "words" }
            ),
        },
        CheckItem {
            ok: !trimmed.is_empty() && !id_hit,
            warn: id_hit,
            label: if id_hit {
                "Possible identifier"
            } else {
                "No identifiers"
            },
            hint: if id_hit {
                "Remove names, phone numbers, IDs or exact dates".into()
            } else {
                "Names, phones, IDs, exact dates".into()
            },
        },
    ];
    let score = items.iter().filter(|c| c.ok).count();
    let tip = if id_hit {
        "Take out the identifier before sending."
    } else if score >= 5 {
        "Well framed. Full depth suits comparative questions like this."
    } else if words == 0 {
        "Sharper questions find better trials. Tap a starter or use Shape with PICO."
    } else {
        "Add what is missing on the list \u{2014} the researcher searches better with all four PICO parts."
    };
    Check {
        items,
        score,
        tip,
        identifier: id_hit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_box_scores_zero_with_the_starter_tip() {
        let c = check("", &Pico::default());
        assert_eq!(c.score, 0);
        assert_eq!(c.items.len(), 6);
        assert!(!c.identifier);
        assert!(c.tip.starts_with("Sharper questions"));
        assert_eq!(c.items[5].label, "No identifiers");
        assert!(!c.items[5].ok);
    }

    #[test]
    fn a_full_pico_question_is_well_framed() {
        let s = STARTERS[0].to_pico();
        let q = s.composed();
        assert_eq!(
            q,
            "In adults with rectal cancer after neoadjuvant therapy, does robotic TME compared with laparoscopic TME improve circumferential margin involvement and conversion?"
        );
        let c = check(&q, &Pico::default());
        assert_eq!(c.score, 6, "{c:?}");
        assert!(c.tip.starts_with("Well framed"));
    }

    #[test]
    fn identifiers_are_flagged_but_only_advised() {
        for q in [
            "Patient Mr. Sharma, robotic LAR leak?",
            "call 9876543210 about the leak",
            "operated on 12/03/2026, leak?",
            "MRN 445 robotic repair pain",
            "UHID 12 pain",
            "ip no 55 pain",
        ] {
            let c = check(q, &Pico::default());
            assert!(c.identifier, "{q}");
            assert!(c.items[5].warn && !c.items[5].ok, "{q}");
            assert_eq!(c.items[5].label, "Possible identifier");
            assert_eq!(c.tip, "Take out the identifier before sending.");
        }
        // Ages, sex and post-op day are fine.
        let c = check(
            "52 y/o M, robotic LAR on post-op day 3: does ICG reduce leak?",
            &Pico::default(),
        );
        assert!(!c.identifier, "{c:?}");
    }

    #[test]
    fn pico_parts_count_and_compose() {
        let p = Pico {
            p: "adults".into(),
            ..Default::default()
        };
        assert!(p.can_compose());
        assert_eq!(
            p.composed(),
            "In adults, does [intervention] compared with [comparison] improve [outcomes]?"
        );
        assert_eq!(Pico::default().composed(), COMPOSE_HINT);
        let c = check("x", &p);
        assert!(c.items[0].ok);
        assert!(!c.items[1].ok);
        assert_eq!(c.items[4].hint, "1 word \u{b7} aim for 10 or more");
        assert!(check("robotic versus lap", &Pico::default()).items[2].ok);
        assert!(check("robotic vs. lap", &Pico::default()).items[2].ok);
    }
}
