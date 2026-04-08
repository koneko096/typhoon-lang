# Typhoon Implementation Plan

This document outlines the step-by-step implementation of the Typhoon language as defined in Section 17 of the language specification.

## Phase 1: Lexer and Parser
**Goal:** Parse all Typhoon syntax and produce a concrete AST with source spans.

- [x] Implement Lexer
    - [x] Basic tokens (keywords, operators, literals)
    - [x] String interpolation (tokenize as sequence of string parts and expression spans)
    - [x] Doc comments
- [x] Implement Parser
    - [x] Operator precedence table (Pipe `|>` lowest, `?` highest after field access)
    - [x] Disambiguation of `{ ...x, f: v }` (merge expression) vs `{ stmt; expr }` (block)
    - [x] AST with source spans for error reporting
- [x] **Milestone:** All example programs in `spec.md` parse without error.

## Phase 2: Name Resolution and Type Inference
**Goal:** Resolve every identifier to its declaration and infer all types via bidirectional Hindley-Milner.

- [ ] Name Resolution
    - [ ] Build scope tree
    - [ ] Resolve all identifiers to canonical `DeclId`
    - [ ] Introduce `DeclId` and `ScopeId` interners (arena indices)
    - [ ] Symbol tables per scope (map `String` → `DeclId`)
    - [ ] Handle shadowing and duplicate declaration errors
    - [ ] Resolve `use` paths and populate namespace imports
    - [ ] Resolve `struct`/`enum`/`newtype` type names in type annotations
    - [ ] Resolve member names for field access and enum variants
    - [ ] Emit source-span-aware errors (unknown name, ambiguous path, private access)
    - [ ] Tests: scope shadowing, unresolved names, `use` glob, path segments
- [ ] Type Inference
    - [ ] Implement bidirectional HM inference
    - [ ] Handle generics and interface bounds
    - [ ] `Result`/`Option` desugaring
    - [ ] Type representation: `TyVar`, `TyCon`, `TyApp`, `TyFn`, `TyTuple`, `TyArray`
    - [ ] Unification with occurs check and union-find
    - [ ] Generalization at `let` bindings
    - [ ] Instantiation at identifier use sites
    - [ ] Expected-type propagation (bidirectional) for literals and blocks
    - [ ] Numeric literal defaulting rules (Int32, Float32) with explicit `as`
    - [ ] Constraint solving for interface bounds (trait-like)
    - [ ] Structural typing rules for `struct` initialization and `enum` variants
    - [ ] Type checking for `if`, `match`, `return`, and block trailing expression
    - [ ] Tests: inferred let types, function calls, polymorphic `Option`/`Result`
- [ ] Desugaring
    - [ ] Desugar `?` operator (verify compatible return type)
    - [ ] Eliminate `|>` by rewriting to direct calls before IR lowering
    - [ ] Desugar `match` arms into core pattern forms (for later liveness)
    - [ ] Desugar string interpolation into `Buf` builder calls
    - [ ] Track desugared spans for error mapping
- [ ] **Milestone:** All example programs type-check. Invalid programs produce clear type errors.

### Phase 2 Status
- Resolver: Basic scope tree, `DeclId`/`ScopeId` interners, per-scope symbol tables, and duplicate detection are implemented in `src/resolver.rs`. The resolver currently handles functions, parameters, `let` bindings, and `use` declarations; its tests cover parameter binding, missing identifiers, and duplicate declarations.
- Type Checker: `src/type_inference.rs` validates literals, annotated `let` bindings, return expressions, and simple block typings against the lightweight `InferType` lattice (`Int32`, `Float32`, `Bool`, `Str`). The accompanying tests exercise a spec-inspired function and produce a mismatch error when a `let` shoves a string into an `Int32`.
- Tests: After clearing the incremental cache, `cargo test` now successfully builds and exercises the parser, resolver, and type-inference suites, demonstrating the applied Phase 2 functionality.

## Phase 3: Liveness Checker
**Goal:** Enforce linear type rules and annotate every binding with its consumption point.

- [ ] Implement Linear Type Rules
    - [ ] Maintain live set per scope (stack of `LiveSetId`s)
    - [ ] Track consumption at assignment (`let b = a`)
    - [ ] Track consumption at function calls
    - [ ] Track consumption in merge expressions
    - [ ] Track consumption in `conc` block captures and channel sends
    - [ ] Track consumption during pattern matching (match arms, destructuring)
    - [ ] Track consumption when calling generic functions (monomorphized types)
    - [ ] Model `ref` types as shared, escaping the linear live set
    - [ ] Record spans of consumption and creation to improve diagnostics
    - [ ] Integrate with resolver/type-inference results (`DeclId` → `InferType`)
- [ ] Implement Automatic Drop Insertion
    - [ ] Insert drops for remaining live bindings at end of scope
    - [ ] Drop-insertion must respect `@repr(C)` / FFI boundaries
    - [ ] Emit drops for `match`/`if` tails that exit early
    - [ ] Support `Drop` trait hooking for standard library types
- [ ] Handle Special Bindings
    - [ ] Exempt `let mut` from liveness tracking (free at scope exit)
    - [ ] Support `static`/`const` globals as always-live
    - [ ] Track renames/aliases created by `let alias = original`
- [ ] Conditional Liveness
    - [ ] Ensure all branches of `if`/`match` consume the same live bindings
    - [ ] Validate loops (`while`, `for`) maintain live-set invariants across iterations
    - [ ] Ensure early `return`/`break`/`continue` consume pending bindings
    - [ ] Emit actionable diagnostics describing which binding was prematurely consumed or forgotten
    - [ ] Generate test suite covering linear violations: conditional moves, `conc` capture misuse, channels
    - [ ] Provide regression harness that runs against `spec.md` examples for `conc`/`merge`
- [ ] **Milestone:** Ownership violations caught with clear error messages. Test suite covers conditional moves, loop moves, and captures.

### Phase 3 Status
- Live sets: `LiveSet`/`LiveBinding` arenas track `let` bindings, parameters, and temporary expressions (`src/liveness.rs`).
- Analyzer: `LiveAnalyzer` walks functions, records consumption on identifier uses, and emits drop notes for unconsumed bindings; `drops` also notes origin context.
- Branches/loops: conditional branches (`if`, `match`, `if let`) now enforce consistent consumption across branches; loops are validated to preserve the entry live set.
- `conc` support: parser, resolver, and type checker accept `conc { ... }` blocks; the liveness analyzer treats `conc` captures as consuming bindings from the parent scope.
- Tests: regression cases show dropped unused parameters, detect double consumption, flag inconsistent conditional consumption, reject loop-consumption patterns, and validate `conc` capture consumption.

### Phase 3 Plan
- Step 1: Define `LiveSet`, `LiveBinding`, and `LiveSetId` arenas plus `LiveSetStack` to mirror the current scope tree (`DeclId` → `InferType` connections will be reused from Phase 2).
- Step 2: Instrument the AST walker (reusing the resolver) so each `let`, `match`, `return`, `conc`, and channel send records consumption/creation spans and updates the live set state.
- Step 3: Emit drop instructions for any live binding that survives to the end of a scope, paying attention to `@repr(C)`/FFI boundaries and the `Drop` trait hook for stdlib types.
- Step 4: Build diagnostics/tests covering conditional moves, loops, `conc` captures, and channel ownership failures, using `spec.md` examples as regression harnesses.

## Phase 1–3 Verification & Gaps

Summary: ran the test-suite and inspected core modules. Phase 1 (lexer + parser) and Phase 3 (liveness) are largely implemented and tested; Phase 2 (name resolution + type inference) is partially implemented. Below lists the gaps discovered and short remediation notes (file pointers).

Phase 1 gaps
- String interpolation split into parts + expression spans: PARTIAL — lexer treats interpolated strings as single StrLit. (src/lexer.rs string_lit)
- Doc-comments tokenization: PARTIAL — comments are skipped but not tracked as doc comments. (src/lexer.rs skip_whitespace)
- AST spans for error mapping: MISSING — AST nodes lack source span fields. (src/ast.rs)

Phase 2 gaps (high priority)
- Resolve `use` paths fully / import population: PARTIAL — `use` declares last path segment only. (src/resolver.rs)
- Member names (field access / enum variants): MISSING — resolver does not map field/variant names to DeclId. (src/resolver.rs; needs symbol metadata)
- Full Hindley–Milner inference (generics, unification, generalization): MISSING — current TypeChecker is an annotation-driven checker (src/type_inference.rs)
- Desugaring (`?`, `|>`, match, string interpolation): MISSING — no dedicated desugar pass.
- Type representations (TyVar/TyCon/TyApp/TyFn): MISSING — InferType is lightweight, needs redesign for HM.

Phase 3 gaps
- Span-aware diagnostics: MISSING — liveness reports strings but AST lacks spans. (src/ast.rs, src/liveness.rs)
- FFI/@repr(C) and Drop-trait aware drop-insertion: MISSING
- Stronger integration with typed DeclId map: PARTIAL — liveness works with identifier names; mapping DeclId→InferType not integrated. (src/liveness.rs)

Suggested next work (Phase 2 continue)
1. Add AST source spans (small API change across parser + lexer) — enables better diagnostics and eases error mapping.
2. Upgrade TypeChecker to a HM core:
   - Introduce TyVar/TyCon/TyApp/TyFn types
   - Implement unification with occurs-check and union-find
   - Generalize at let bindings and instantiate at use sites
3. Implement generic resolution: declare generic params and bind them to type variables during inference.
4. Add desugar pass to lower `?`, `|>`, and string interpolation before IR lowering.

## Phase 4: LLVM Code Generation
**Goal:** Produce correct native binaries for all example programs.

- [ ] Lower AST to LLVM IR
    - [ ] Struct lowering (sorted by alignment unless `@repr(C)`)
    - [ ] `alloca` for size-stable `let` bindings
    - [ ] `malloc` placeholder for heap types (to be replaced in Phase 5)
    - [ ] `match` lowering (switch for integers, decision trees for structures)
    - [ ] Function prolog/epilog generation (stack frame layout)
    - [ ] Generic monomorphization
    - [ ] Call lowering with ABI usage and argument passing/promotion
    - [ ] Pointer provenance metadata for `ref` vs linear pointers
    - [ ] Inline `@derive` generated helpers (Eq, Hash, Display)
- [ ] Optimizations and Annotations
    - [ ] Add `noalias` to non-`ref` pointers
    - [ ] Add `nonnull` to non-optional pointers
    - [ ] Overflow behavior (`nsw`/`nuw` in debug, wrapping in release)
    - [ ] Tail-call optimization hints for recursive functions
    - [ ] Loop unrolling hints for `for` over `[T]`
    - [ ] Emit debug metadata for source spans
- [ ] **Milestone:** Programs compile to native binaries and produce correct output.

### Phase 4 Status
- Control flow: `if`/`else` and `while` loops emit labeled basic blocks with conditional `br`; `return`, `let`, and expressions lower to registers/local slots.
- Struct/enum: module preamble emits `struct`/`enum` type defs, `struct` initialization uses GEP/store per field, `merge` mutates fields via GEP/store, and placeholders for enums/match prepare for tagging.
- Codegen binary: `src/main.rs` now drives lexing → parsing → resolution → typing → liveness → codegen, writes `.ll`, and invokes `clang`; README documents the compile recipe.

## Phase 5: Slab Allocator and Scheduler
**Goal:** Replace `malloc`/`free` placeholders with production runtime.

- [ ] Implement Per-Task Slab Allocator
    - [ ] Bump allocator
    - [ ] Size-class free list
    - [ ] Virtual memory reservation (`mmap`/`VirtualAlloc`)
    - [ ] Integration with LLVM IR (heap type lowering uses allocator interfaces)
- [ ] Implement M:N Scheduler
    - [ ] Work-stealing deques (one OS thread per core)
    - [ ] Stackful coroutines (64 KB initial, grows on fault)
    - [ ] Cooperative yielding at I/O and channel blocks
    - [ ] Preemptive yielding via `SIGPROF`
    - [ ] Task-local slabs recycled per coroutine to avoid cross-thread locking
    - [ ] Scheduler API exposed to language runtime (`spawn`, `await`, `conc`)
- [ ] Implement Channels
    - [ ] `chan<T>` as bounded ring buffer with coroutine waitlists
    - [ ] Support `select`/`recv` semantics with fairness hints
    - [ ] Linear ownership of channel tokens (send consumes, recv produces)
- [ ] **Milestone:** `conc` and `chan` examples run correctly under concurrent load. Benchmarked favorably against `jemalloc`.

## Phase 6: IO and Networking
**Goal:** Working HTTP server with zero-copy parsing and capability model.

- [ ] Implement IO Driver (Rust FFI Bridge)
    - [ ] `io_uring` (Linux)
    - [ ] `kqueue` (macOS)
    - [ ] `IOCP` (Windows)
    - [ ] Async-safe handles consumable per task (`Network` capability token)
- [ ] Integration with Scheduler
    - [ ] I/O operations transparently yield coroutines
    - [ ] Polling driver uses scheduler waitlists for read/write readiness
- [ ] Capability Model
    - [ ] Generate `Network` token at `main` entry point
    - [ ] Enforce token linearity during `net.listen` / `net.accept`
- [ ] Networking Implementation
    - [ ] `LinearSocket` with single-owner semantics
    - [ ] Zero-copy HTTP/1.1 resumable parser (`StrView` pointers into slab)
    - [ ] Header linear scan
    - [ ] TLS handshake offloaded to Rust or OS primitives (optional phase)
    - [ ] Back-pressure via channel-based request queue
- [ ] **Milestone:** HTTP server handles 10,000 concurrent connections. Benchmarked against Go and Rust.

## Phase 7: Standard Library
**Goal:** Provide essential built-in types and utilities.

- [ ] Tier 1: Core (Global)
    - [ ] `Str`, `Buf`, `[T]`, `Map<K,V>`, `Set<T>`, `Option<T>`, `Result<T,E>`
    - [ ] `std::io` (read, write, print, scan)
    - [ ] `assert!`/`assert_eq!` macros in `test`
    - [ ] Expose `StrView` for zero-copy parsing helpers
- [ ] Tier 2: Standard (Explicit Import)
    - [ ] `ref T`
    - [ ] `std::math`, `std::time`, `std::fmt`, `std::fs`, `std::process`
    - [ ] Add `std::net` wrappers around `LinearSocket` for convenience
- [ ] Tier 3: Ecosystem (Packages)
    - [ ] `json`, `http` (client), `test`, `@derive`
    - [ ] Guidelines for publishing packages via `typhoon.toml`
- [ ] **Milestone:** Complete and usable standard library for practical application development.
