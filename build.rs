fn main() {
    // One root file: slint-build generates a single Rust module, so every
    // window is re-exported from ui/app.slint rather than compiled separately.
    slint_build::compile("ui/app.slint").expect("failed to compile ui/app.slint");

    if cfg!(target_os = "windows") {
        // SDL2 — prebuilt MSVC libraries (still used by the audio player)
        let sdl2_lib = "D:\\dependency\\SDL2-2.32.10\\lib\\x64";
        println!("cargo:rustc-link-search=native={}", sdl2_lib);
    }
}
