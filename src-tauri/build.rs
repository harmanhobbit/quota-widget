fn main() {
    // A branch name changes the compiled app's visible dev badge, so it must
    // invalidate Cargo's build-script cache just like a source-file change.
    println!("cargo:rerun-if-env-changed=QUOTA_WIDGET_BRANCH");
    // The mobile CI seed key is read via `option_env!("OPENROUTER_CI_KEY")` in
    // mobile.rs. Cargo auto-tracks env!/option_env! since 1.46, but making the
    // dependency explicit guards against a cached build baking a stale `None`
    // from before the secret existed — which would silently disable the seed.
    println!("cargo:rerun-if-env-changed=OPENROUTER_CI_KEY");
    tauri_build::build()
}
