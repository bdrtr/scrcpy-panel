fn main() {
    slint_build::compile("ui/mirror.slint").expect("failed to compile ui/mirror.slint");

    if cfg!(target_os = "windows") {
        // SDL2 — prebuilt MSVC libraries (still used by the audio player)
        let sdl2_lib = "D:\\dependency\\SDL2-2.32.10\\lib\\x64";
        println!("cargo:rustc-link-search=native={}", sdl2_lib);
    }
}
