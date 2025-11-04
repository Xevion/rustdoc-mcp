# cargo-doc-mcp Plan

## Purpose

An MCP server that provides intelligent access to Rust documentation for AI assistants. Enables natural language queries about types, functions, traits, and modules by efficiently querying rustdoc JSON data.

**Design for AI assistants first**: Unified tools with flexible parameters, fuzzy matching, session context, and smart suggestions.

## Design Principles

- **AI-First UX**: Unified tools with parameters instead of many specialized tools
- **Session Context**: Persistent working directory across requests
- **Lazy & Smart**: Only load crates when needed, cache intelligently
- **Path Resolution**: Handle `crate::module::Type` and cross-crate references
- **Fuzzy Everything**: AI doesn't need exact names, provide helpful suggestions
- **Fast Search**: Persistent TF-IDF indexing with smart tokenization
- **Production Ready**: Excellent error messages, comprehensive documentation

## Architecture

### Three-Tier Caching Strategy

```
Server Context (persistent across requests)
  ↓ working directory, project metadata
Request Scope (per-MCP-call)
  ↓ loaded crates cache, auto-cleanup on drop
Lazy Loading
  ↓ only load crates when accessed
```

**Why this matters**: Loading all dependencies eagerly is slow and memory-intensive. This architecture loads crates on-demand and cleans them up after each request.

### DocRef Pattern

Zero-copy references that bundle:

- Item reference (`&Item`)
- Crate docs reference (`&RustdocData`)
- Request reference (for cross-crate lookups)
- Custom name override (for `use` statements)

Implements `Copy` for efficiency. Enables clean traversal APIs.

### Path Resolution System

Handles:

- **Module paths**: `crate::module::submodule::Type`
- **Cross-crate**: `serde_json::Value`, `std::vec::Vec`
- **Use statements**: Follows `pub use` declarations
- **Glob imports**: Resolves `use module::*`
- **Fuzzy matching**: Suggests corrections on typos

### Iterator Abstractions

Custom iterators hide complexity:

- `methods()` - all methods (inherent + trait impls)
- `traits()` - all trait implementations
- `child_items()` - recursive module/enum/struct traversal
- Automatic `use` statement resolution
- Handles re-exports transparently

## Core Tools

### 1. set_workspace

Establish session context for subsequent queries.

**Input**: Path to Rust project root
**Output**: Confirmation, discovered workspace info

**Enables**: Using "crate" to refer to project crate, auto-discovery of dependencies

### 2. inspect_item

Unified tool for inspecting types, modules, functions, and traits.

**Input**:

- `name` (string): Path like "crate::MyStruct", "serde_json::Value", or "std::vec::Vec"
- `recursive` (bool, optional): Recursively list module contents
- `filter` (array, optional): Filter by kind: `["struct", "enum", "function", "trait", ...]`
- `include_source` (bool, optional): Include source code snippets
- `verbosity` (enum, optional): "minimal" | "brief" | "full"

**Output**: Context-aware based on item type:

- **Type (struct/enum)**: Fields/variants, visibility, documentation, methods
- **Function**: Signature with generics, parameters, return type, docs
- **Module**: List of public items (filtered if requested)
- **Trait**: Required methods, associated types

**Examples**:

```
inspect_item("crate::MyStruct", include_source: true)
inspect_item("crate::module", recursive: true, filter: ["struct", "enum"])
inspect_item("std::vec::Vec", verbosity: "full")
```

**Why unified?**: AI assistants are better at selecting parameters than choosing between 6 different tools. One flexible tool reduces cognitive load.

### 3. list_crates

Discover crates in the workspace and dependencies.

**Input**:

- `workspace_member` (string, optional): Scope to specific workspace member

**Output**: List of available crate names with versions

**Use case**: Help AI discover what crates are available before querying them

### 4. search_docs

Full-text search with relevance ranking across names and documentation.

**Input**:

- `crate_name` (string): Crate to search in (use "crate" for project crate)
- `query` (string): Search terms (multi-term queries combine scores)
- `limit` (int, optional): Max results (default: 10)

**Output**: Ranked results with paths, types, and documentation snippets

**Features**:

- Persistent TF-IDF index (cached on disk, invalidated on crate changes)
- Smart tokenization: handles `CamelCase`, `snake_case`, `kebab-case`
- Relevance cutoff: Only shows meaningful results
- Searches both item names and documentation

## Key Implementation Details

### Fuzzy Matching

- **Algorithm**: Jaro-Winkler distance (weights prefix matches)
- **Threshold**: 0.8 minimum for suggestions
- **Top-N**: Show up to 5 suggestions on failed lookups
- **Crate name normalization**: `foo-bar` ↔ `foo_bar`

### Search Indexing

```rust
// Smart tokenization examples:
"CamelCase" → ["Camel", "Case", "CamelCase"]
"snake_case" → ["snake", "case", "snake_case"]
"kebab-case" → ["kebab", "case", "kebab-case"]
"iterators" → ["iterator", "iterators"]  // plural handling

// TF-IDF scoring:
- Term frequency × Inverse document frequency
- Multi-term queries: additive scoring
- Name matches: 2x weight vs documentation
```

**Persistence**: Index stored as bincode, includes mtime for invalidation

### Workspace Discovery

- Uses `cargo_metadata` to find all dependencies
- Discovers workspace members automatically
- Loads standard library from nightly rustup component
- Parallel loading where beneficial

### Use Statement Resolution

Transparently handles:

```rust
pub use module::Type;           // Simple re-export
pub use module::*;              // Glob import (expands automatically)
pub use external_crate::Type;   // Cross-crate re-export
```

## Standard Library Support

The Rust standard library documentation is available as JSON through a rustup component:

**Installation:**

```bash
rustup component add --toolchain nightly rust-docs-json
```

**Location:**

```bash
$(rustup run nightly rustc --print sysroot)/share/doc/rust/json/
```

**Available crates**: `std`, `core`, `alloc`, `proc_macro`, `test`

Auto-discovered when available, gracefully skipped if missing.

## Error Handling & UX

### Excellent Error Messages

- **Path not found**: Show fuzzy suggestions with types
- **Ambiguous match**: Show all options with disambiguation hints
- **Crate not loaded**: Suggest running in project directory
- **Invalid filter**: Show valid filter options

### Verbosity Levels

- **Minimal**: Structure only (types, signatures, no docs)
- **Brief** (default): Truncated docs with "..." hint for more
- **Full**: Complete documentation without truncation

Helps AI balance context window vs information needs.

## Future Enhancements

### High Priority

- **CLI mode**: Interactive REPL for human exploration
- **Trait method lookup**: "What traits provide method X?"
- **Type constructor analysis**: Find `new()`, `default()`, builders
- **Signature search**: Find functions by return type

### Medium Priority

- **Dependency graph**: Visualize crate relationships
- **Doc coverage**: Analyze missing documentation
- **Link validation**: Check doc links are valid
- **Custom output formats**: JSON, Markdown, HTML

### Research Needed

- **Feature mapping**: Show which Cargo features gate items (rustdoc JSON limitations)
- **Macro documentation**: Macro syntax patterns (not in rustdoc JSON)
- **Type unification**: Advanced signature matching (complex)

## Why This Project?

**Compared to existing solutions:**

1. **Better Documentation**: Comprehensive examples, clear architecture explanations
2. **AI-First Design**: Unified tools optimized for LLM decision-making
3. **Reliability Focus**: Excellent error messages, graceful degradation
4. **Performance**: Smart caching prevents memory bloat on large projects
5. **Actually Works**: Tested on real-world projects, handles edge cases

**Core Value**: Make Rust documentation accessible to AI assistants without requiring them to understand rustdoc JSON complexity or cargo/rustup internals.
