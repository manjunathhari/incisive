//! Evidence-agent runs: the view types shared by server and browser, plus the
//! server-only Postgres store.
//!
//! The types here compile without `ssr` (the browser deserializes them), so
//! they depend only on serde, serde_json, chrono and uuid.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "ssr")]
pub mod api;
#[cfg(feature = "ssr")]
pub mod budget;
#[cfg(feature = "ssr")]
pub mod store;
#[cfg(feature = "ssr")]
pub mod worker;

/// Shown for a missing run, or one that belongs to someone else. Shared with
/// the browser, which stops polling when it sees it.
pub const RUN_NOT_FOUND_MESSAGE: &str = "That question wasn't found. It may have been deleted.";

/// How deep the run goes. Focused unless the user picks Quick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Quick,
    #[default]
    Focused,
}

/// Which engine answers Quick questions (`QUICK_ENGINE`, Phase 3 Stage 2):
/// the evidence team (researcher → writer, the default) or the Phase 2
/// pipeline (`classic`, the owner's rollback switch). Focused always runs
/// the Phase 2 pipeline until Stage 3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickEngine {
    #[default]
    Team,
    Classic,
}

impl QuickEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            QuickEngine::Team => "team",
            QuickEngine::Classic => "classic",
        }
    }

    /// Exactly `team` or `classic` (lower case).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "team" => Some(QuickEngine::Team),
            "classic" => Some(QuickEngine::Classic),
            _ => None,
        }
    }

    /// Whether a run of `mode` is answered by the evidence team.
    pub fn uses_team(self, mode: RunMode) -> bool {
        mode == RunMode::Quick && self == QuickEngine::Team
    }
}

/// The limits one depth runs under (owner-editable through `RUN_LIMITS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepthSettings {
    /// Query refinements allowed inside Search.
    pub refinements: u8,
    /// Records screened (top-ranked by database relevance).
    pub screen_n: u16,
    /// Papers used for extraction.
    pub use_n: u8,
    /// Whether the Retrieve step fetches free full text.
    pub full_text: bool,
    /// Cost limit in paise, reserved in full when the run starts.
    pub limit_paise: i64,
    /// Time limit in seconds (graceful finish works against it).
    pub time_limit_s: u32,

    // ----- Phase 3 evidence team (T3) -----
    //
    // Additive: the Phase 2 pipeline reads none of these, and keeps
    // `limit_paise` / `time_limit_s` above until Stage 2 switches the run
    // over. `focused` carries Phase 3's **Full** mode (spec §5); `quick`
    // its Quick mode. The global, daily and monthly caps are unchanged.
    /// Phase 3 cost limit in paise (Quick ₹60, Full ₹200).
    pub agent_limit_paise: i64,
    /// Phase 3 time limit in seconds (Quick 4 min, Full 8 min).
    pub agent_time_limit_s: u32,
    /// Researcher tool-use turns at most (Quick 8, Full 15).
    pub researcher_turns: u8,
    /// Skeptic tool-use turns at most (Full 8; 0: no skeptic, Quick).
    pub skeptic_turns: u8,
    /// `max_tokens` per researcher turn (and its final call).
    pub researcher_max_tokens: u32,
    /// `max_tokens` per skeptic turn (and its final call).
    pub skeptic_max_tokens: u32,
    /// `max_tokens` per extractor + appraiser call.
    pub extractor_max_tokens: u32,
    /// `max_tokens` of the writer's call.
    pub writer_max_tokens: u32,
}

/// Most tool-use turns a role may be given (`RUN_LIMITS`).
pub const MAX_AGENT_TURNS: u8 = 50;
/// Largest `max_tokens` a role may be given (`RUN_LIMITS`).
pub const MAX_ROLE_TOKENS: u32 = 64_000;

/// Phase 3 per-call `max_tokens` defaults. A turn's hold reserves all of
/// it (thinking counts toward it), so these are kept modest: at Opus-class
/// prices a 4,000-token turn holds about ₹9 of output before input.
const AGENT_MAX_TOKENS: u32 = 4_000;
const WRITER_MAX_TOKENS: u32 = 8_000;

impl DepthSettings {
    pub const QUICK: DepthSettings = DepthSettings {
        refinements: 1,
        screen_n: 20,
        use_n: 5,
        full_text: false,
        // ₹15 (owner ruling, 28 Oct 2026; was ₹5): Quick must leave the
        // finish reserve for Write and Check (Ruling R-13c).
        limit_paise: 1500,
        time_limit_s: 300,
        agent_limit_paise: 6000,
        agent_time_limit_s: 240,
        researcher_turns: 8,
        skeptic_turns: 0,
        researcher_max_tokens: AGENT_MAX_TOKENS,
        skeptic_max_tokens: AGENT_MAX_TOKENS,
        extractor_max_tokens: AGENT_MAX_TOKENS,
        writer_max_tokens: WRITER_MAX_TOKENS,
    };
    pub const FOCUSED: DepthSettings = DepthSettings {
        refinements: 3,
        screen_n: 100,
        use_n: 10,
        full_text: true,
        limit_paise: 4000,
        time_limit_s: 300,
        agent_limit_paise: 20000,
        agent_time_limit_s: 480,
        researcher_turns: 15,
        skeptic_turns: 8,
        researcher_max_tokens: AGENT_MAX_TOKENS,
        skeptic_max_tokens: AGENT_MAX_TOKENS,
        extractor_max_tokens: AGENT_MAX_TOKENS,
        writer_max_tokens: WRITER_MAX_TOKENS,
    };

    fn validate(&self) -> Result<(), String> {
        if self.limit_paise <= 0 {
            return Err("limit_paise must be a positive number of paise".into());
        }
        if self.time_limit_s == 0 {
            return Err("time_limit_s must be positive".into());
        }
        if self.screen_n == 0 {
            return Err("screen_n must be positive".into());
        }
        if self.use_n == 0 {
            return Err("use_n must be positive".into());
        }
        if self.agent_limit_paise <= 0 {
            return Err("agent_limit_paise must be a positive number of paise".into());
        }
        if self.agent_time_limit_s == 0 {
            return Err("agent_time_limit_s must be positive".into());
        }
        if !(1..=MAX_AGENT_TURNS).contains(&self.researcher_turns) {
            return Err(format!("researcher_turns must be 1 to {MAX_AGENT_TURNS}"));
        }
        if self.skeptic_turns > MAX_AGENT_TURNS {
            return Err(format!("skeptic_turns must be 0 to {MAX_AGENT_TURNS}"));
        }
        for (name, v) in [
            ("researcher_max_tokens", self.researcher_max_tokens),
            ("skeptic_max_tokens", self.skeptic_max_tokens),
            ("extractor_max_tokens", self.extractor_max_tokens),
            ("writer_max_tokens", self.writer_max_tokens),
        ] {
            if !(1..=MAX_ROLE_TOKENS).contains(&v) {
                return Err(format!("{name} must be 1 to {MAX_ROLE_TOKENS}"));
            }
        }
        Ok(())
    }
}

/// Depth settings for both modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunsConfig {
    pub quick: DepthSettings,
    pub focused: DepthSettings,
}

impl Default for RunsConfig {
    fn default() -> Self {
        RunsConfig {
            quick: DepthSettings::QUICK,
            focused: DepthSettings::FOCUSED,
        }
    }
}

impl RunsConfig {
    pub fn for_mode(&self, mode: RunMode) -> DepthSettings {
        match mode {
            RunMode::Quick => self.quick,
            RunMode::Focused => self.focused,
        }
    }

    /// The cost limit (paise, reserved in full when the run starts) and the
    /// time limit (seconds) of a run of `mode` under `engine`: the Phase 3
    /// limits (`agent_limit_paise`, `agent_time_limit_s`) when the evidence
    /// team answers it, the Phase 2 ones otherwise.
    pub fn limits(&self, mode: RunMode, engine: QuickEngine) -> (i64, u32) {
        let s = self.for_mode(mode);
        if engine.uses_team(mode) {
            (s.agent_limit_paise, s.agent_time_limit_s)
        } else {
            (s.limit_paise, s.time_limit_s)
        }
    }

    /// Parse `RUN_LIMITS`: `{"quick": {...}, "focused": {...}}`, either key
    /// optional, each a partial [`DepthSettings`] over the defaults. Unknown
    /// keys and out-of-range values are errors. The error text is the reason only.
    pub fn from_json(json: &str) -> Result<RunsConfig, String> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("not valid JSON ({e})"))?;
        let obj = v
            .as_object()
            .ok_or("expected a JSON object with `quick` and/or `focused`")?;
        let mut cfg = RunsConfig::default();
        for (mode, patch) in obj {
            let slot = match mode.as_str() {
                "quick" => &mut cfg.quick,
                "focused" => &mut cfg.focused,
                other => {
                    return Err(format!(
                        "unknown mode `{other}` (expected `quick` or `focused`)"
                    ));
                }
            };
            let patch = patch
                .as_object()
                .ok_or_else(|| format!("`{mode}` must be an object"))?;
            let mut merged = match serde_json::to_value(*slot) {
                Ok(serde_json::Value::Object(m)) => m,
                _ => return Err("internal: settings did not serialize".into()),
            };
            for (k, val) in patch {
                merged.insert(k.clone(), val.clone());
            }
            let parsed: DepthSettings = serde_json::from_value(serde_json::Value::Object(merged))
                .map_err(|e| format!("`{mode}`: {e}"))?;
            parsed.validate().map_err(|e| format!("`{mode}`: {e}"))?;
            *slot = parsed;
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Done,
    Partial,
    Failed,
    Interrupted,
}

/// Pipeline step a progress line belongs to (`Note` is a free-form aside).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Frame,
    Search,
    Screen,
    Retrieve,
    Extract,
    Write,
    Check,
    Present,
    Note,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Quick => "quick",
            RunMode::Focused => "focused",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "quick" => Some(RunMode::Quick),
            "focused" => Some(RunMode::Focused),
            _ => None,
        }
    }
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Done => "done",
            RunStatus::Partial => "partial",
            RunStatus::Failed => "failed",
            RunStatus::Interrupted => "interrupted",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(RunStatus::Queued),
            "running" => Some(RunStatus::Running),
            "done" => Some(RunStatus::Done),
            "partial" => Some(RunStatus::Partial),
            "failed" => Some(RunStatus::Failed),
            "interrupted" => Some(RunStatus::Interrupted),
            _ => None,
        }
    }
    /// True while the run still occupies the user's single active slot.
    pub fn is_active(self) -> bool {
        matches!(self, RunStatus::Queued | RunStatus::Running)
    }
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Frame => "frame",
            StepKind::Search => "search",
            StepKind::Screen => "screen",
            StepKind::Retrieve => "retrieve",
            StepKind::Extract => "extract",
            StepKind::Write => "write",
            StepKind::Check => "check",
            StepKind::Present => "present",
            StepKind::Note => "note",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "frame" => Some(StepKind::Frame),
            "search" => Some(StepKind::Search),
            "screen" => Some(StepKind::Screen),
            "retrieve" => Some(StepKind::Retrieve),
            "extract" => Some(StepKind::Extract),
            "write" => Some(StepKind::Write),
            "check" => Some(StepKind::Check),
            "present" => Some(StepKind::Present),
            "note" => Some(StepKind::Note),
            _ => None,
        }
    }
}

/// One row of the history list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: Uuid,
    pub mode: RunMode,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub cost_paise: i64,
    /// First 80 characters (not bytes) of the question.
    pub title: String,
}

/// A progress line shown while a run works.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepLine {
    pub seq: i32,
    pub at: DateTime<Utc>,
    pub kind: StepKind,
    pub line: String,
}

/// A run with its progress lines and (once written) its structured answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub summary: RunSummary,
    pub steps: Vec<StepLine>,
    pub answer: Option<serde_json::Value>,
    /// `run_answers.answer_version`: 2 for a Phase 2 `Answer`, 3 for a
    /// Phase 3 `AnswerV3` (2 when there is no answer).
    #[serde(default = "phase2_version")]
    pub answer_version: i16,
}

fn phase2_version() -> i16 {
    2
}
