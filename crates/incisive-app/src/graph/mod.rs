//! The knowledge graph (off by default; `GRAPH=on`): after a run finishes,
//! one background call picks its concepts and the run is linked to them and
//! to its ranked papers (migration 0017).

pub mod extract;
pub mod job;
pub mod layout;
pub mod organs;
pub mod rank;
pub mod read;
pub mod settings;
pub mod store;
