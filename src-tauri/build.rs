fn main() {
    // A branch name changes the compiled app's visible dev badge, so it must
    // invalidate Cargo's build-script cache just like a source-file change.
    println!("cargo:rerun-if-env-changed=QUOTA_WIDGET_BRANCH");
    tauri_build::build()
}
