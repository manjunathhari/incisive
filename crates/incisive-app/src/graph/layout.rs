//! Dot sizes for the map (the positions are `graph_view::layout`).

use crate::graph_view::DotKind;

/// A concept or run dot's radius; a treatment in more than one run is
/// bigger. Papers use [`paper_radius`].
pub fn concept_radius(kind: DotKind, runs: usize) -> f32 {
    match kind {
        DotKind::Disease => 15.0,
        DotKind::Organ => 12.0,
        DotKind::Treatment if runs > 1 => 14.0,
        DotKind::Treatment => 11.0,
        DotKind::Run => 9.0,
        DotKind::Outcome => 8.0,
        DotKind::Paper => 4.5,
    }
}

/// A paper's radius from its rank code (`rct`, `meta`, ...).
pub fn paper_radius(rank: &str) -> f32 {
    match rank {
        "rct" => 8.0,
        "meta" => 6.5,
        "prospective" => 5.5,
        "retrospective" => 5.0,
        _ => 4.5,
    }
}
