//! Iterator patterns for traversing documentation items with proper re-export handling.

use crate::item::ItemRef;
use rustdoc_types::{Id, Item, ItemEnum, ItemKind, Type, Use};
use std::collections::hash_map::Values;

/// Iterator for methods defined in impl blocks
pub struct MethodIterator<'a> {
    item: ItemRef<'a, Item>,
    impl_block_iter: InherentImplIterator<'a>,
    current_impl: Option<ItemRef<'a, Item>>,
    current_index: usize,
}

impl<'a> MethodIterator<'a> {
    pub fn new(item: ItemRef<'a, Item>) -> Self {
        let impl_block_iter = InherentImplIterator::new(item);
        Self {
            item,
            impl_block_iter,
            current_impl: None,
            current_index: 0,
        }
    }
}

impl<'a> Iterator for MethodIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Try to get next item from current impl block
            if let Some(current_impl) = self.current_impl
                && let ItemEnum::Impl(impl_block) = current_impl.inner()
                && self.current_index < impl_block.items.len()
            {
                let id = &impl_block.items[self.current_index];
                self.current_index += 1;
                if let Some(item) = self.item.get(id) {
                    return Some(item);
                }
                continue;
            }

            // Move to next impl block
            self.current_impl = self.impl_block_iter.next();
            self.current_index = 0;

            self.current_impl?;
        }
    }
}

/// Kind of impl block to iterate
#[derive(Copy, Clone)]
enum ImplKind {
    /// Trait implementations (impl Trait for Type)
    Trait,
    /// Inherent implementations (impl Type)
    Inherent,
}

/// Generic iterator for impl blocks (both trait and inherent)
struct ImplIterator<'a> {
    item: ItemRef<'a, Item>,
    item_iter: Values<'a, Id, Item>,
    kind: ImplKind,
}

impl<'a> ImplIterator<'a> {
    fn new(item: ItemRef<'a, Item>, kind: ImplKind) -> Self {
        let item_iter = item.crate_index().index.values();
        Self {
            item,
            item_iter,
            kind,
        }
    }
}

impl<'a> Iterator for ImplIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        for item in &mut self.item_iter {
            if let ItemEnum::Impl(impl_block) = &item.inner
                && let Type::ResolvedPath(path) = &impl_block.for_
                && path.id == self.item.id
            {
                let matches = match self.kind {
                    ImplKind::Trait => impl_block.trait_.is_some(),
                    ImplKind::Inherent => impl_block.trait_.is_none(),
                };

                if matches {
                    return Some(self.item.build_ref(item));
                }
            }
        }
        None
    }
}

/// Iterator for trait implementations
pub struct TraitIterator<'a>(ImplIterator<'a>);

impl<'a> TraitIterator<'a> {
    pub fn new(item: ItemRef<'a, Item>) -> Self {
        Self(ImplIterator::new(item, ImplKind::Trait))
    }
}

impl<'a> Iterator for TraitIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// Iterator over a collection of Item Ids with re-export resolution
pub struct IdIterator<'a, T> {
    item: ItemRef<'a, T>,
    id_iter: std::slice::Iter<'a, Id>,
    // Stack of pending glob expansions to avoid nested Box allocations
    glob_stack: Vec<std::slice::Iter<'a, Id>>,
    include_use: bool,
}

impl<'a, T> IdIterator<'a, T> {
    pub fn new(item: ItemRef<'a, T>, ids: &'a [Id]) -> Self {
        Self {
            item,
            id_iter: ids.iter(),
            glob_stack: Vec::new(),
            include_use: false,
        }
    }

    /// Configure whether to include Use items in the iteration
    pub fn with_include_use(mut self, include_use: bool) -> Self {
        self.include_use = include_use;
        self
    }
}

impl<'a, T> Iterator for IdIterator<'a, T> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        'outer: loop {
            // Process current ID iterator
            while let Some(id) = self.id_iter.next() {
                let Some(item) = self.item.get(id) else {
                    continue;
                };

                // Handle re-exports
                if let ItemEnum::Use(use_item) = item.inner() {
                    if self.include_use {
                        return Some(item);
                    }

                    // Resolve the re-export target
                    let mut source_item = use_item
                        .id
                        .and_then(|id| item.crate_index().get(item.query(), &id))
                        .or_else(|| item.query().resolve_path(&use_item.source, &mut vec![]))?;

                    // Handle glob imports
                    if use_item.is_glob {
                        let glob_ids = match source_item.inner() {
                            ItemEnum::Module(module) => Some(&module.items),
                            ItemEnum::Enum(enum_item) => Some(&enum_item.variants),
                            _ => None,
                        };

                        if let Some(ids) = glob_ids {
                            // Push current iterator to stack and start processing glob
                            self.glob_stack
                                .push(std::mem::replace(&mut self.id_iter, ids.iter()));
                            continue 'outer;
                        }
                        // If glob expansion failed, continue with next item
                    } else {
                        // Apply custom name to the resolved item
                        source_item.set_name(&use_item.name);
                        return Some(source_item);
                    }
                } else {
                    return Some(item);
                }
            }

            // Current iterator exhausted, pop from glob stack if available
            if let Some(prev_iter) = self.glob_stack.pop() {
                self.id_iter = prev_iter;
            } else {
                // No more iterators to process
                return None;
            }
        }
    }
}

/// Iterator for inherent impl blocks (non-trait impls)
pub struct InherentImplIterator<'a>(ImplIterator<'a>);

impl<'a> InherentImplIterator<'a> {
    pub fn new(item: ItemRef<'a, Item>) -> Self {
        Self(ImplIterator::new(item, ImplKind::Inherent))
    }
}

impl<'a> Iterator for InherentImplIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// Iterator for re-export (use) items with glob expansion support
pub struct UseIterator<'a> {
    use_item: Option<ItemRef<'a, Use>>,
    resolved_iter: Option<IdIterator<'a, Item>>,
    include_use: bool,
}

impl<'a> UseIterator<'a> {
    pub fn new(use_item: ItemRef<'a, Use>, include_use: bool) -> Self {
        Self {
            use_item: Some(use_item),
            resolved_iter: None,
            include_use,
        }
    }
}

impl<'a> Iterator for UseIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // First check if we have a glob expansion in progress
            if let Some(resolved_iter) = &mut self.resolved_iter {
                if let Some(item) = resolved_iter.next() {
                    return Some(item);
                } else {
                    // Exhausted the glob expansion
                    self.resolved_iter = None;
                    return None;
                }
            }

            // Process the use item
            let use_ref = self.use_item.take()?;

            // Extract references we need before moving use_ref
            let name = use_ref.name();
            let is_glob = use_ref.is_glob;

            // Resolve the re-export target
            let mut resolved_item = use_ref
                .id
                .and_then(|id| use_ref.get(&id))
                .or_else(|| use_ref.query().resolve_path(&use_ref.source, &mut vec![]))?;

            // Apply the re-export name
            resolved_item.set_name(name);

            // Handle glob imports
            if is_glob {
                match resolved_item.inner() {
                    ItemEnum::Module(module) => {
                        self.resolved_iter = Some(
                            resolved_item
                                .id_iter(&module.items)
                                .with_include_use(self.include_use),
                        );
                    }
                    ItemEnum::Enum(enum_item) => {
                        self.resolved_iter = Some(
                            resolved_item
                                .id_iter(&enum_item.variants)
                                .with_include_use(self.include_use),
                        );
                    }
                    _ => {
                        return None;
                    }
                }
                // Continue loop to process the glob expansion
            } else if let ItemEnum::Use(ui) = resolved_item.inner()
                && !self.include_use
            {
                // Chain through another use item
                self.use_item = Some(resolved_item.build_ref(ui));
                // Continue loop to resolve the chained use
            } else {
                return Some(resolved_item);
            }
        }
    }
}

/// Builder for constructing filtered child iterators with fluent API.
pub struct ChildrenBuilder<'a> {
    item: ItemRef<'a, Item>,
    include_use: bool,
    kind_filter: Option<ItemKind>,
}

impl<'a> ChildrenBuilder<'a> {
    /// Create a new children builder for an item.
    pub fn new(item: ItemRef<'a, Item>) -> Self {
        Self {
            item,
            include_use: false,
            kind_filter: None,
        }
    }

    /// Include Use (re-export) items in the iteration.
    pub fn include_use(mut self) -> Self {
        self.include_use = true;
        self
    }

    /// Filter children to only include items of a specific kind.
    pub fn only_kind(mut self, kind: ItemKind) -> Self {
        self.kind_filter = Some(kind);
        self
    }

    /// Build and return the configured child iterator.
    pub fn build(self) -> ChildIterator<'a> {
        let mut iterator = ChildIterator::new(self.item);
        if self.include_use {
            iterator = iterator.with_use();
        }
        // TODO: Apply kind_filter if needed
        iterator
    }
}

/// Enum for iterating over different types of child items
pub enum ChildIterator<'a> {
    /// Methods from impl blocks
    AssociatedMethods(MethodIterator<'a>),
    /// Module items
    Module(IdIterator<'a, Item>),
    /// Re-export with optional glob expansion
    Use(UseIterator<'a>),
    /// Enum variants and methods
    Enum(IdIterator<'a, Item>, MethodIterator<'a>),
    /// No children
    None,
}

impl<'a> Iterator for ChildIterator<'a> {
    type Item = ItemRef<'a, Item>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            ChildIterator::AssociatedMethods(method_iter) => method_iter.next(),
            ChildIterator::Module(id_iter) => id_iter.next(),
            ChildIterator::Enum(id_iter, method_iter) => {
                id_iter.next().or_else(|| method_iter.next())
            }
            ChildIterator::Use(use_iter) => use_iter.next(),
            ChildIterator::None => None,
        }
    }
}

impl<'a> ChildIterator<'a> {
    /// Create an iterator for the children of an item
    pub fn new(item: ItemRef<'a, Item>) -> Self {
        match item.inner() {
            ItemEnum::Module(module) => Self::Module(item.id_iter(&module.items)),
            ItemEnum::Enum(enum_item) => {
                Self::Enum(item.id_iter(&enum_item.variants), item.methods())
            }
            ItemEnum::Struct(_) => Self::AssociatedMethods(item.methods()),
            ItemEnum::Union(_) => Self::AssociatedMethods(item.methods()),
            ItemEnum::Use(use_item) => {
                ChildIterator::Use(UseIterator::new(item.build_ref(use_item), false))
            }
            ItemEnum::Trait(_) => Self::AssociatedMethods(item.methods()),
            _ => Self::None,
        }
    }

    /// Configure whether to include Use items in the iteration
    pub fn with_use(mut self) -> Self {
        match &mut self {
            ChildIterator::AssociatedMethods(_) => {}
            ChildIterator::Module(id_iter) => {
                *id_iter = std::mem::replace(id_iter, IdIterator::new(id_iter.item, &[]))
                    .with_include_use(true);
            }
            ChildIterator::Enum(id_iter, _) => {
                *id_iter = std::mem::replace(id_iter, IdIterator::new(id_iter.item, &[]))
                    .with_include_use(true);
            }
            ChildIterator::Use(use_iter) => {
                use_iter.include_use = true;
            }
            ChildIterator::None => {}
        }
        self
    }
}

/// Extension methods for ItemRef to access iterators
impl<'a> ItemRef<'a, Item> {
    /// Get an iterator over methods defined in impl blocks
    pub fn methods(&self) -> MethodIterator<'a> {
        MethodIterator::new(*self)
    }

    /// Get an iterator over trait implementations
    pub fn traits(&self) -> TraitIterator<'a> {
        TraitIterator::new(*self)
    }

    /// Get a builder for configuring child item iteration.
    pub fn children(&self) -> ChildrenBuilder<'a> {
        ChildrenBuilder::new(*self)
    }
}

/// Extension methods for ItemRef to create Id iterators
impl<'a, T> ItemRef<'a, T> {
    /// Create an iterator over a collection of item IDs
    pub fn id_iter(&self, ids: &'a [Id]) -> IdIterator<'a, T> {
        IdIterator::new(*self, ids)
    }
}
