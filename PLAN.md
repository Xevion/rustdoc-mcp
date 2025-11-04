# cargo-doc-mcp Plan

## Purpose

A CLI tool (future MCP server) that provides structured access to Rust documentation for AI assistants. Queries rustdoc JSON to answer concrete questions about types, functions, traits, and modules.

## Design Principles

- **Accuracy**: Only show public, documented items
- **Completeness**: Don't miss methods, impls, or paths
- **Fuzzy matching**: AI doesn't need exact names
- **Speed**: Parallel loading, efficient indexing
- **Structured output**: Parseable, factual data

## Core Tools

### 1. get_type_definition
Show the structure of types (structs, enums, unions).

**Input**: Type name (fuzzy)
**Output**: Fields/variants with types, visibility, documentation

### 2. list_methods
List all methods available on a type (inherent + trait methods).

**Input**: Type name (fuzzy)
**Output**: Method signatures grouped by source (inherent, trait impls)

### 3. list_trait_impls
What traits does this type implement?

**Input**: Type name (fuzzy)
**Output**: List of implemented traits (local crate + visible from deps)

### 4. get_function_signature
Get detailed signature for functions.

**Input**: Function name (fuzzy)
**Output**: Full signature with generics, bounds, parameters, return type, docs

### 5. list_module_contents
List everything exported from a module.

**Input**: Module path (fuzzy)
**Output**: All public items grouped by kind (types, functions, traits, constants, etc.)

### 6. get_generic_bounds
Show generic constraints for a type or function.

**Input**: Item name (fuzzy)
**Output**: Type parameters, trait bounds, where clauses

## Implementation Notes

- Fuzzy search: case-insensitive, substring matching, relevance scoring
- Privacy filtering: Only show `pub` items, no internal paths
- Multi-crate support: Search across dependencies in parallel
- Result limits: Default to reasonable counts, allow override

## Standard Library Documentation

The Rust standard library documentation is available as JSON through a special rustup component:

**Installation:**
```bash
rustup component add --toolchain nightly rust-docs-json
```

**Location:**
The JSON files are located in the sysroot under `share/doc/rust/json/`:
```bash
cd $(rustup run nightly rustc --print sysroot)
# JSON files at: share/doc/rust/json/
```

**Available JSON files:**
- `std.json` (9.9 MB) - Main standard library
- `core.json` (48 MB) - Core library (no_std)
- `alloc.json` (4.2 MB) - Allocation and collections
- `proc_macro.json` (648 KB) - Procedural macros
- `test.json` (639 KB) - Test framework
- `std_detect.json` (115 KB) - Platform detection

## Out of Scope (For Now)

**Potentially useful but complex:**

- **search_by_signature**: Find functions by return type or parameter types
  (Requires type matching/unification)

- **find_constructors**: Detect common construction patterns (`new()`, `default()`, builders)
  (Heuristic-based, may be useful if kept simple)

- **feature_mapping**: Show which cargo features gate specific items
  (rustdoc JSON doesn't preserve `#[cfg]` reliably)

**Not feasible:**

- **macro_signatures**: rustdoc JSON doesn't capture macro syntax patterns
