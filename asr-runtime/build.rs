//! Build script for whisper.cpp integration.
//!
//! For Alpha-0 MVP: whisper.cpp is expected to be compiled separately
//! (via CMake) and the resulting `whisper-cli` binary bundled with the app.
//!
//! The Rust crate communicates with whisper.cpp via subprocess (CLI mode).
//! Direct FFI linking will be added in a future iteration.
//!
//! To build whisper.cpp separately:
//!   cd external/whisper.cpp
//!   mkdir build && cd build
//!   cmake .. -DGGML_CUDA=ON   # or -DGGML_CUDA=OFF for CPU-only
//!   cmake --build . --config Release
//!
//! This produces: build/bin/Release/whisper-cli.exe (Windows)

fn main() {
    // For Alpha-0: we use subprocess CLI mode, so no compilation needed here.
    // The build.rs exists as a placeholder for future FFI integration.

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:info=whisper.cpp integration uses CLI subprocess mode in Alpha-0.");
    println!("cargo:info=Build whisper.cpp separately with CMake for the CLI binary.");
}
