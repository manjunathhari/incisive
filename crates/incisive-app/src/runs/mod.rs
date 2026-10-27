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

/// How deep the run goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Quick,
    Focused,
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
