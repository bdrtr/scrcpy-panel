fn main() {
    if cfg!(target_os = "windows") {
        // SDL2 — prebuilt MSVC libraries
        let sdl2_lib = "D:\\dependency\\SDL2-2.32.10\\lib\\x64";
        println!("cargo:rustc-link-search=native={}", sdl2_lib);
    }
}
