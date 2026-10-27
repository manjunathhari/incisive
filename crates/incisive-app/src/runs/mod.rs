//! Evidence-agent runs: the view types shared by server and browser, plus the
//! server-only Postgres store.
//!
//! The types here compile without `ssr` (the browser deserializes them), so
//! they depend only on serde, serde_json, chrono and uuid.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "ssr")]
pub mod budget;
#[cfg(feature = "ssr")]
pub mod store;
#[cfg(feature = "ssr")]
pub mod worker;

/// How deep the run goes. Focused unless the user picks Quick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Quick,
    #[default]
    Focused,
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
}

impl DepthSettings {
    pub const QUICK: DepthSettings = DepthSettings {
        refinements: 1,
        screen_n: 20,
        use_n: 5,
        full_text: false,
        limit_paise: 500,
        time_limit_s: 300,
    };
    pub const FOCUSED: DepthSettings = DepthSettings {
        refinements: 3,
        screen_n: 100,
        use_n: 10,
        full_text: true,
        limit_paise: 4000,
        time_limit_s: 300,
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
}
