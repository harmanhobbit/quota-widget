use crate::model::{Status, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;

/// Display order for the account list. `Manual` is the user's config order,
/// which is what every build before this one did.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Manual,
    UsageDesc,
    UsageAsc,
    ExpirySoonest,
    ExpiryFurthest,
}

/// Which number the non-manual orders sort on. Both bases also decide *which
/// window's* `resets_at` the expiry orders use, so this selector matters for
/// all four orders, not just the usage ones.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortBasis {
    /// The amount this account contributes to the tray icon — honouring the
    /// pinned `tray_metric` and the selected headlines.
    #[default]
    Icon,
    /// The worst non-informational window, whatever the headline selection is.
    WorstCase,
}

/// The amount an account sorts on, and the reset time of the window that
/// supplied it. Either may be absent: a pure-credits account has no
/// percentage, and plenty of windows have no reset time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SortKey {
    pub pct: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
}

/// A corner of a monitor's work area. The mini summary pins to one of these
/// rather than to a free position, because it resizes to fit its content: an
/// anchored edge is what stops the window jumping when an account appears.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    /// What every build before this one did, so an old config keeps its place.
    #[default]
    BottomRight,
}

impl Corner {
    /// The corner nearest a window's centre within the given work area, used to
    /// resolve where a drag was dropped into a corner to store.
    pub fn nearest(
        centre_x: f64,
        centre_y: f64,
        area_x: f64,
        area_y: f64,
        area_w: f64,
        area_h: f64,
    ) -> Self {
        let right = centre_x >= area_x + area_w / 2.0;
        let bottom = centre_y >= area_y + area_h / 2.0;
        match (bottom, right) {
            (true, true) => Corner::BottomRight,
            (true, false) => Corner::BottomLeft,
            (false, true) => Corner::TopRight,
            (false, false) => Corner::TopLeft,
        }
    }

    pub fn is_bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }

    pub fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }
}

/// A rectangle in virtual-desktop coordinates — a window's outer bounds, or a
/// monitor's work area. Plain `f64` because the callers mix `i32` positions
/// with `u32` sizes and every comparison here is a midpoint test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The centre, which is what decides both which monitor a window counts as
    /// being on and which corner of it is nearest.
    pub fn centre(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Where the mini summary sits: a preferred monitor and a corner of its work
/// area.
///
/// `monitor` is a *preference*, not a fact about the current display layout. It
/// holds the name Tauri reports (`DP-1`, `\\.\DISPLAY1`) and is deliberately
/// kept when no connected monitor matches — undocking a laptop must not destroy
/// the choice. Resolve it against the live monitor list on every use and never
/// write the fallback back here. See docs/adr/0001-monitor-identity-by-name.md.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct MiniAnchor {
    /// `None` means "wherever the window already is" — the pre-existing
    /// behaviour, and what an unnamed monitor falls back to.
    pub monitor: Option<String>,
    pub corner: Corner,
}

impl MiniAnchor {
    /// The anchor a window at `window` currently implies: the corner of
    /// `work_area` nearest its centre, on the monitor it is actually on.
    ///
    /// Both the drop of a drag and the moment of pinning ask the same question —
    /// "which work-area corner does this window sit in?" — so both go through
    /// here, and neither moves the window to answer it. Pinning in particular
    /// derives the anchor purely for later content resizes; the window stays
    /// exactly where the user could already see it.
    ///
    /// `monitor_name` is `None` for a monitor the platform did not name, which
    /// cannot be stored (see ADR 0001): the derived corner is still kept, and
    /// `previous` keeps its monitor preference so an unnamed screen never
    /// destroys a deliberate choice.
    pub fn derive(
        window: Rect,
        work_area: Rect,
        monitor_name: Option<String>,
        previous: &MiniAnchor,
    ) -> Self {
        let (centre_x, centre_y) = window.centre();
        MiniAnchor {
            monitor: monitor_name.or_else(|| previous.monitor.clone()),
            corner: Corner::nearest(
                centre_x,
                centre_y,
                work_area.x,
                work_area.y,
                work_area.width,
                work_area.height,
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Thresholds {
    pub warn_pct: f64,
    pub critical_pct: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warn_pct: 80.0,
            critical_pct: 95.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertToggles {
    pub toast: bool,
    pub tray_color: bool,
    pub auto_popup: bool,
}

impl Default for AlertToggles {
    fn default() -> Self {
        Self {
            toast: true,
            tray_color: true,
            auto_popup: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    /// Adapter kind. Missing means the original map key, preserving old files.
    pub kind: Option<String>,
    /// User-facing account label. Missing uses the adapter's standard name.
    pub label: Option<String>,
    pub enabled: bool,
    /// Whether this account's mini-summary headline contributes to the tray
    /// icon's status and gauge fill. On by default; turn off to keep the
    /// account visible without letting its selected headline colour the tray.
    pub in_tray: bool,
    /// Overrides the global thresholds when set.
    pub thresholds: Option<Thresholds>,
    /// Overrides the global alert toggles when set.
    pub alerts: Option<AlertToggles>,
    /// Warn when a credit balance drops to/below this value.
    pub low_balance_warn: Option<f64>,
    /// Superseded by `mini_summary_metrics`. Kept so older files still load
    /// and so a downgrade finds the value it wrote; `migrate_mini_summary`
    /// folds it into the list on load.
    pub mini_summary_metric: Option<String>,
    /// Headlines shown for this account in the compact tray-click summary, one
    /// row each, in the order given. `None` preserves the automatic
    /// worst-window/credits selection; an empty list omits the account.
    pub mini_summary_metrics: Option<Vec<String>>,
    /// Which headline drives the tray icon's status and gauge. `None` folds
    /// the worst across every selected headline; `"none"` contributes nothing;
    /// otherwise the named metric alone.
    pub tray_metric: Option<String>,
    /// Provider-specific knobs (endpoint overrides, token price, …).
    pub settings: serde_json::Map<String, serde_json::Value>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: None,
            label: None,
            enabled: false,
            in_tray: true,
            thresholds: None,
            alerts: None,
            low_balance_warn: None,
            mini_summary_metric: None,
            mini_summary_metrics: None,
            tray_metric: None,
            settings: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Reserved for future migrations. Old, unversioned files deserialize as 0.
    pub version: u32,
    pub poll_interval_secs: u64,
    pub thresholds: Thresholds,
    pub alerts: AlertToggles,
    pub autostart: bool,
    /// Hide the popup when it loses focus (click-away dismiss). Off by
    /// default: on Windows, starting a title-bar drag briefly drops focus,
    /// so this fights window dragging (tauri#10767) — the poller grants a
    /// grace period after a drag press, but Esc/✕/tray remain the reliable
    /// ways to dismiss.
    pub hide_on_blur: bool,
    /// Show usage bars in the compact tray-click summary.
    pub mini_summary_bars: bool,
    /// Let scrolling over a window fade its painted shell. The level itself is
    /// deliberately ephemeral so reopening the widget never leaves it hidden.
    pub scroll_opacity: bool,
    /// Swap which wheel direction fades. The webview reports wheel deltas with
    /// the platform's own sign convention, so the gesture that fades on one OS
    /// restores on another — this lets the user match it to their machine.
    pub scroll_opacity_invert: bool,
    /// Check the public distribution manifest for a newer released build.
    pub check_updates: bool,
    /// Whether the AppImage desktop-integration question has been put to the
    /// user. It records that they were *asked*, not what they answered: saying
    /// "not now" must be remembered as firmly as saying yes, or every launch
    /// would ask again. Settings keeps the explicit add and remove controls, so
    /// a deferral is never final. Meaningless off Linux and on non-AppImage
    /// builds, which never ask.
    pub desktop_integration_prompted: bool,
    /// Display order for the account list. Defaults to `Manual`, so an existing
    /// config.json that predates this field keeps its hand-arranged order.
    pub sort_order: SortOrder,
    /// Which number the non-manual orders sort on.
    pub sort_basis: SortBasis,
    /// Where the mini summary is pinned. Defaults to the bottom-right corner of
    /// whatever monitor the window is already on, which is what every build
    /// before this one did unconditionally.
    pub mini_anchor: MiniAnchor,
    /// Account iteration order is the user-selected display order everywhere.
    pub providers: IndexMap<String, ProviderConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = IndexMap::new();
        // Claude and Codex work out of the box when their CLIs are logged in.
        providers.insert(
            "claude".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        providers.insert(
            "codex".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        providers.insert("openrouter".into(), ProviderConfig::default());
        providers.insert("elevenlabs".into(), ProviderConfig::default());
        providers.insert("firecrawl".into(), ProviderConfig::default());
        providers.insert("deepseek".into(), ProviderConfig::default());
        providers.insert("moonshot".into(), ProviderConfig::default());
        providers.insert("venice".into(), ProviderConfig::default());
        providers.insert("onehop".into(), ProviderConfig::default());
        providers.insert("fireworks".into(), ProviderConfig::default());
        providers.insert("anthropic_admin".into(), ProviderConfig::default());
        providers.insert("openai_admin".into(), ProviderConfig::default());
        providers.insert("hermes".into(), ProviderConfig::default());
        Self {
            version: 2,
            poll_interval_secs: 60,
            thresholds: Thresholds::default(),
            alerts: AlertToggles::default(),
            autostart: false,
            hide_on_blur: false,
            mini_summary_bars: true,
            scroll_opacity: true,
            scroll_opacity_invert: false,
            check_updates: true,
            desktop_integration_prompted: false,
            sort_order: SortOrder::default(),
            sort_basis: SortBasis::default(),
            mini_anchor: MiniAnchor::default(),
            providers,
        }
    }
}

impl Config {
    pub fn effective_thresholds(&self, provider_id: &str) -> Thresholds {
        self.providers
            .get(provider_id)
            .and_then(|p| p.thresholds.clone())
            .unwrap_or_else(|| self.thresholds.clone())
    }

    pub fn effective_alerts(&self, provider_id: &str) -> AlertToggles {
        self.providers
            .get(provider_id)
            .and_then(|p| p.alerts.clone())
            .unwrap_or_else(|| self.alerts.clone())
    }

    /// The headlines to show for an account. `None` means "decide
    /// automatically" — the caller picks the worst gating window, or credits.
    /// An empty list means the account is deliberately omitted. Unknown
    /// providers (a config written by a newer build) fall back to automatic.
    pub fn resolved_mini_metrics(&self, provider_id: &str) -> Option<Vec<String>> {
        self.providers
            .get(provider_id)
            .and_then(|p| p.mini_summary_metrics.clone())
    }

    /// Which single headline drives the tray, if the user pinned one. `None`
    /// means fold across every selected headline.
    pub fn tray_metric(&self, provider_id: &str) -> Option<String> {
        self.providers
            .get(provider_id)
            .and_then(|p| p.tray_metric.clone())
    }

    /// The status and optional gauge percentage an account contributes to the
    /// tray icon. The user either pins one headline or lets the worst of the
    /// selected headlines win; `"none"` deliberately contributes nothing,
    /// which lets an account stay visible in the main popup without appearing
    /// in either compact surface.
    pub fn mini_tray_status(
        &self,
        snapshot: &UsageSnapshot,
        low_balance_warn: Option<f64>,
    ) -> Option<(Status, Option<f64>)> {
        let thresholds = self.effective_thresholds(&snapshot.provider_id);
        let (warn_pct, critical_pct) = (thresholds.warn_pct, thresholds.critical_pct);
        let pinned = self.tray_metric(&snapshot.provider_id);
        if pinned.as_deref() == Some("none") {
            return None;
        }
        if snapshot.error.is_some() {
            return Some((Status::Stale, None));
        }
        if let Some(metric) = pinned {
            return metric_tray_status(snapshot, &metric, warn_pct, critical_pct, low_balance_warn);
        }

        // Worst of the selected headlines. An empty selection is the account
        // opting out of the summary entirely, so it contributes nothing.
        if let Some(selected) = self.resolved_mini_metrics(&snapshot.provider_id) {
            if selected.is_empty() {
                return None;
            }
            return selected
                .iter()
                .filter_map(|metric| {
                    metric_tray_status(snapshot, metric, warn_pct, critical_pct, low_balance_warn)
                })
                .reduce(|(status_a, pct_a), (status_b, pct_b)| {
                    (status_a.max(status_b), max_pct(pct_a, pct_b))
                });
        }

        // Automatic: the worst real quota window, as before.
        let pct = snapshot
            .windows
            .iter()
            .filter(|window| !window.informational)
            .map(|window| window.used_pct)
            .max_by(f64::total_cmp);
        Some((
            snapshot.status(warn_pct, critical_pct, low_balance_warn),
            pct,
        ))
    }

    /// The window an account sorts on under the configured basis, if it has
    /// one. Returning the window rather than a bare number is what lets the
    /// expiry orders use *that* window's `resets_at` instead of an arbitrary
    /// one — the basis selector therefore matters for all four orders.
    fn sort_window<'a>(&self, snapshot: &'a UsageSnapshot) -> Option<&'a UsageWindow> {
        // A failed fetch has no meaningful number under either basis; the
        // numbers still on screen are stale by definition.
        if snapshot.error.is_some() {
            return None;
        }
        match self.sort_basis {
            SortBasis::WorstCase => {
                worst_window(snapshot.windows.iter().filter(|w| !w.informational))
            }
            SortBasis::Icon => {
                let pinned = self.tray_metric(&snapshot.provider_id);
                if pinned.as_deref() == Some("none") {
                    // Deliberately contributes nothing to the icon, so it has
                    // nothing to sort on either.
                    return None;
                }
                if let Some(metric) = pinned {
                    return metric_window(snapshot, &metric);
                }
                match self.resolved_mini_metrics(&snapshot.provider_id) {
                    Some(selected) => worst_window(
                        selected
                            .iter()
                            .filter_map(|metric| metric_window(snapshot, metric)),
                    ),
                    // Automatic: the worst real quota window, as the icon does.
                    None => worst_window(snapshot.windows.iter().filter(|w| !w.informational)),
                }
            }
        }
    }

    /// The amount and expiry an account sorts on. Both are absent for accounts
    /// that have no number under the configured basis — a pure-credits
    /// provider, an account pinned to "none", or a failed fetch.
    pub fn sort_key(&self, snapshot: &UsageSnapshot) -> SortKey {
        match self.sort_window(snapshot) {
            Some(window) => SortKey {
                pct: Some(window.used_pct),
                resets_at: window.resets_at,
            },
            None => SortKey {
                pct: None,
                resets_at: None,
            },
        }
    }

    /// Reorders accounts for display. `Manual` leaves the config order exactly
    /// as it is. Every other order is total and stable: accounts with no
    /// number under the configured basis sink to the bottom, keeping config
    /// order among themselves, and `total_cmp` keeps NaN from making the
    /// comparator inconsistent.
    pub fn sort_snapshots(&self, snapshots: &mut Vec<UsageSnapshot>) {
        if self.sort_order == SortOrder::Manual {
            return;
        }
        let mut decorated: Vec<(SortKey, UsageSnapshot)> = std::mem::take(snapshots)
            .into_iter()
            .map(|snapshot| (self.sort_key(&snapshot), snapshot))
            .collect();
        let order = self.sort_order;
        decorated.sort_by(|(a, _), (b, _)| compare_keys(order, a, b));
        *snapshots = decorated
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .collect();
    }

    /// Folds the pre-v2 single-headline field into the list form. Gated on the
    /// file version rather than on the new fields being absent, because
    /// `tray_metric: None` is itself a meaningful value ("worst of selected")
    /// and so cannot double as "not yet migrated".
    fn migrate_mini_summary(&mut self) {
        if self.version >= 2 {
            return;
        }
        for provider in self.providers.values_mut() {
            provider.mini_summary_metrics = match provider.mini_summary_metric.as_deref() {
                // Automatic: leave as None so the adaptive pick continues.
                None => None,
                Some("none") => Some(Vec::new()),
                Some(metric) => Some(vec![metric.to_string()]),
            };
            // The old dropdown pinned the tray to exactly the one selected
            // metric, and the old checkbox was the only way to opt out.
            provider.tray_metric = if !provider.in_tray {
                Some("none".into())
            } else {
                provider.mini_summary_metric.clone().map(|metric| {
                    if metric == "none" {
                        "none".into()
                    } else {
                        metric
                    }
                })
            };
        }
        self.version = 2;
    }

    pub fn provider_setting(&self, provider_id: &str, key: &str) -> Option<serde_json::Value> {
        self.providers.get(provider_id)?.settings.get(key).cloned()
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        let mut cfg: Self = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        cfg.migrate_mini_summary();
        cfg
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut out = self.clone();
        out.migrate_mini_summary();
        // The pre-v2 fields are dead to this build but a downgrade still reads
        // them, so mirror the picker into them: the first selected headline is
        // the closest single-value equivalent.
        for provider in out.providers.values_mut() {
            provider.in_tray = provider.tray_metric.as_deref() != Some("none");
            provider.mini_summary_metric = match provider.mini_summary_metrics.as_deref() {
                None => None,
                Some([]) => Some("none".into()),
                Some([first, ..]) => Some(first.clone()),
            };
        }
        let text = serde_json::to_string_pretty(&out).expect("config serializes");
        let path = dir.join("config.json");
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

/// One selected headline's contribution to the tray. A window no longer in the
/// snapshot — a provider dropped it, or the account was reconfigured — yields
/// nothing rather than a misleading calm 0%.
fn metric_tray_status(
    snapshot: &UsageSnapshot,
    metric: &str,
    warn_pct: f64,
    critical_pct: f64,
    low_balance_warn: Option<f64>,
) -> Option<(Status, Option<f64>)> {
    if let Some(metric_id) = metric.strip_prefix("window:") {
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.metric_id == metric_id)?;
        let status = if window.used_pct >= critical_pct {
            Status::Critical
        } else if window.used_pct >= warn_pct {
            Status::Warn
        } else {
            Status::Ok
        };
        return Some((status, Some(window.used_pct)));
    }
    if metric == "credits" {
        return Some((
            snapshot.status(warn_pct, critical_pct, low_balance_warn),
            None,
        ));
    }
    None
}

/// The window with the highest `used_pct`. `total_cmp` rather than `partial_cmp`
/// so a NaN percentage from a provider cannot panic or pick arbitrarily.
fn worst_window<'a>(windows: impl Iterator<Item = &'a UsageWindow>) -> Option<&'a UsageWindow> {
    windows.max_by(|a, b| a.used_pct.total_cmp(&b.used_pct))
}

/// The window a selected headline names, if the snapshot still reports it.
/// `"credits"` names no window, so credits-only accounts sort as "no number" —
/// a balance in dollars is not comparable with a percentage.
fn metric_window<'a>(snapshot: &'a UsageSnapshot, metric: &str) -> Option<&'a UsageWindow> {
    let metric_id = metric.strip_prefix("window:")?;
    snapshot
        .windows
        .iter()
        .find(|window| window.metric_id == metric_id)
}

/// Total order for two sort keys. Accounts missing the relevant value always
/// compare greater, so they sink to the bottom whichever direction the present
/// values are sorted in; `sort_by` is stable, so they keep config order among
/// themselves.
fn compare_keys(order: SortOrder, a: &SortKey, b: &SortKey) -> Ordering {
    match order {
        SortOrder::Manual => Ordering::Equal,
        SortOrder::UsageDesc => missing_last(a.pct, b.pct, |x, y| y.total_cmp(x)),
        SortOrder::UsageAsc => missing_last(a.pct, b.pct, |x, y| x.total_cmp(y)),
        SortOrder::ExpirySoonest => missing_last(a.resets_at, b.resets_at, |x, y| x.cmp(y)),
        SortOrder::ExpiryFurthest => missing_last(a.resets_at, b.resets_at, |x, y| y.cmp(x)),
    }
}

fn missing_last<T>(a: Option<T>, b: Option<T>, cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
    match (&a, &b) {
        (Some(a), Some(b)) => cmp(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn max_pct(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (some, None) | (None, some) => some,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            poll_interval_secs: 120,
            ..Default::default()
        };
        cfg.providers.get_mut("openrouter").unwrap().enabled = true;
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(loaded, cfg);
        assert!(loaded.providers["openrouter"].enabled);
        assert_eq!(loaded.effective_thresholds("claude").warn_pct, 80.0);
    }

    #[test]
    fn config_without_mini_anchor_defaults_to_bottom_right() {
        // A config.json written before the anchor existed must keep the
        // placement it has always had, not land in a corner it never chose.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"version":2,"poll_interval_secs":60}"#,
        )
        .unwrap();
        let loaded = Config::load(dir.path());
        assert_eq!(loaded.mini_anchor.corner, Corner::BottomRight);
        assert_eq!(loaded.mini_anchor.monitor, None);
    }

    #[test]
    fn mini_anchor_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            mini_anchor: MiniAnchor {
                monitor: Some("DP-1".into()),
                corner: Corner::TopLeft,
            },
            ..Default::default()
        };
        cfg.save(dir.path()).unwrap();
        assert_eq!(Config::load(dir.path()).mini_anchor, cfg.mini_anchor);
    }

    #[test]
    fn nearest_corner_splits_the_work_area_in_quadrants() {
        // Work area offset from the origin, as a secondary monitor always is.
        let (x, y, w, h) = (1920.0, 0.0, 2560.0, 1440.0);
        let at = |cx, cy| Corner::nearest(cx, cy, x, y, w, h);
        assert_eq!(at(2000.0, 100.0), Corner::TopLeft);
        assert_eq!(at(4400.0, 100.0), Corner::TopRight);
        assert_eq!(at(2000.0, 1300.0), Corner::BottomLeft);
        assert_eq!(at(4400.0, 1300.0), Corner::BottomRight);
        // Exactly centred resolves to bottom-right, matching the default.
        assert_eq!(at(x + w / 2.0, y + h / 2.0), Corner::BottomRight);
    }

    #[test]
    fn derived_anchor_names_the_corner_the_window_already_sits_in() {
        // A secondary monitor whose work area excludes a 48px bottom panel.
        let area = Rect::new(1920.0, 0.0, 2560.0, 1392.0);
        let previous = MiniAnchor {
            monitor: Some("DP-1".into()),
            corner: Corner::BottomRight,
        };
        let derive = |x, y| {
            MiniAnchor::derive(
                Rect::new(x, y, 320.0, 200.0),
                area,
                Some("DP-2".into()),
                &previous,
            )
        };
        // Nowhere near a corner — mid-left of the work area — still resolves to
        // the nearest one, which is what a later resize grows from.
        assert_eq!(derive(2000.0, 500.0).corner, Corner::TopLeft);
        assert_eq!(derive(4100.0, 100.0).corner, Corner::TopRight);
        assert_eq!(derive(2000.0, 1100.0).corner, Corner::BottomLeft);
        assert_eq!(derive(4100.0, 1100.0).corner, Corner::BottomRight);
        // The monitor the window is on wins over the stored preference: pinning
        // must keep the summary on the screen it currently appears on.
        assert_eq!(derive(2000.0, 500.0).monitor.as_deref(), Some("DP-2"));
    }

    #[test]
    fn derived_anchor_keeps_the_monitor_preference_when_the_screen_is_unnamed() {
        // An unnamed monitor cannot be stored (ADR 0001), so the corner is
        // adopted and the previous preference survives untouched.
        let previous = MiniAnchor {
            monitor: Some("DP-1".into()),
            corner: Corner::BottomRight,
        };
        let derived = MiniAnchor::derive(
            Rect::new(20.0, 10.0, 320.0, 200.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            None,
            &previous,
        );
        assert_eq!(derived.monitor.as_deref(), Some("DP-1"));
        assert_eq!(derived.corner, Corner::TopLeft);

        // No preference and no name is still legal: "wherever the window is".
        let derived = MiniAnchor::derive(
            Rect::new(20.0, 10.0, 320.0, 200.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            None,
            &MiniAnchor::default(),
        );
        assert_eq!(derived.monitor, None);
    }

    #[test]
    fn derived_anchor_respects_the_work_area_not_the_monitor_bounds() {
        // Under a tall top panel, a summary can sit in the *lower* half of the
        // monitor and the *upper* half of the work area. Work area is what
        // placement uses (see AGENTS.md "Positioning respects panels"), so the
        // derived corner must follow it and anchor to the top.
        let monitor = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let work_area = Rect::new(0.0, 400.0, 1920.0, 680.0);
        let window = Rect::new(1500.0, 560.0, 320.0, 120.0);
        assert_eq!(
            MiniAnchor::derive(
                window,
                work_area,
                Some("DP-1".into()),
                &MiniAnchor::default()
            )
            .corner,
            Corner::TopRight
        );
        assert_eq!(
            MiniAnchor::derive(window, monitor, Some("DP-1".into()), &MiniAnchor::default()).corner,
            Corner::BottomRight
        );
    }

    #[test]
    fn missing_or_corrupt_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
        std::fs::write(dir.path().join("config.json"), "{not json").unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
    }

    #[test]
    fn per_provider_overrides_win() {
        let mut cfg = Config::default();
        cfg.providers.get_mut("claude").unwrap().thresholds = Some(Thresholds {
            warn_pct: 50.0,
            critical_pct: 75.0,
        });
        assert_eq!(cfg.effective_thresholds("claude").warn_pct, 50.0);
        assert_eq!(cfg.effective_thresholds("codex").warn_pct, 80.0);
    }

    #[test]
    fn old_unversioned_config_keeps_default_account_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"providers":{"claude":{"enabled":true}}}"#,
        )
        .unwrap();
        let cfg = Config::load(dir.path());
        // Load migrates an unversioned file forward.
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.providers["claude"].kind, None);
        assert_eq!(cfg.providers["claude"].label, None);
        assert!(cfg.scroll_opacity);
        assert!(!cfg.scroll_opacity_invert);
        assert!(cfg.check_updates);
        // An existing user has not been asked about desktop integration yet, so
        // a file predating the field must read as "not yet prompted" rather
        // than silently suppressing the question forever.
        assert!(!cfg.desktop_integration_prompted);
    }

    /// The deferral has to survive a save, or "not now" would be forgotten at
    /// the next launch and the prompt would nag.
    #[test]
    fn a_recorded_desktop_integration_prompt_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            desktop_integration_prompted: true,
            ..Default::default()
        };
        cfg.save(dir.path()).unwrap();
        assert!(Config::load(dir.path()).desktop_integration_prompted);
    }

    /// Every pre-v2 headline setting has to land on the equivalent list, or an
    /// upgrade silently changes what the mini summary and tray show.
    #[test]
    fn single_headline_setting_migrates_to_the_list_form() {
        let cases = [
            // (mini_summary_metric, in_tray, expected metrics, expected tray)
            (None, true, None, None),
            (Some("none"), true, Some(vec![]), Some("none")),
            (
                Some("window:five_hour"),
                true,
                Some(vec!["window:five_hour".to_string()]),
                Some("window:five_hour"),
            ),
            // The opt-out checkbox wins over whichever headline was picked.
            (
                Some("credits"),
                false,
                Some(vec!["credits".to_string()]),
                Some("none"),
            ),
            (None, false, None, Some("none")),
        ];
        for (metric, in_tray, want_metrics, want_tray) in cases {
            let dir = tempfile::tempdir().unwrap();
            let metric_json = match metric {
                Some(m) => format!(r#""{m}""#),
                None => "null".into(),
            };
            std::fs::write(
                dir.path().join("config.json"),
                format!(
                    r#"{{"version":1,"providers":{{"claude":{{"enabled":true,"in_tray":{in_tray},"mini_summary_metric":{metric_json}}}}}}}"#
                ),
            )
            .unwrap();
            let cfg = Config::load(dir.path());
            let claude = &cfg.providers["claude"];
            assert_eq!(
                claude.mini_summary_metrics, want_metrics,
                "metrics {metric:?}/{in_tray}"
            );
            assert_eq!(
                claude.tray_metric.as_deref(),
                want_tray,
                "tray {metric:?}/{in_tray}"
            );
        }
    }

    /// A v2 file is authoritative: re-running the migration over it would
    /// overwrite a multi-metric selection with the single-value mirror that
    /// `save` writes for downgrades.
    #[test]
    fn migration_does_not_rewrite_an_already_migrated_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.providers["claude"].mini_summary_metrics =
            Some(vec!["window:five_hour".into(), "window:weekly".into()]);
        cfg.providers["claude"].tray_metric = Some("window:weekly".into());
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(
            loaded.providers["claude"].mini_summary_metrics,
            Some(vec!["window:five_hour".into(), "window:weekly".into()])
        );
        assert_eq!(
            loaded.providers["claude"].tray_metric.as_deref(),
            Some("window:weekly")
        );
        // The downgrade mirror is the first selected headline.
        assert_eq!(
            loaded.providers["claude"].mini_summary_metric.as_deref(),
            Some("window:five_hour")
        );
    }

    #[test]
    fn saved_provider_order_and_new_metric_setting_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        let codex = cfg.providers.shift_remove("codex").unwrap();
        cfg.providers.insert("codex".into(), codex);
        cfg.providers.insert("claude".into(), claude);
        cfg.providers["claude"].mini_summary_metrics = Some(vec!["window:five_hour".into()]);
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(
            loaded.providers.keys().collect::<Vec<_>>(),
            vec![
                &"openrouter",
                &"elevenlabs",
                &"firecrawl",
                &"deepseek",
                &"moonshot",
                &"venice",
                &"onehop",
                &"fireworks",
                &"anthropic_admin",
                &"openai_admin",
                &"hermes",
                &"codex",
                &"claude"
            ]
        );
        assert_eq!(
            loaded.providers["claude"].mini_summary_metrics,
            Some(vec!["window:five_hour".to_string()])
        );
    }

    mod tray {
        use super::*;
        use crate::model::UsageWindow;

        fn snapshot() -> UsageSnapshot {
            UsageSnapshot::ok(
                "claude",
                "Claude",
                vec![
                    UsageWindow {
                        metric_id: "five_hour".into(),
                        label: "5-hour".into(),
                        used_pct: 13.0,
                        ..Default::default()
                    },
                    UsageWindow {
                        metric_id: "weekly".into(),
                        label: "Weekly".into(),
                        used_pct: 88.0,
                        ..Default::default()
                    },
                ],
                None,
            )
        }

        fn status_of(
            metrics: Option<Vec<&str>>,
            tray: Option<&str>,
        ) -> Option<(Status, Option<f64>)> {
            let mut cfg = Config::default();
            let claude = cfg.providers.get_mut("claude").unwrap();
            claude.mini_summary_metrics =
                metrics.map(|m| m.into_iter().map(String::from).collect());
            claude.tray_metric = tray.map(String::from);
            cfg.mini_tray_status(&snapshot(), None)
        }

        /// The headline the user pinned is the only one that counts, even when
        /// a louder one is also on screen.
        #[test]
        fn a_pinned_metric_drives_the_tray_alone() {
            let got = status_of(
                Some(vec!["window:five_hour", "window:weekly"]),
                Some("window:five_hour"),
            );
            assert_eq!(got, Some((Status::Ok, Some(13.0))));
        }

        /// Without a pin, no selected headline may go critical unnoticed.
        #[test]
        fn worst_of_selected_folds_across_every_headline() {
            let got = status_of(Some(vec!["window:five_hour", "window:weekly"]), None);
            assert_eq!(got, Some((Status::Warn, Some(88.0))));
        }

        #[test]
        fn none_and_an_empty_selection_contribute_nothing() {
            assert_eq!(status_of(Some(vec![]), None), None);
            assert_eq!(status_of(Some(vec!["window:weekly"]), Some("none")), None);
        }

        /// Automatic still means "worst real quota window".
        #[test]
        fn automatic_selection_keeps_the_old_behaviour() {
            assert_eq!(status_of(None, None), Some((Status::Warn, Some(88.0))));
        }

        /// A selection naming a window the provider stopped reporting must not
        /// register as a calm 0%.
        #[test]
        fn a_missing_window_is_skipped_rather_than_counted_as_zero() {
            let got = status_of(Some(vec!["window:gone", "window:five_hour"]), None);
            assert_eq!(got, Some((Status::Ok, Some(13.0))));
            assert_eq!(status_of(Some(vec!["window:gone"]), None), None);
        }
    }

    mod sorting {
        use super::*;
        use crate::model::{FetchError, UsageWindow};
        use chrono::TimeZone;

        fn at(hour: u32) -> Option<DateTime<Utc>> {
            Some(Utc.with_ymd_and_hms(2026, 8, 4, hour, 0, 0).unwrap())
        }

        fn window(id: &str, used_pct: f64, hour: Option<u32>) -> UsageWindow {
            UsageWindow {
                metric_id: id.into(),
                label: id.into(),
                used_pct,
                resets_at: hour.and_then(at),
                ..Default::default()
            }
        }

        fn snap(id: &str, windows: Vec<UsageWindow>) -> UsageSnapshot {
            UsageSnapshot::ok(id, id, windows, None)
        }

        /// Four accounts whose icon-attributing and worst-case numbers
        /// deliberately disagree, so a test that passes under one basis fails
        /// under the other.
        fn fixture() -> (Config, Vec<UsageSnapshot>) {
            let mut cfg = Config::default();
            // Pinned to the calm window; worst-case sees the loud one.
            cfg.providers["claude"].tray_metric = Some("window:five_hour".into());
            let snapshots = vec![
                snap(
                    "claude",
                    vec![
                        window("five_hour", 10.0, Some(9)),
                        window("weekly", 90.0, Some(20)),
                    ],
                ),
                snap("codex", vec![window("five_hour", 50.0, Some(12))]),
                snap("openrouter", vec![window("monthly", 70.0, Some(6))]),
            ];
            (cfg, snapshots)
        }

        fn ids(snapshots: &[UsageSnapshot]) -> Vec<&str> {
            snapshots.iter().map(|s| s.provider_id.as_str()).collect()
        }

        fn sorted(order: SortOrder, basis: SortBasis) -> Vec<String> {
            let (mut cfg, mut snapshots) = fixture();
            cfg.sort_order = order;
            cfg.sort_basis = basis;
            cfg.sort_snapshots(&mut snapshots);
            ids(&snapshots).into_iter().map(String::from).collect()
        }

        /// Manual must not disturb the order at all — it is what every build
        /// before this feature did, and the default for existing configs.
        #[test]
        fn manual_is_exactly_config_order() {
            let (mut cfg, snapshots) = fixture();
            cfg.sort_order = SortOrder::Manual;
            for basis in [SortBasis::Icon, SortBasis::WorstCase] {
                cfg.sort_basis = basis;
                let mut got = snapshots.clone();
                cfg.sort_snapshots(&mut got);
                assert_eq!(got, snapshots, "{basis:?}");
            }
        }

        /// The basis picks a different number for Claude (10% pinned vs 90%
        /// worst), so it must move Claude to the other end of the list.
        #[test]
        fn usage_orders_follow_the_basis() {
            assert_eq!(
                sorted(SortOrder::UsageDesc, SortBasis::Icon),
                ["openrouter", "codex", "claude"]
            );
            assert_eq!(
                sorted(SortOrder::UsageAsc, SortBasis::Icon),
                ["claude", "codex", "openrouter"]
            );
            assert_eq!(
                sorted(SortOrder::UsageDesc, SortBasis::WorstCase),
                ["claude", "openrouter", "codex"]
            );
            assert_eq!(
                sorted(SortOrder::UsageAsc, SortBasis::WorstCase),
                ["codex", "openrouter", "claude"]
            );
        }

        /// Expiry must use the reset time of the window that supplied the
        /// amount: Claude's pinned 5-hour window resets at 09:00 while its
        /// worst-case weekly window resets at 20:00, which is the whole point
        /// of the basis applying to the expiry orders too.
        #[test]
        fn expiry_orders_use_the_basis_windows_reset_time() {
            assert_eq!(
                sorted(SortOrder::ExpirySoonest, SortBasis::Icon),
                ["openrouter", "claude", "codex"]
            );
            assert_eq!(
                sorted(SortOrder::ExpiryFurthest, SortBasis::Icon),
                ["codex", "claude", "openrouter"]
            );
            // Worst-case moves Claude's expiry from 09:00 to 20:00.
            assert_eq!(
                sorted(SortOrder::ExpirySoonest, SortBasis::WorstCase),
                ["openrouter", "codex", "claude"]
            );
            assert_eq!(
                sorted(SortOrder::ExpiryFurthest, SortBasis::WorstCase),
                ["claude", "codex", "openrouter"]
            );
        }

        /// Equal numbers must not reshuffle: the sort is stable, so ties keep
        /// the user's hand-arranged order.
        #[test]
        fn ties_keep_config_order() {
            let cfg = Config {
                sort_order: SortOrder::UsageDesc,
                ..Default::default()
            };
            let mut snapshots = vec![
                snap("claude", vec![window("w", 42.0, Some(9))]),
                snap("codex", vec![window("w", 42.0, Some(9))]),
                snap("openrouter", vec![window("w", 42.0, Some(9))]),
            ];
            let before = snapshots.clone();
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(snapshots, before);
        }

        /// Accounts with no number sink, in config order among themselves,
        /// whichever direction the ones that do have numbers are sorted in.
        #[test]
        fn accounts_without_a_number_sink_to_the_bottom() {
            let mut cfg = Config::default();
            // Pure credits: no window, so nothing to compare as a percentage.
            cfg.providers["openrouter"].tray_metric = Some("credits".into());
            // Opted out of the icon entirely.
            cfg.providers["elevenlabs"].tray_metric = Some("none".into());
            let build = || {
                vec![
                    UsageSnapshot::ok(
                        "openrouter",
                        "OpenRouter",
                        vec![],
                        Some(crate::model::Credits {
                            balance: 5.0,
                            label: None,
                            unit: "USD".into(),
                            used: None,
                            granted: None,
                            est_tokens_remaining: None,
                        }),
                    ),
                    snap("elevenlabs", vec![window("w", 99.0, Some(9))]),
                    snap("claude", vec![window("w", 10.0, Some(9))]),
                    snap("codex", vec![window("w", 80.0, Some(12))]),
                ]
            };
            for order in [
                SortOrder::UsageDesc,
                SortOrder::UsageAsc,
                SortOrder::ExpirySoonest,
                SortOrder::ExpiryFurthest,
            ] {
                cfg.sort_order = order;
                let mut snapshots = build();
                cfg.sort_snapshots(&mut snapshots);
                // The two numberless accounts are last, in config order.
                assert_eq!(
                    &ids(&snapshots)[2..],
                    ["openrouter", "elevenlabs"],
                    "{order:?}"
                );
            }
        }

        /// A window with no reset time has nothing to sort on under the expiry
        /// orders, even though it has a perfectly good percentage.
        #[test]
        fn a_window_without_a_reset_time_sinks_under_expiry_orders() {
            let mut cfg = Config {
                sort_order: SortOrder::ExpirySoonest,
                ..Default::default()
            };
            let mut snapshots = vec![
                snap("claude", vec![window("w", 90.0, None)]),
                snap("codex", vec![window("w", 10.0, Some(12))]),
            ];
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(ids(&snapshots), ["codex", "claude"]);
            // The same pair sorts on the percentages under a usage order.
            cfg.sort_order = SortOrder::UsageDesc;
            let mut snapshots = vec![
                snap("claude", vec![window("w", 90.0, None)]),
                snap("codex", vec![window("w", 10.0, Some(12))]),
            ];
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(ids(&snapshots), ["claude", "codex"]);
        }

        /// A failed fetch leaves stale numbers on screen; sorting on them would
        /// rank an account by data we already know is wrong.
        #[test]
        fn errored_snapshots_sink_rather_than_sorting_on_stale_numbers() {
            let cfg = Config {
                sort_order: SortOrder::UsageDesc,
                ..Default::default()
            };
            let mut stale = snap("claude", vec![window("w", 99.0, Some(9))]);
            stale.error = Some(FetchError::Network("boom".into()));
            let mut snapshots = vec![stale, snap("codex", vec![window("w", 10.0, Some(12))])];
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(ids(&snapshots), ["codex", "claude"]);
        }

        /// Informational windows never gate anything, so they must not decide
        /// the worst-case number either.
        #[test]
        fn worst_case_ignores_informational_windows() {
            let cfg = Config {
                sort_order: SortOrder::UsageDesc,
                sort_basis: SortBasis::WorstCase,
                ..Default::default()
            };
            let mut loud = window("trickle", 100.0, Some(9));
            loud.informational = true;
            let mut snapshots = vec![
                snap("claude", vec![loud, window("real", 5.0, Some(9))]),
                snap("codex", vec![window("w", 50.0, Some(12))]),
            ];
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(ids(&snapshots), ["codex", "claude"]);
        }

        /// A NaN percentage must not panic or make the comparator inconsistent
        /// — `total_cmp` orders it after every real number.
        #[test]
        fn a_nan_percentage_is_ordered_rather_than_panicking() {
            let cfg = Config {
                sort_order: SortOrder::UsageDesc,
                ..Default::default()
            };
            let mut snapshots = vec![
                snap("claude", vec![window("w", f64::NAN, Some(9))]),
                snap("codex", vec![window("w", 50.0, Some(12))]),
                snap("openrouter", vec![window("w", 90.0, Some(6))]),
            ];
            cfg.sort_snapshots(&mut snapshots);
            assert_eq!(ids(&snapshots), ["claude", "openrouter", "codex"]);
        }

        /// The whole point of the serde defaults: a file written before this
        /// feature existed must load as today's behaviour, with no migration
        /// and no version bump.
        #[test]
        fn a_config_without_the_new_fields_defaults_to_manual() {
            let cfg: Config =
                serde_json::from_str(r#"{"version":2,"providers":{"claude":{"enabled":true}}}"#)
                    .unwrap();
            assert_eq!(cfg.sort_order, SortOrder::Manual);
            assert_eq!(cfg.sort_basis, SortBasis::Icon);
            assert_eq!(cfg.version, 2);
        }

        /// The settings selects round-trip through JSON as snake_case strings.
        #[test]
        fn the_new_fields_round_trip_as_snake_case() {
            let cfg = Config {
                sort_order: SortOrder::ExpirySoonest,
                sort_basis: SortBasis::WorstCase,
                ..Default::default()
            };
            let text = serde_json::to_string(&cfg).unwrap();
            assert!(text.contains(r#""sort_order":"expiry_soonest""#), "{text}");
            assert!(text.contains(r#""sort_basis":"worst_case""#), "{text}");
            let back: Config = serde_json::from_str(&text).unwrap();
            assert_eq!(back.sort_order, SortOrder::ExpirySoonest);
            assert_eq!(back.sort_basis, SortBasis::WorstCase);
        }
    }

    /// Saving from Settings does not go straight from struct to file: the
    /// config crosses IPC as a `serde_json::Value` first. Without serde_json's
    /// `preserve_order` feature that intermediate `Map` is a `BTreeMap`, which
    /// silently re-sorts the account keys alphabetically and threw away every
    /// reorder the user made. The file round-trip above cannot catch this
    /// because it never builds a `Value`.
    #[test]
    fn provider_order_survives_a_round_trip_through_serde_json_value() {
        let mut cfg = Config::default();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        cfg.providers.insert("claude".into(), claude); // move claude last
        let before: Vec<String> = cfg.providers.keys().cloned().collect();
        assert_ne!(
            before,
            {
                let mut sorted = before.clone();
                sorted.sort();
                sorted
            },
            "test is only meaningful if the order is not already alphabetical"
        );

        let via_value = serde_json::to_value(&cfg).unwrap();
        let back: Config = serde_json::from_value(via_value).unwrap();

        assert_eq!(back.providers.keys().cloned().collect::<Vec<_>>(), before);
    }
}
