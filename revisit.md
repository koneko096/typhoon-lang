# Phase 2 Gaps: AST Spans and Hindley-Milner Inference

This plan covers the implementation of source spans for better diagnostics and the transition to a full Hindley-Milner (HM) type system as outlined in the Phase 2 gaps.

## Objective
1.  **AST Spans**: Ensure every expression, statement, and declaration tracks its source location (`Span`).
2.  **HM Type Inference**: Replace the current basic type checker with a unification-based HM system supporting generics.
3.  **Advanced Resolution**: Improve name resolution for `use` paths and members.
4.  **Desugaring**: Implement a pass to lower high-level syntax before IR generation.

## Key Files & Context
- `src/span.rs`: Defines the `Span` structure (already created).
- `src/ast.rs`: Needs refactoring to include spans in nodes.
- `src/lexer.rs`: Needs to emit tokens with accurate spans (start/end offsets).
- `src/parser.rs`: Needs to capture spans during parsing.
- `src/type_inference.rs`: Will be rewritten for HM inference.
- `src/resolver.rs`: Needs updates for path resolution and member lookup.

## Implementation Plan

### Step 1: AST Span Integration (Priority)
- **Refactor `ast.rs`**: 
    - Introduce a `Spanned<T>` wrapper or add `span` fields to key node types.
    - Prefer wrapping `Expression`, `Statement`, `Pattern`, `Type`, and `Declaration` to keep enum variants concise.
    - Example: `pub struct Expr { pub kind: ExprKind, pub span: Span }`.
- **Update `lexer.rs`**:
    - Update `Token` to include `Span`.
    - Modify `Lexer` to track character offsets (`pos`) in addition to line/col.
- **Update `parser.rs`**:
    - Update all parsing functions to return `Spanned` nodes.
    - Use token spans to calculate the full span of an expression (e.g., `start_token.span.join(end_token.span)`).
- **Fix Cascading Changes**:
    - Update `resolver.rs`, `type_inference.rs`, `liveness.rs`, and `codegen.rs` to account for the new AST structure (matching on `.kind` instead of the node directly).

### Step 2: Hindley-Milner Type System
- **Define Type Representation**:
    - Create a new `Ty` enum in `type_inference.rs` with `Var`, `Con`, `App`, and `Fn`.
- **Unification Engine**:
    - Implement a `Substitution` or `Unifier` that handles type variable mapping.
    - Implement the occurs-check.
- **Inference Logic**:
    - Update the walker to perform bidirectional inference.
    - Implement `generalize` (for `let` bindings) and `instantiate` (for polymorphic calls).

### Step 3: Advanced Name Resolution & Desugaring
- **Path Resolution**: Update `resolver.rs` to fully resolve nested paths in `use` statements.
- **Member Resolution**: Track struct fields and enum variants in the symbol table to allow lookup during type checking.
- **Desugaring Pass**:
    - Implement a transformer that rewrites the AST.
    - Lower `|>` (pipe), `?` (try), and string interpolation.
    - Flatten `match` patterns.

## Verification & Testing
- **Span Verification**: Add tests to ensure error messages point to the correct line/column.
- **HM Tests**: Add test cases for generic functions (e.g., `id<T>(x: T) -> T`) and type unification.
- **Regression**: Run the existing `spec.md` examples to ensure they still parse and check correctly.
