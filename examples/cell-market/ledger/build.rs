fn main() {
    // This extracts #[cell_macro] definitions (if any) and generates the helper crate.
    // Even without explicit #[cell_macro]s, it prepares the structure.
    cell_build::CellBuilder::configure()
        .extract_macros()
        .unwrap();
}