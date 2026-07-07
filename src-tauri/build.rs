fn main() {
    // Force recompilation when these env vars change (used by option_env! in code)
    println!("cargo:rerun-if-env-changed=AUDRAFLOW_GITHUB_TOKEN");
    println!("cargo:rerun-if-env-changed=AUDRAFLOW_COMPONENT_RELEASE_TAG");
    println!("cargo:rerun-if-env-changed=AUDRAFLOW_COMPONENT_BASE_URL");
    println!("cargo:rerun-if-env-changed=AUDRAFLOW_BUILD_REPO");
    tauri_build::build()
}
