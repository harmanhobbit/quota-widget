use crate::model::{Status, UsageSnapshot, UsageWindow};
use crate::platform_preferences::PlatformPreferences;
use crate::shared_config::SharedConfig;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

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

/// The weekdays an account is expected to be used on. Every weekday defaults
/// to active, so an account that has never opted in is paced across all seven
/// days — byte-for-byte the behaviour of every build before the field existed.
///
/// The schedule is purely presentational (ADR-0007): it reshapes a weekly
/// window's period marker to advance only on scheduled days, and never touches
/// the usage a provider reports nor the status and alerts that key off it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct UsageSchedule {
    pub monday: bool,
    pub tuesday: bool,
    pub wednesday: bool,
    pub thursday: bool,
    pub friday: bool,
    pub saturday: bool,
    pub sunday: bool,
}

impl Default for UsageSchedule {
    fn default() -> Self {
        Self {
            monday: true,
            tuesday: true,
            wednesday: true,
            thursday: true,
            friday: true,
            saturday: true,
            sunday: true,
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
    /// The weekdays this account is expected to be used on. All seven by
    /// default, which is indistinguishable from the field never existing.
    pub usage_schedule: UsageSchedule,
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
            usage_schedule: UsageSchedule::default(),
            settings: Default::default(),
        }
    }
}

/// The `auth_mode` value that pins an account to a local CLI credential file
/// (`~/.codex/auth.json`, `~/.claude/.credentials.json`, `~/.grok/auth.json`).
const AUTH_MODE_CLI: &str = "cli";
/// The `auth_mode` value that uses the widget's own built-in sign-in — the only
/// source that exists on Android, and what a freshly-added mobile account gets.
const AUTH_MODE_OAUTH: &str = "oauth";

/// Coerce every `auth_mode: "cli"` account to the built-in sign-in.
///
/// Android has no CLI for any provider, so a `cli` auth mode can never be
/// satisfied there — yet one arrives all the same, carried verbatim from a
/// desktop config through a credential transfer (`crate::transfer::apply_bundle`
/// copies each account's settings as-is) or left behind by an older build. Since
/// the mobile Settings UI has no sign-in-method selector, the user cannot correct
/// it, and the account is stuck reporting "no CLI login found" even after a
/// successful built-in sign-in (the `cli` fetch path never consults the stored
/// OAuth token). Rewriting the mode to `oauth` — what `MobileApp`'s `newAccount`
/// writes for a fresh account — makes the transferred account behave like a
/// locally-added one.
///
/// Returns whether anything changed, so a caller can skip a redundant save.
/// Callers are the Android config seams only; desktop keeps `cli` mode.
pub fn coerce_cli_auth_mode_to_oauth(providers: &mut IndexMap<String, ProviderConfig>) -> bool {
    let mut changed = false;
    for config in providers.values_mut() {
        if config.settings.get("auth_mode").and_then(|v| v.as_str()) == Some(AUTH_MODE_CLI) {
            config.settings.insert(
                "auth_mode".into(),
                serde_json::Value::String(AUTH_MODE_OAUTH.into()),
            );
            changed = true;
        }
    }
    changed
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
        // Grok is disabled by default: unlike Claude/Codex it needs an explicit
        // sign-in (or the grok CLI logged in) before it can fetch anything.
        providers.insert("grok".into(), ProviderConfig::default());
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

/// Why an existing `config.json` could not be turned into a `Config`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    /// The bytes were readable but are not a config — truncated, hand-edited
    /// into invalid JSON, or written by something else entirely.
    Malformed,
    /// The file is there but the process cannot read it: permissions, a
    /// directory in its place, an I/O error on the underlying storage.
    Unreadable,
}

/// A configuration that exists on disk and could not be loaded.
///
/// This is deliberately *not* a repair. The accounts, thresholds and provider
/// settings in that file are the user's only copy of work that took real effort
/// to enter, and the secret keys in the keyring are derived from its provider
/// keys (see AGENTS.md, "Secret keys are derived from config") — so a config we
/// cannot parse still names secrets we cannot enumerate. The file is therefore
/// left exactly where it is, running on defaults, until the user decides what
/// happens to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigRecovery {
    pub kind: RecoveryKind,
    /// The untouched original, still at its normal path.
    pub path: PathBuf,
    /// The parse or I/O error, verbatim, because it is usually the fastest
    /// route to fixing the file by hand (`line 12 column 3`).
    pub detail: String,
}

impl ConfigRecovery {
    /// One line, suitable for the application log and for a status banner.
    pub fn message(&self) -> String {
        let what = match self.kind {
            RecoveryKind::Malformed => "could not be parsed",
            RecoveryKind::Unreadable => "could not be read",
        };
        format!(
            "{} {what} ({}). Running on defaults; the original has been kept and \
             saving is blocked until it is recovered or replaced.",
            self.path.display(),
            self.detail
        )
    }
}

/// `shared_config::SharedConfig` carries its own copy of this shape (see that
/// module's doc for why it does not depend on this one surviving), so once
/// storage has migrated its recovery state is bridged back into the type this
/// module's callers — and the IPC layer — already know.
impl From<crate::shared_config::RecoveryKind> for RecoveryKind {
    fn from(kind: crate::shared_config::RecoveryKind) -> Self {
        match kind {
            crate::shared_config::RecoveryKind::Malformed => RecoveryKind::Malformed,
            crate::shared_config::RecoveryKind::Unreadable => RecoveryKind::Unreadable,
        }
    }
}

impl From<crate::shared_config::SharedConfigRecovery> for ConfigRecovery {
    fn from(recovery: crate::shared_config::SharedConfigRecovery) -> Self {
        ConfigRecovery {
            kind: recovery.kind.into(),
            path: recovery.path,
            detail: recovery.detail,
        }
    }
}

/// The result of a load: always a usable `Config`, plus whatever had to be set
/// aside to produce it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLoad {
    pub config: Config,
    /// `None` for both a valid file and no file at all — a first run is not a
    /// failure and must not ask the user to recover anything.
    pub recovery: Option<ConfigRecovery>,
}

/// The outcome of [`Config::load_presence`]: whether a *persisted, healthy*
/// shared configuration exists, deliberately **without** a "fell back to
/// defaults" case. Both the callers that use it — the headless refresh and the
/// widget's cold read — must never act on the built-in default accounts
/// (`Config::default` pre-enables Claude and Codex), so they need to tell a real
/// persisted config apart from the two shapes where [`Config::load`] would
/// otherwise hand them substituted defaults: a **missing** config (a first run)
/// and a **corrupt** one. Issue #113's contract is explicit that *both* missing
/// and corrupt shared config must never substitute defaults.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigPresence {
    /// A healthy, persisted shared config. The refresh may fetch and save; the
    /// widget projects over it normally.
    Present(Config),
    /// No shared config has ever been written (a first run): nothing is
    /// configured, and there are no credentials to read. The refresh stands down
    /// and touches nothing; the widget surfaces "needs configuration" rather than
    /// rendering the built-in defaults.
    Absent,
    /// A shared config exists but is corrupt or unreadable. The refresh stands
    /// down and leaves the last-known read model intact; the widget surfaces
    /// "needs configuration". Never the substituted defaults.
    Corrupt(ConfigRecovery),
}

/// What `save` refuses to do, and why. `io::Error` rather than a bespoke type
/// so the existing `Result<(), io::Error>` call sites are unchanged.
fn refuse_overwrite(recovery: &ConfigRecovery) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "refusing to overwrite the existing configuration: {}",
            recovery.message()
        ),
    )
}

impl Config {
    /// Android's true first run: `Config::default()` pre-enables Claude and
    /// Codex, which is right for desktop (CLI login may already be present)
    /// but wrong for Android, which starts provider-onboarding empty (see
    /// `docs/adr/0006-…`) — an out-of-the-box Claude/Codex account there has
    /// no CLI to discover and no credential yet, so it would only ever show
    /// as a failed account until the user has even opened Settings.
    pub fn mobile_first_run_default() -> Config {
        Config {
            providers: IndexMap::new(),
            ..Config::default()
        }
    }

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

    /// Splits into the two files storage now uses: see
    /// `crate::shared_config` and `crate::platform_preferences`.
    pub fn split(&self) -> (SharedConfig, PlatformPreferences) {
        (
            SharedConfig::from_legacy(self),
            PlatformPreferences::from_legacy(self),
        )
    }

    /// Reassembles the combined shape every other part of the desktop app
    /// (IPC, `Settings.svelte`, the poller) still reads and writes — the split
    /// is a storage-layer detail, not something that reaches the frontend.
    pub fn from_parts(shared: SharedConfig, prefs: PlatformPreferences) -> Self {
        Self {
            // Always current: `SharedConfig`/`PlatformPreferences` are never
            // constructed except from an already-migrate_mini_summary'd
            // legacy `Config` (see `read_legacy`) or from defaults that are
            // already in the current shape, so re-running that migration
            // would be wrong — `migrate_mini_summary` keys off this field, and
            // `SharedConfig::version` tracks a different, unrelated axis.
            version: 2,
            poll_interval_secs: prefs.poll_interval_secs,
            thresholds: shared.thresholds,
            alerts: shared.alerts,
            autostart: prefs.autostart,
            hide_on_blur: prefs.hide_on_blur,
            mini_summary_bars: prefs.mini_summary_bars,
            scroll_opacity: prefs.scroll_opacity,
            scroll_opacity_invert: prefs.scroll_opacity_invert,
            check_updates: prefs.check_updates,
            desktop_integration_prompted: prefs.desktop_integration_prompted,
            sort_order: shared.sort_order,
            sort_basis: shared.sort_basis,
            mini_anchor: prefs.mini_anchor,
            providers: shared.providers,
        }
    }

    /// Reads a pre-split `config.json` exactly as every build before the
    /// split did — including the mini-summary migration — without deciding
    /// what to do about the result. Used only by `migrate_if_needed`.
    fn read_legacy(path: &Path) -> (Self, Option<ConfigRecovery>) {
        let (mut cfg, recovery) = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (
                    Self::default(),
                    Some(ConfigRecovery {
                        kind: RecoveryKind::Malformed,
                        path: path.to_path_buf(),
                        detail: e.to_string(),
                    }),
                ),
            },
            // Only "no such file" is a first run. Everything else — a
            // permission denial, a directory in its place, a failing disk —
            // means a config is there and we simply cannot see it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(ConfigRecovery {
                    kind: RecoveryKind::Unreadable,
                    path: path.to_path_buf(),
                    detail: e.to_string(),
                }),
            ),
        };
        cfg.migrate_mini_summary();
        (cfg, recovery)
    }

    /// One-time upgrade from the combined `config.json` to the two files
    /// storage now uses, run transparently the first time this build sees a
    /// data directory that has not migrated yet.
    ///
    /// Returns `Some` only when migration could not proceed because the
    /// legacy file is there but unreadable — exactly the condition that used
    /// to block `save`, now reported against `config.json` one last time
    /// rather than a `shared-config.json` that was never created. A directory
    /// that has already migrated (`shared-config.json` present) or never had
    /// a legacy file at all returns `None` immediately and touches nothing.
    /// One-time migration of a legacy `config.json` into the split
    /// shared-config + preferences files. The caller MUST hold the snapshot
    /// store lock: this persists configuration, and every configuration write
    /// is coordinated with the merge's membership validation through that
    /// lock (`Config::save`, `save_after_recovery` — the same rule as r10).
    /// Callers inside an already-held lock (`load`, reached under the lock
    /// from `membership_authority`) simply pass it through; taking the lock
    /// here would self-deadlock a `flock`-based mutex.
    fn migrate_if_needed(dir: &Path) -> Option<ConfigRecovery> {
        if dir.join(crate::shared_config::FILE_NAME).exists() {
            return None;
        }
        let legacy_path = dir.join("config.json");
        let (legacy, recovery) = Self::read_legacy(&legacy_path);
        if let Some(recovery) = recovery {
            return Some(recovery);
        }
        // No legacy file either: a true first run, nothing to migrate.
        if !legacy_path.exists() {
            return None;
        }
        let (shared, prefs) = legacy.split();
        // Best-effort: a failure here just means the fresh split files don't
        // exist yet and the *next* load or save retries the whole migration
        // from the still-untouched legacy file — nothing is lost either way.
        if shared.write_fresh(dir).is_err() || prefs.save(dir).is_err() {
            return None;
        }
        // Kept, never deleted, and never consulted again once the split files
        // exist — `migrate_if_needed`'s first check short-circuits on them.
        let _ = std::fs::rename(&legacy_path, dir.join("config.json.migrated"));
        None
    }

    /// Reads the current configuration, always yielding a usable value.
    ///
    /// Three outcomes: no data directory (or one that migrated cleanly) is a
    /// first run or an ordinary load on the established defaults; a valid
    /// shared configuration loads as-is; a shared configuration — or, before
    /// the one-time migration, a legacy `config.json` — that exists but cannot
    /// be read or parsed runs on defaults *and* reports a [`ConfigRecovery`],
    /// because quietly treating it as a first run is how a working setup gets
    /// replaced by an empty one at the next save.
    /// Read the current configuration WITHOUT coordinating through the store
    /// lock. For callers already holding that lock (`membership_authority`,
    /// reached under it from `merge_and_store`) — taking it here would
    /// self-deadlock a `flock`-based mutex. Reads are safe without the lock:
    /// `save`/`write_split` publish atomically via temp-file-then-rename.
    /// The one-time legacy migration may write, and DOES honor the caller's
    /// held lock (see `migrate_if_needed`'s contract).
    pub(crate) fn load_unlocked(dir: &Path) -> ConfigLoad {
        if let Some(recovery) = Self::migrate_if_needed(dir) {
            return ConfigLoad {
                config: Self::default(),
                recovery: Some(recovery),
            };
        }
        let shared = SharedConfig::load(dir);
        let prefs = PlatformPreferences::load(dir);
        ConfigLoad {
            config: Self::from_parts(shared.config, prefs),
            recovery: shared.recovery.map(ConfigRecovery::from),
        }
    }

    /// Read the current configuration, always yielding a usable value.
    ///
    /// Three outcomes: no data directory (or one that migrated cleanly) is a
    /// first run or an ordinary load on the established defaults; a valid
    /// shared configuration loads as-is; a shared configuration — or, before
    /// the one-time migration, a legacy `config.json` — that exists but cannot
    /// be read or parsed runs on defaults *and* reports a [`ConfigRecovery`],
    /// because quietly treating it as a first run is how a working setup gets
    /// replaced by an empty one at the next save. Takes the snapshot store
    /// lock for the migration it may perform (see `migrate_if_needed`).
    pub fn load(dir: &Path) -> ConfigLoad {
        // The migration this may perform persists configuration, so the store
        // lock's guard is BOUND for the whole load — migration and read are
        // one critical section against configuration writes and merges.
        // Dropping the guard after acquisition (checking only `is_err()`)
        // would release the lock before `load_unlocked` runs and leave the
        // migration uncoordinated again. Callers that already hold the lock
        // (`membership_authority`, under `merge_and_store`) use
        // `load_unlocked` instead. A lock failure is pathological (the lock
        // file cannot be opened); degrade to an uncoordinated best-effort
        // read rather than fail the whole read.
        let _lock = match crate::snapshots::store_lock(dir) {
            Ok(lock) => lock,
            Err(_) => return Self::load_unlocked(dir),
        };
        let loaded = Self::load_unlocked(dir);
        if let Some(recovery) = loaded.recovery {
            return ConfigLoad {
                config: Self::default(),
                recovery: Some(recovery),
            };
        }
        loaded
    }

    pub fn load_presence(dir: &Path) -> ConfigPresence {
        let load = Self::load(dir);
        if let Some(recovery) = load.recovery {
            return ConfigPresence::Corrupt(recovery);
        }
        // After a clean `load`, a persisted shared config exists iff the split
        // file is on disk. `load` runs the one-time legacy migration first, so a
        // pre-split `config.json` has already been turned into this file by now
        // (and a corrupt/unreadable legacy file would have surfaced as `recovery`
        // above) — only a true first run, with no legacy file to migrate either,
        // leaves it absent, and `load` never writes one on its own. This is the
        // same crate-private filename `migrate_if_needed` keys off.
        if !dir.join(crate::shared_config::FILE_NAME).exists() {
            return ConfigPresence::Absent;
        }
        ConfigPresence::Present(load.config)
    }

    /// The recovery state of whatever is on disk right now, independent of what
    /// is in memory. `save` consults this rather than trusting a flag carried
    /// from startup, so a file that became unreadable while the app was running
    /// is caught too.
    pub fn recovery_state(dir: &Path) -> Option<ConfigRecovery> {
        Self::load(dir).recovery
    }

    /// Writes the configuration, unless doing so would destroy an existing one
    /// we could not read.
    ///
    /// The ordinary save path must never be the thing that discards a user's
    /// accounts: with an unreadable shared configuration (or, pre-migration, an
    /// unreadable legacy `config.json`) present this fails with `InvalidData`
    /// and writes nothing. Replacing it is a separate, explicit act —
    /// [`Config::save_after_recovery`].
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        // EVERY configuration persistence coordinates through the snapshot
        // store lock — including the one-time migration this may perform:
        // it persists configuration (the split shared-config files), and an
        // uncoordinated write here could interleave with the merge's
        // membership validation. The lock covers the migration AND the save,
        // so neither can land between the merge's membership comparison and
        // its own save of the read model.
        let _store_lock = crate::snapshots::store_lock(dir)?;
        if let Some(recovery) = Self::migrate_if_needed(dir) {
            return Err(refuse_overwrite(&recovery));
        }
        if let Some(recovery) = SharedConfig::recovery_state(dir) {
            return Err(refuse_overwrite(&ConfigRecovery::from(recovery)));
        }
        self.write_split(dir)
    }

    /// The explicit recovery action: move the unreadable original aside, then
    /// save. Returns where the original was kept, so the caller can tell the
    /// user where to find it — nothing is ever deleted here.
    ///
    /// A no-op recovery (nothing wrong on disk) is not an error; it just saves,
    /// which keeps the command idempotent if the file was fixed by hand in the
    /// meantime. Handles both a pre-migration unreadable `config.json` and a
    /// post-migration unreadable `shared-config.json` — whichever one is
    /// actually blocking is the one moved aside.
    pub fn save_after_recovery(&self, dir: &Path) -> std::io::Result<Option<PathBuf>> {
        // One store lock across everything this can write — the migration,
        // the legacy backup, and the recovery save: the merge's membership
        // validation reads the configuration under that same lock, so none of
        // these may interleave with it (r10's coordination, extended to the
        // r11 gap: migration ran BEFORE the lock was taken).
        let _store_lock = crate::snapshots::store_lock(dir)?;
        // Pre-migration: an outstanding unreadable legacy `config.json` is the
        // thing to recover from. A valid legacy file (or none at all) just
        // migrates transparently, same as `load`/`save`, before falling
        // through to the post-migration path below.
        if !dir.join(crate::shared_config::FILE_NAME).exists() {
            let legacy_path = dir.join("config.json");
            let (_, recovery) = Self::read_legacy(&legacy_path);
            if recovery.is_some() {
                let kept = free_backup_path(dir, "config.json");
                std::fs::rename(&legacy_path, &kept)?;
                self.write_split(dir)?;
                return Ok(Some(kept));
            }
            Self::migrate_if_needed(dir);
        }
        let Some(_) = SharedConfig::recovery_state(dir) else {
            self.write_split(dir)?;
            return Ok(None);
        };
        let (shared, prefs) = self.split();
        let kept = shared
            .save_after_recovery(dir)?
            .expect("recovery_state confirmed a shared-config recovery is outstanding");
        prefs.save(dir)?;
        Ok(Some(kept))
    }

    fn write_split(&self, dir: &Path) -> std::io::Result<()> {
        let (shared, prefs) = self.split();
        shared.write_fresh(dir)?;
        prefs.save(dir)
    }
}

/// Where an unreadable legacy `config.json` is kept when the user replaces it
/// (pre-migration only — a post-migration `shared-config.json` uses
/// `crate::shared_config`'s own numbering). Numbered rather than overwritten,
/// because a second failure must not destroy the copy taken after the first
/// one — the whole point of keeping it is that it is the only surviving
/// record of the accounts.
fn free_backup_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(format!("{name}.unreadable"));
    if !first.exists() {
        return first;
    }
    for n in 2u32.. {
        let candidate = dir.join(format!("{name}.unreadable.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 exhausted while naming a backup")
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

    /// The config a load produced, for the majority of tests that are about
    /// what was read rather than about recovery.
    fn load(dir: &Path) -> Config {
        Config::load(dir).config
    }

    /// A healthy, persisted shared config is reported `Present` with exactly that
    /// config — the only case a refresh may fetch/save or the widget may render.
    #[test]
    fn load_presence_returns_a_persisted_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.providers.clear();
        cfg.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        cfg.save(dir.path()).unwrap();

        match Config::load_presence(dir.path()) {
            ConfigPresence::Present(loaded) => {
                assert!(loaded.providers.contains_key("openrouter"));
                assert!(!loaded.providers.contains_key("claude"));
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    /// A true first run — no shared config ever written — is `Absent`, NOT the
    /// built-in defaults (which pre-enable Claude and Codex). This is the case
    /// the review flagged: `Config::load` returns `recovery: None` here, so a
    /// `recovery`-only check would silently substitute the defaults.
    #[test]
    fn load_presence_on_a_first_run_is_absent_not_defaulted() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load_presence(dir.path()), ConfigPresence::Absent);
        // `Config::load` alone would have handed back the pre-enabled defaults.
        let load = Config::load(dir.path());
        assert!(load.recovery.is_none());
        assert!(load.config.providers.contains_key("claude"));
    }

    /// A corrupt shared config is `Corrupt`, so the caller stands down rather
    /// than acting on substituted defaults.
    #[test]
    fn load_presence_on_a_corrupt_config_reports_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(crate::shared_config::FILE_NAME),
            "{ not json",
        )
        .unwrap();
        match Config::load_presence(dir.path()) {
            ConfigPresence::Corrupt(recovery) => {
                assert_eq!(recovery.kind, RecoveryKind::Malformed);
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    /// The legacy-migration path stays compatible with `Present`: a pre-split
    /// `config.json` (no `shared-config.json` yet) is migrated by `load` before
    /// the presence check, so an upgrading user with a real config is `Present`,
    /// never mistaken for a first-run `Absent` that would drop their accounts.
    #[test]
    fn load_presence_treats_a_migrated_legacy_config_as_present() {
        let dir = tempfile::tempdir().unwrap();
        // A pre-split combined config.json with a hand-entered account.
        let mut legacy = Config::default();
        legacy.providers.clear();
        legacy.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        // No shared-config.json exists yet — only the legacy file.
        assert!(!dir.path().join(crate::shared_config::FILE_NAME).exists());

        match Config::load_presence(dir.path()) {
            ConfigPresence::Present(loaded) => {
                assert!(loaded.providers.contains_key("openrouter"));
                assert!(!loaded.providers.contains_key("claude"));
            }
            other => panic!("expected Present after migration, got {other:?}"),
        }
    }

    fn account_with_auth_mode(kind: &str, auth_mode: Option<&str>) -> ProviderConfig {
        let mut config = ProviderConfig {
            kind: Some(kind.into()),
            ..Default::default()
        };
        if let Some(mode) = auth_mode {
            config
                .settings
                .insert("auth_mode".into(), serde_json::Value::String(mode.into()));
        }
        config
    }

    #[test]
    fn coerce_cli_auth_mode_rewrites_only_cli_and_reports_change() {
        let mut providers = IndexMap::new();
        providers.insert("codex".into(), account_with_auth_mode("codex", Some("cli")));
        providers.insert(
            "claude".into(),
            account_with_auth_mode("claude", Some("cli")),
        );
        providers.insert("grok".into(), account_with_auth_mode("grok", Some("oauth")));
        providers.insert("hermes".into(), account_with_auth_mode("hermes", None));

        assert!(coerce_cli_auth_mode_to_oauth(&mut providers));

        // Both `cli` accounts are rewritten to the built-in sign-in.
        assert_eq!(
            providers["codex"].settings["auth_mode"],
            serde_json::json!("oauth")
        );
        assert_eq!(
            providers["claude"].settings["auth_mode"],
            serde_json::json!("oauth")
        );
        // An explicit `oauth` is left as-is; an account with no `auth_mode`
        // (the Auto default) is untouched, not given one.
        assert_eq!(
            providers["grok"].settings["auth_mode"],
            serde_json::json!("oauth")
        );
        assert!(!providers["hermes"].settings.contains_key("auth_mode"));
    }

    #[test]
    fn coerce_cli_auth_mode_is_a_noop_without_any_cli_account() {
        let mut providers = IndexMap::new();
        providers.insert(
            "codex".into(),
            account_with_auth_mode("codex", Some("oauth")),
        );
        providers.insert(
            "openrouter".into(),
            account_with_auth_mode("openrouter", None),
        );

        // Nothing to change, so it reports false and leaves settings alone.
        assert!(!coerce_cli_auth_mode_to_oauth(&mut providers));
        assert_eq!(
            providers["codex"].settings["auth_mode"],
            serde_json::json!("oauth")
        );
        assert!(!providers["openrouter"].settings.contains_key("auth_mode"));
    }

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            poll_interval_secs: 120,
            ..Default::default()
        };
        cfg.providers.get_mut("openrouter").unwrap().enabled = true;
        cfg.save(dir.path()).unwrap();

        let loaded = load(dir.path());
        assert_eq!(loaded, cfg);
        assert!(loaded.providers["openrouter"].enabled);
        assert_eq!(loaded.effective_thresholds("claude").warn_pct, 80.0);
    }

    #[test]
    fn mobile_first_run_default_has_no_preset_accounts() {
        let cfg = Config::mobile_first_run_default();
        assert!(cfg.providers.is_empty());
        // Everything else matches desktop's defaults — only the account
        // presets differ.
        assert_eq!(cfg.poll_interval_secs, Config::default().poll_interval_secs);
        assert_eq!(cfg.thresholds, Config::default().thresholds);
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
        let loaded = load(dir.path());
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
        assert_eq!(load(dir.path()).mini_anchor, cfg.mini_anchor);
    }

    fn all_seven(schedule: &UsageSchedule) -> bool {
        schedule.monday
            && schedule.tuesday
            && schedule.wednesday
            && schedule.thursday
            && schedule.friday
            && schedule.saturday
            && schedule.sunday
    }

    #[test]
    fn usage_schedule_defaults_to_all_seven_days() {
        assert!(all_seven(&UsageSchedule::default()));
        // Every preset account carries the all-seven default, so a fresh
        // config is indistinguishable from a build before the field existed.
        for provider in Config::default().providers.values() {
            assert!(all_seven(&provider.usage_schedule), "{provider:?}");
        }
    }

    #[test]
    fn config_without_usage_schedule_loads_all_seven_days() {
        // A config.json written before the field existed must load with every
        // day active — an absent schedule is "used all seven days", never an
        // account that is suddenly paced across zero days.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"version":2,"providers":{"claude":{"enabled":true}}}"#,
        )
        .unwrap();
        let loaded = load(dir.path());
        assert!(all_seven(&loaded.providers["claude"].usage_schedule));
    }

    #[test]
    fn usage_schedule_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        let schedule = UsageSchedule {
            saturday: false,
            sunday: false,
            ..UsageSchedule::default()
        };
        cfg.providers.get_mut("claude").unwrap().usage_schedule = schedule;
        cfg.save(dir.path()).unwrap();

        let reloaded = load(dir.path()).providers["claude"].usage_schedule;
        assert_eq!(reloaded, schedule);
        assert!(!reloaded.saturday && !reloaded.sunday);
        assert!(all_seven(&UsageSchedule::default()));
    }

    #[test]
    fn a_partial_usage_schedule_keeps_unspecified_days_active() {
        // A hand-edited schedule that names only the days it switches off must
        // not silently turn the unmentioned days off — the all-seven default is
        // the base, and an explicit `false` is the exception.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"version":2,"providers":{"claude":{"enabled":true,"usage_schedule":{"sunday":false}}}}"#,
        )
        .unwrap();
        let schedule = load(dir.path()).providers["claude"].usage_schedule;
        assert!(!schedule.sunday);
        assert!(schedule.monday && schedule.saturday);
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

    /// The three load outcomes, which the rest of the recovery behaviour hangs
    /// off. A first run is not a failure; a file we cannot use is not a first
    /// run.
    mod recovery {
        use super::*;

        /// A config with something in it worth losing, so a test that silently
        /// discards it is distinguishable from one that keeps it.
        fn a_configured_file() -> String {
            let mut cfg = Config {
                poll_interval_secs: 300,
                ..Default::default()
            };
            cfg.providers
                .insert("claude#2".into(), ProviderConfig::default());
            serde_json::to_string_pretty(&cfg).unwrap()
        }

        #[test]
        fn a_missing_file_is_a_first_run_on_defaults() {
            let dir = tempfile::tempdir().unwrap();
            let loaded = Config::load(dir.path());
            assert_eq!(loaded.config, Config::default());
            // Nothing to recover: an empty config dir must not put the app into
            // a recovery state it can never leave.
            assert_eq!(loaded.recovery, None);
            // And an ordinary save is allowed, which is what makes first run
            // work at all.
            loaded.config.save(dir.path()).unwrap();
            assert_eq!(load(dir.path()), Config::default());
        }

        #[test]
        fn a_valid_file_reports_no_recovery() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), a_configured_file()).unwrap();
            let loaded = Config::load(dir.path());
            assert_eq!(loaded.recovery, None);
            assert_eq!(loaded.config.poll_interval_secs, 300);
            assert!(loaded.config.providers.contains_key("claude#2"));
        }

        #[test]
        fn a_malformed_file_runs_on_defaults_and_is_kept() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(&path, "{not json").unwrap();

            let loaded = Config::load(dir.path());
            assert_eq!(loaded.config, Config::default());
            let recovery = loaded.recovery.expect("malformed file reports recovery");
            assert_eq!(recovery.kind, RecoveryKind::Malformed);
            assert_eq!(recovery.path, path);
            // The bytes are untouched — this is the user's only copy.
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
            // The message names the file and says what happened.
            assert!(recovery.message().contains("config.json"), "{recovery:?}");
            assert!(
                recovery.message().contains("could not be parsed"),
                "{recovery:?}"
            );
        }

        /// A file that is there but cannot be read is not a missing file. A
        /// directory in its place reproduces that for any user, where a
        /// permission bit would not (root reads everything).
        #[test]
        fn an_unreadable_file_runs_on_defaults_and_is_kept() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::create_dir(&path).unwrap();

            let loaded = Config::load(dir.path());
            assert_eq!(loaded.config, Config::default());
            let recovery = loaded.recovery.expect("unreadable file reports recovery");
            assert_eq!(recovery.kind, RecoveryKind::Unreadable);
            assert!(path.exists());
            assert!(
                recovery.message().contains("could not be read"),
                "{recovery:?}"
            );
        }

        /// Denied permissions are the case users actually hit. Skipped when the
        /// tests run as root, where the mode is not enforced.
        #[cfg(unix)]
        #[test]
        fn a_permission_denied_file_reports_unreadable_rather_than_missing() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(&path, a_configured_file()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            if std::fs::read_to_string(&path).is_ok() {
                return; // running as root: the mode proves nothing
            }
            let recovery = Config::load(dir.path())
                .recovery
                .expect("permission denial reports recovery");
            assert_eq!(recovery.kind, RecoveryKind::Unreadable);
        }

        /// The core of the issue: the ordinary save path is what would replace
        /// the accounts, and it must refuse.
        #[test]
        fn an_ordinary_save_refuses_to_replace_an_unreadable_config() {
            // Hand-edited into invalid JSON, and truncated to nothing — the
            // shape a half-written or zero-length file actually has.
            for broken in ["{not json", "", r#"{"poll_interval_secs":"#] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("config.json");
                std::fs::write(&path, broken).unwrap();

                let err = Config::default()
                    .save(dir.path())
                    .expect_err("save must refuse");
                assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
                // Nothing written, nothing renamed: the original is still there
                // byte for byte, and no temp file was left behind.
                assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
                assert!(!dir.path().join("config.json.tmp").exists());
            }
        }

        /// The explicit action: the original is moved aside, not deleted, and
        /// the new config takes its place.
        #[test]
        fn recovery_keeps_the_original_and_lets_the_next_save_through() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), "{not json").unwrap();

            let cfg = Config {
                poll_interval_secs: 90,
                ..Default::default()
            };
            let kept = cfg
                .save_after_recovery(dir.path())
                .unwrap()
                .expect("the original is kept somewhere");
            assert_eq!(std::fs::read_to_string(&kept).unwrap(), "{not json");

            let loaded = Config::load(dir.path());
            assert_eq!(loaded.recovery, None);
            assert_eq!(loaded.config.poll_interval_secs, 90);
            // And ordinary saving works again from here.
            loaded.config.save(dir.path()).unwrap();
        }

        /// Once migrated, `config.json` is no longer consulted at all — a second
        /// write to it (simulating some other process touching the old file
        /// after the split) must not be mistaken for a second recovery, and
        /// must not disturb the shared configuration the app is actually using.
        /// The numbered-backup guarantee for *repeated* corruption now belongs
        /// to `shared_config`, since `shared-config.json` is the live store —
        /// see `shared_config::tests::a_second_recovery_keeps_the_earlier_copy`.
        #[test]
        fn a_stale_legacy_file_after_migration_is_inert() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), "first").unwrap();
            let first = Config::default()
                .save_after_recovery(dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");

            // Something else writes garbage back to the now-inert legacy path.
            std::fs::write(dir.path().join("config.json"), "second").unwrap();
            let cfg = Config {
                poll_interval_secs: 77,
                ..Default::default()
            };
            // Not treated as a recovery scenario: the live store is fine.
            assert_eq!(cfg.save_after_recovery(dir.path()).unwrap(), None);
            assert_eq!(load(dir.path()).poll_interval_secs, 77);
        }

        /// Recovering when there is nothing wrong is an ordinary save, so the
        /// command is safe to repeat and never invents a backup file.
        #[test]
        fn recovery_with_nothing_wrong_is_just_a_save() {
            let dir = tempfile::tempdir().unwrap();
            let cfg = Config {
                poll_interval_secs: 45,
                ..Default::default()
            };
            assert_eq!(cfg.save_after_recovery(dir.path()).unwrap(), None);
            assert_eq!(load(dir.path()).poll_interval_secs, 45);
            assert!(!dir.path().join("config.json.unreadable").exists());
        }

        /// Recovery must not become a way to lose a *valid* config: with a
        /// readable file present there is nothing to move aside, so the save
        /// goes through the ordinary path and leaves no copy behind.
        #[test]
        fn recovery_never_moves_a_valid_config_aside() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), a_configured_file()).unwrap();
            assert_eq!(
                Config::default().save_after_recovery(dir.path()).unwrap(),
                None
            );
            assert!(!dir.path().join("config.json.unreadable").exists());
        }
    }

    /// The on-disk half of the shared-configuration/platform-preferences
    /// split: a pre-split `config.json` transparently becomes the two new
    /// files the first time this build touches the data directory, with every
    /// value preserved and the legacy file kept (never deleted) as a receipt.
    mod migration {
        use super::*;

        /// A config with something in it worth losing, so a test that silently
        /// discards it is distinguishable from one that keeps it.
        fn a_configured_file() -> String {
            let mut cfg = Config {
                poll_interval_secs: 300,
                ..Default::default()
            };
            cfg.providers
                .insert("claude#2".into(), ProviderConfig::default());
            serde_json::to_string_pretty(&cfg).unwrap()
        }

        #[test]
        fn a_legacy_config_migrates_transparently_on_first_load() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), a_configured_file()).unwrap();

            let loaded = Config::load(dir.path());
            assert_eq!(loaded.recovery, None, "a clean migration reports nothing");
            assert_eq!(loaded.config.poll_interval_secs, 300);
            assert!(loaded.config.providers.contains_key("claude#2"));

            // The split files exist, and the legacy file is kept as a receipt
            // rather than deleted.
            assert!(dir.path().join("shared-config.json").exists());
            assert!(dir.path().join("platform-preferences.json").exists());
            assert!(!dir.path().join("config.json").exists());
            assert!(dir.path().join("config.json.migrated").exists());

            // The migration runs exactly once: a second load reads the split
            // files directly and does not re-touch the legacy receipt.
            let receipt_before =
                std::fs::read_to_string(dir.path().join("config.json.migrated")).unwrap();
            let again = Config::load(dir.path());
            assert_eq!(again.config, loaded.config);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("config.json.migrated")).unwrap(),
                receipt_before
            );
        }

        /// Every shared field and every platform field survives the split,
        /// not just the two spot-checked by the transparency test above.
        #[test]
        fn migration_preserves_every_field_across_both_new_files() {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = Config {
                sort_order: SortOrder::UsageDesc,
                sort_basis: SortBasis::WorstCase,
                autostart: true,
                hide_on_blur: true,
                mini_anchor: MiniAnchor {
                    monitor: Some("DP-1".into()),
                    corner: Corner::TopLeft,
                },
                ..Default::default()
            };
            cfg.thresholds.warn_pct = 42.0;
            cfg.providers.get_mut("claude").unwrap().label = Some("Work".into());
            let text = serde_json::to_string_pretty(&cfg).unwrap();
            std::fs::write(dir.path().join("config.json"), text).unwrap();

            let migrated = Config::load(dir.path()).config;
            assert_eq!(migrated, cfg);
        }

        /// An unreadable legacy `config.json` still blocks migration exactly as
        /// it always blocked an ordinary load — the split files must not be
        /// created from data that could not be read.
        #[test]
        fn an_unreadable_legacy_config_blocks_migration() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.json"), "{not json").unwrap();

            let loaded = Config::load(dir.path());
            assert!(loaded.recovery.is_some());
            assert!(!dir.path().join("shared-config.json").exists());
            assert!(!dir.path().join("platform-preferences.json").exists());
            assert!(dir.path().join("config.json").exists());
        }

        /// A fresh install (no legacy file at all) never creates a
        /// `config.json.migrated` receipt — there was nothing to migrate.
        #[test]
        fn a_true_first_run_never_writes_a_migration_receipt() {
            let dir = tempfile::tempdir().unwrap();
            let loaded = Config::load(dir.path());
            assert_eq!(loaded.config, Config::default());
            assert!(!dir.path().join("config.json.migrated").exists());
            loaded.config.save(dir.path()).unwrap();
            assert!(!dir.path().join("config.json.migrated").exists());
            assert!(dir.path().join("shared-config.json").exists());
        }
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
        let cfg = load(dir.path());
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
        assert!(load(dir.path()).desktop_integration_prompted);
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
            let cfg = load(dir.path());
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

        let loaded = load(dir.path());
        assert_eq!(
            loaded.providers["claude"].mini_summary_metrics,
            Some(vec!["window:five_hour".into(), "window:weekly".into()])
        );
        assert_eq!(
            loaded.providers["claude"].tray_metric.as_deref(),
            Some("window:weekly")
        );
        // No downgrade mirror any more: `write_split` no longer maintains the
        // pre-v2 `mini_summary_metric` field the way legacy `Config::write`
        // did. That mirror existed so an *older build* reading the same
        // `config.json` could still find a usable single-value field — but an
        // older, pre-split build has no idea `shared-config.json` exists at
        // all, so maintaining the mirror inside it would serve no reader.
        assert_eq!(loaded.providers["claude"].mini_summary_metric, None);
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

        let loaded = load(dir.path());
        assert_eq!(
            loaded.providers.keys().collect::<Vec<_>>(),
            vec![
                &"grok",
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
