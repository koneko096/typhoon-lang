1. Static Interface Resolution (Monomorphization)

To maintain Maximum Throughput, Helix will not use VTables or runtime dynamic dispatch for interfaces.

    Compile-Time Expansion: When a function uses a generic bound (e.g., T: Display), the compiler generates a unique version of that function for every concrete type used in the program.

    Inlining Potential: Because the concrete type is known at the call site, LLVM can inline interface methods directly into the caller. This removes the branch-prediction penalty of a virtual call.

    Strict Typing: The compiler validates that the concrete type implements all required methods of the InterfaceDecl before code generation.

2. Mutability & Concurrency Resolution

The core safety rule of Helix is that mut is a local-only permission that cannot cross the "Isolation Boundary" of a conc block.
The "Isolation Guard" Mechanism

    Capture Analysis: When the compiler encounters a ConcStmt, it inspects the MoveCaptures.

    The Restriction: Any variable marked as mut in the parent scope is forbidden from appearing in MoveCaptures.

    The Workflow:

        A user can mutate a local variable (e.g., building a list).

        To send it to a concurrent task, they must "freeze" it by re-binding it as an immutable linear type: let frozen_data = my_mut_data.

        Only frozen_data can be moved into the conc block.
