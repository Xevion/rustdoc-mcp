//! Item references and iterators for traversing documentation.

pub mod item_ref;
pub mod iterator;

pub use item_ref::{ItemPath, ItemRef};
pub use iterator::{
    ChildIterator, IdIterator, InherentImplIterator, MethodIterator, TraitIterator,
};
