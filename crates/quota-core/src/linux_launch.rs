//! Startup environment shims for the compatibility-floor Linux AppImage.
//!
//! The AppImage is deliberately built on Ubuntu 22.04 so it starts on the
//! widest range of hosts (see the release notes in `AGENTS.md`). That same
//! bundled-old-userspace design is what makes two things go wrong when the
//! image runs on a *newer* host than it was built on:
//!
//! 1. **WebKitGTK's DMA-BUF renderer aborts.** WebKitGTK ≥ 2.42 defaults to a
//!    DMA-BUF/EGL rendering path that asks the host for an EGL display. When the
//!    AppImage's bundled Mesa/EGL cannot bind the host GPU stack — common under
//!    XWayland and on several drivers — WebKit prints
//!    `Could not create default EGL display: EGL_BAD_PARAMETER` and calls
//!    `abort()`, so the window never opens. `WEBKIT_DISABLE_DMABUF_RENDERER=1`
//!    selects the fallback renderer, which needs no EGL display. This is the
//!    fatal failure; the shim below is what keeps the app launchable.
//!
//! 2. **GIO loads the host's GVFS modules against the bundled GLib.** The
//!    bundled GLib is 2.72 (22.04); a newer host's GIO modules
//!    (`libgvfscommon.so`, `libgvfsdbus.so`) reference `g_task_set_static_name`,
//!    a symbol added in GLib 2.76 that the bundled GLib does not export, so GIO
//!    logs `undefined symbol: g_task_set_static_name` for each as it scans the
//!    host module directory. Pointing `GIO_MODULE_DIR` at the AppImage's own
//!    bundled modules keeps GIO on the version-matched set. The widget needs no
//!    GIO module of its own — its HTTP is reqwest + rustls and the webview runs
//!    under a `default-src 'self'` CSP — so dropping the host modules removes
//!    only the mismatched ones whose load was already failing. The related
//!    `atk-bridge` "unknown signature" line has the same root cause (a newer
//!    at-spi bus answering the bundled older accessibility client) and is
//!    likewise a harmless consequence of the version skew, not a fault.
//!
//! Both overrides apply only when running as an AppImage (`APPIMAGE` is set by
//! the AppRun runtime) and only when the user has not already set the variable,
//! so a native or Nix install — and any explicit override — is left untouched.

/// One environment override to apply before GTK/WebKit initialise: the variable
/// name and the value to set it to.
pub type EnvOverride = (&'static str, String);

/// Decide the startup environment overrides for a Linux AppImage launch.
///
/// `get` reads the current environment. `bundled_gio_modules` is the AppImage's
/// own gio-modules directory *when it exists on disk* — the caller resolves and
/// existence-checks it, so this function stays pure and unit-testable. Returns
/// the variables to set, in order; empty when not running as an AppImage, or
/// when every variable it would set is already present.
pub fn appimage_launch_overrides(
    get: impl Fn(&str) -> Option<String>,
    bundled_gio_modules: Option<&str>,
) -> Vec<EnvOverride> {
    // `APPIMAGE` is set by the AppRun runtime to the path of the mounted image.
    // Its absence means a native package, a Nix build, or `cargo tauri dev` —
    // none of which bundle an old userspace, so none of these shims apply.
    if get("APPIMAGE").is_none() {
        return Vec::new();
    }

    let mut overrides = Vec::new();

    // Never override an explicit user choice: someone who set the variable —
    // to force the DMA-BUF renderer back on, or to point GIO elsewhere —
    // outranks the default we would otherwise pick.
    if get("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        overrides.push(("WEBKIT_DISABLE_DMABUF_RENDERER", "1".to_string()));
    }

    if get("GIO_MODULE_DIR").is_none() {
        if let Some(dir) = bundled_gio_modules {
            overrides.push(("GIO_MODULE_DIR", dir.to_string()));
        }
    }

    overrides
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an environment reader from a set of key/value pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn native_launch_sets_nothing() {
        // No APPIMAGE in the environment: a native or Nix install, or a dev
        // run. Even with everything else unset, the shim must not touch it.
        let out = appimage_launch_overrides(env(&[]), Some("/some/gio/modules"));
        assert!(
            out.is_empty(),
            "native launch must be left untouched: {out:?}"
        );
    }

    #[test]
    fn appimage_sets_both_when_available_and_unset() {
        let out = appimage_launch_overrides(
            env(&[("APPIMAGE", "/home/u/QuotaWidget.AppImage")]),
            Some("/tmp/.mount_xyz/usr/lib/x86_64-linux-gnu/gio/modules"),
        );
        assert_eq!(
            out,
            vec![
                ("WEBKIT_DISABLE_DMABUF_RENDERER", "1".to_string()),
                (
                    "GIO_MODULE_DIR",
                    "/tmp/.mount_xyz/usr/lib/x86_64-linux-gnu/gio/modules".to_string()
                ),
            ]
        );
    }

    #[test]
    fn user_webkit_override_is_respected() {
        // The user forced the DMA-BUF renderer on; we set only GIO_MODULE_DIR.
        let out = appimage_launch_overrides(
            env(&[
                ("APPIMAGE", "/home/u/QuotaWidget.AppImage"),
                ("WEBKIT_DISABLE_DMABUF_RENDERER", "0"),
            ]),
            Some("/mnt/gio"),
        );
        assert_eq!(out, vec![("GIO_MODULE_DIR", "/mnt/gio".to_string())]);
    }

    #[test]
    fn user_gio_override_is_respected() {
        let out = appimage_launch_overrides(
            env(&[
                ("APPIMAGE", "/home/u/QuotaWidget.AppImage"),
                ("GIO_MODULE_DIR", "/opt/custom/gio"),
            ]),
            Some("/mnt/gio"),
        );
        assert_eq!(
            out,
            vec![("WEBKIT_DISABLE_DMABUF_RENDERER", "1".to_string())]
        );
    }

    #[test]
    fn missing_bundled_gio_dir_only_fixes_rendering() {
        // No bundled modules directory found on disk: leave GIO_MODULE_DIR
        // alone rather than point it at a path that isn't there.
        let out =
            appimage_launch_overrides(env(&[("APPIMAGE", "/home/u/QuotaWidget.AppImage")]), None);
        assert_eq!(
            out,
            vec![("WEBKIT_DISABLE_DMABUF_RENDERER", "1".to_string())]
        );
    }

    #[test]
    fn everything_already_set_is_a_noop() {
        let out = appimage_launch_overrides(
            env(&[
                ("APPIMAGE", "/home/u/QuotaWidget.AppImage"),
                ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
                ("GIO_MODULE_DIR", "/opt/custom/gio"),
            ]),
            Some("/mnt/gio"),
        );
        assert!(out.is_empty(), "nothing left to set: {out:?}");
    }
}
