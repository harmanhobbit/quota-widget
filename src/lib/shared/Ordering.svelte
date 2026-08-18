<script>
  // Sort-order/sort-basis picker, shared by every host. `sortOrder` and
  // `sortBasis` mirror `config.sort_order`/`config.sort_basis` and are
  // mutated in place — the caller owns persistence.
  let { sortOrder = $bindable(), sortBasis = $bindable() } = $props();

  const SORT_ORDERS = [
    { id: 'manual', label: 'Manual (my order)' },
    { id: 'usage_desc', label: 'Usage: high to low' },
    { id: 'usage_asc', label: 'Usage: low to high' },
    { id: 'expiry_soonest', label: 'Expiry: soonest first' },
    { id: 'expiry_furthest', label: 'Expiry: furthest first' },
  ];
  const SORT_BASES = [
    { id: 'icon', label: 'the number in the tray icon' },
    { id: 'worst_case', label: 'the worst window' },
  ];
</script>

<div class="row">
  <label class="inline">Order accounts by
    <select bind:value={sortOrder}>
      {#each SORT_ORDERS as order}<option value={order.id}>{order.label}</option>{/each}
    </select>
  </label>
</div>
<!-- The basis chooses which number sorts, so it means nothing while the order
     is the user's own. Disabled rather than hidden: it keeps its saved value,
     and the row does not jump as the order changes. -->
<div class="row">
  <label class="inline" class:disabled={sortOrder === 'manual'}>Sorting on
    <select bind:value={sortBasis} disabled={sortOrder === 'manual'}>
      {#each SORT_BASES as basis}<option value={basis.id}>{basis.label}</option>{/each}
    </select>
  </label>
</div>
<p class="note">Ordering applies everywhere accounts are listed. Accounts with no matching number — a credits-only balance, or an account that isn't in the tray — stay at the bottom in your own order.</p>
