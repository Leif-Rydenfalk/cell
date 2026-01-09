utilize:  rust #![plugin]

#![plugin(…)] is the unstable crate-level attribute that tells rustc to dynamically load a compiler plugin — a dylib you write that runs inside the compiler and can
add custom lints
register new syntax extensions (procedural macros before the modern proc_macro system)
inspect or mutate the AST/HIR, emit diagnostics, etc.
