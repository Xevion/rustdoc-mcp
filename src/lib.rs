pub mod cargo;
pub mod cli;
pub mod doc;
pub mod format;
pub mod handlers;
pub mod types;

pub use doc::DocIndex;
pub use types::{ItemKind, SearchResult, TraitImplInfo};
