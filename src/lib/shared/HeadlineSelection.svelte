<script>
  // Per-account headline picker: which windows/credits show as the mini
  // summary's headline for this account, plus which one (if any) drives the
  // compact tray-style status. `account` is mutated in place; `options` is the
  // caller-computed list of { id, label } this account can show — computing it
  // needs the live snapshot and the account id, both of which the caller
  // already has, so it stays their responsibility rather than being duplicated
  // here. Only one menu is ever open across a list of accounts, so `open` and
  // `onToggle` are lifted to the caller too.
  let { account = $bindable(), options, open, onToggle } = $props();

  const selectedMetrics = () => account.mini_summary_metrics ?? null;

  function setMetric(metricId, checked) {
    const current = selectedMetrics() ?? [];
    // Unchecking the last one leaves an empty list, not automatic — otherwise
    // "show nothing for this account" would be unreachable.
    account.mini_summary_metrics = checked
      ? [...current, metricId]
      : current.filter((m) => m !== metricId);
    pruneTrayMetric();
  }

  function setAutomatic(checked) {
    // Automatic (`null`) and an empty selection (`[]`) are distinct saved
    // states: unticking it is the explicit "show no headline" choice.
    account.mini_summary_metrics = checked ? null : [];
    pruneTrayMetric();
  }

  // A pinned tray metric that is no longer shown would keep driving the icon
  // from off-screen, so drop it back to "worst of selected".
  function pruneTrayMetric() {
    const chosen = selectedMetrics();
    const pinned = account.tray_metric;
    if (!pinned || pinned === 'none') return;
    if (chosen === null || !chosen.includes(pinned)) account.tray_metric = null;
  }

  const summaryText = $derived.by(() => {
    const chosen = selectedMetrics();
    if (chosen === null) return 'Automatic';
    if (chosen.length === 0) return 'None';
    const names = chosen.map((m) => options.find((o) => o.id === m)?.label ?? m);
    return names.length > 2 ? `${names.length} selected` : names.join(', ');
  });
</script>

<div class="field metric-picker">
  <span>Mini-summary headlines</span>
  <button class="metric-toggle" aria-expanded={open} onclick={onToggle}>{summaryText} ▾</button>
  {#if open}
    <div class="metric-menu">
      <!-- Pinned above the metrics and mutually exclusive with them: automatic
           is a mode, not one more headline. -->
      <label class="metric-item">
        <input
          type="checkbox"
          checked={selectedMetrics() === null}
          onchange={(event) => setAutomatic(event.currentTarget.checked)}
        />
        Automatic
      </label>
      <hr />
      {#each options as metric}
        <label class="metric-item">
          <input
            type="checkbox"
            checked={selectedMetrics()?.includes(metric.id) ?? false}
            onchange={(event) => setMetric(metric.id, event.currentTarget.checked)}
          />
          {metric.label}
        </label>
      {/each}
    </div>
  {/if}
</div>
{#if account.enabled}
  <label class="field">Tray icon status
    <select value={account.tray_metric ?? ''} onchange={(event) => (account.tray_metric = event.currentTarget.value || null)}>
      <option value="">Worst of selected</option>
      <option value="none">None</option>
      {#each options.filter((m) => selectedMetrics()?.includes(m.id)) as metric}
        <option value={metric.id}>{metric.label}</option>
      {/each}
    </select>
  </label>
{/if}
