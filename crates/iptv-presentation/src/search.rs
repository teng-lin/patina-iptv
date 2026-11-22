//! Search interaction and selection of a result.

mod selection;
mod state;

pub use selection::{ResultPolicy, ResultSelection, select_result};
pub use state::{Focus, Phase, ResultId, SearchAction, SearchEffect, SearchState, Tab};
