<script>
  // Per-account usage-schedule editor for the mobile account rows: a compact
  // seven-weekday toggle set, mirroring the desktop editor in Settings.svelte
  // (the two are deliberately kept in sync). It paces only a weekly window's
  // period marker across the days this account is actually used (ADR-0007);
  // every other window, and the fill, status, alerts and tray gauge, are
  // untouched. All seven on is the default and is identical to the raw
  // calendar marker, so the editor is shown on every account — harmless where
  // there is no weekly window to reshape.
  //
  // `account` is mutated in place. `account.usage_schedule` is guaranteed to
  // exist by the caller (MobileApp seeds it in `newAccount` and backfills it in
  // `syncAccountState`), which is what lets `bind:checked` target it directly.
  let { account = $bindable() } = $props();

  // Display order, with keys matching quota-core's serialized `UsageSchedule`
  // field names so a checked set round-trips through Rust without translation.
  // Same list as Settings.svelte's `WEEKDAYS`.
  const WEEKDAYS = [
    { key: 'monday', label: 'Mon' },
    { key: 'tuesday', label: 'Tue' },
    { key: 'wednesday', label: 'Wed' },
    { key: 'thursday', label: 'Thu' },
    { key: 'friday', label: 'Fri' },
    { key: 'saturday', label: 'Sat' },
    { key: 'sunday', label: 'Sun' },
  ];
</script>

<div class="field schedule-picker">
  <span class="schedule-label">Usage days</span>
  <div class="schedule-toggles">
    {#each WEEKDAYS as day}
      <label class="schedule-day" title={day.label}>
        <input type="checkbox" bind:checked={account.usage_schedule[day.key]} />
        <span>{day.label}</span>
      </label>
    {/each}
  </div>
</div>
