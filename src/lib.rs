pub mod cargo;
pub mod cli;
pub mod handlers;
pub mod doc;
pub mod types;

pub use doc::DocIndex;
pub use types::{ItemKind, SearchResult, TraitImplInfo};
