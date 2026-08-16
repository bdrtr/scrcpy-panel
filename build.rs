fn main() {
    // One root file: slint-build generates a single Rust module, so every
    // window is re-exported from ui/app.slint rather than compiled separately.
    slint_build::compile("ui/app.slint").expect("failed to compile ui/app.slint");
}
