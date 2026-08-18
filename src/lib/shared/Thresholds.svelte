<script>
  // Warn/critical percent editor, shared by every host that lets the user
  // adjust default thresholds. `thresholds` is the same shape as
  // `config.thresholds` ({ warn_pct, critical_pct }) and is mutated in place —
  // the caller owns persistence.
  let { thresholds = $bindable() } = $props();
</script>

<div class="row">
  <label class="inline">Warn at <input type="number" class="num" bind:value={thresholds.warn_pct} />%</label>
  <label class="inline">Critical at <input type="number" class="num" bind:value={thresholds.critical_pct} />%</label>
</div>
