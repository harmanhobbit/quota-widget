<script>
  // The [[retention policy]] control for [[usage history]], shared by the
  // desktop History tab and the Android app. One flow, one contract:
  //
  // - Applying a tighter bound FIRST previews — the host's pure
  //   preview_prune answers, and only when it reports readings that would go
  //   does the [[cleaning preview]] dialog appear, showing the exact count,
  //   time span and (for a file-size bound) bytes.
  // - Confirming is the only path to `onapply`. Cancelling is inert: nothing
  //   was persisted, nothing was deleted, and the dialog simply goes away.
  // - A change to *forever* cleans nothing by definition, so it applies
  //   straight away with no preview at all; a bound change the store answers
  //   with an empty preview applies the same way (it, too, would clean
  //   nothing).
  //
  // The policy crosses the boundary in the serde shape quota-core
  // serializes: `"forever"`, `{ age: { unit, amount } }` or
  // `{ file_size: { bytes } }`. The file-size picker counts megabytes — what
  // a person chooses — and converts at the boundary.
  //
  // [[retention policy]]: ../../CONTEXT.md
  // [[usage history]]: ../../CONTEXT.md
  // [[cleaning preview]]: ../../CONTEXT.md

  let { policy = 'forever', onpreview, onapply, onpreviewopenchange } = $props();

  const UNITS = ['days', 'weeks', 'months', 'years'];

  // The editable draft, normalized from the saved policy's serde shape.
  function fromPolicy(saved) {
    if (saved && typeof saved === 'object' && saved.age) {
      return {
        mode: 'age',
        unit: saved.age.unit ?? 'days',
        amount: saved.age.amount ?? 30,
        mb: 50,
      };
    }
    if (saved && typeof saved === 'object' && saved.file_size) {
      return {
        mode: 'size',
        unit: 'days',
        amount: 30,
        mb: Math.max(1, Math.round((saved.file_size.bytes ?? 0) / (1024 * 1024))),
      };
    }
    return { mode: 'forever', unit: 'days', amount: 30, mb: 50 };
  }

  function toPolicy(draft) {
    if (draft.mode === 'age') {
      return { age: { unit: draft.unit, amount: Math.max(0, Number(draft.amount) || 0) } };
    }
    if (draft.mode === 'size') {
      const mb = Math.max(1, Math.round(Number(draft.mb) || 0));
      return { file_size: { bytes: mb * 1024 * 1024 } };
    }
    return 'forever';
  }

  // Seeding the draft from the policy as of mount is deliberate — the
  // initializer only needs that initial value, and the effect below owns
  // reactivity, re-normalising the draft whenever the saved policy changes.
  // svelte-ignore state_referenced_locally
  let draft = $state(fromPolicy(policy));
  // The open confirmation, holding the FROZEN policy together with the
  // figures that were shown for it — `null` when no confirmation is due.
  let preview = $state(null);
  let busy = $state(false);
  let error = $state('');
  let previewEl = $state(null);

  // The controls describe the persisted policy: when it changes — applied
  // here, or reloaded from the host's `config` broadcast — the draft follows.
  // The effect reads only `policy`, so it can never loop on its own write.
  $effect(() => {
    draft = fromPolicy(policy);
  });

  // While the confirmation is up, nothing editable may move under it — and
  // the dialog takes focus so a keyboard user lands inside the decision
  // they're being asked to make.
  let locked = $derived(busy || preview !== null);

  $effect(() => {
    if (preview) {
      previewEl?.focus();
    }
  });

  // The parent/child Escape seam: while the destructive confirmation is on
  // screen the shell's own Escape meanings must yield to this dialog's
  // cancel. The shell listener may be registered BEFORE this component even
  // mounts (desktop App registers at startup), so the yield travels by state
  // through this callback — not by listener ordering. The cleanup closes the
  // window when the dialog goes away OR the tab unmounts mid-confirmation.
  $effect(() => {
    onpreviewopenchange?.(preview !== null);
    return () => onpreviewopenchange?.(false);
  });

  async function applyNow(wanted) {
    error = '';
    busy = true;
    try {
      await onapply(wanted);
    } catch (e) {
      error = String(e?.message ?? e);
    }
    busy = false;
  }

  async function requestApply() {
    error = '';
    const wanted = toPolicy(draft);
    // Forever cleans nothing by definition — no preview, no confirmation.
    if (wanted === 'forever') {
      await applyNow(wanted);
      return;
    }
    busy = true;
    try {
      const result = await onpreview(wanted);
      if (result && result.removed_count > 0) {
        // The policy is captured WITH its figures and stays frozen for the
        // confirmation: confirming applies exactly the policy that was
        // previewed, never whatever the draft controls say by the time the
        // click lands. The controls are locked (below) while the dialog is
        // open; the capture is what makes the guarantee hold even so.
        preview = { policy: wanted, figures: result };
      } else {
        // The store answers that this bound would clean nothing: applying
        // needs no gate, exactly like a loosening change.
        await onapply(wanted);
      }
    } catch (e) {
      error = String(e?.message ?? e);
    }
    busy = false;
  }

  async function confirmApply() {
    const wanted = preview?.policy;
    preview = null;
    if (wanted != null) {
      await applyNow(wanted);
    }
  }

  // Cancel is inert by contract: the preview computed nothing on disk and
  // changed nothing in the store, so discarding it is exactly that.
  function cancelPreview() {
    preview = null;
  }

  // Escape over an open preview is the cancel gesture, and must not fall
  // through to the shell's other Escape meanings (hiding the window). The
  // handler is registered before the shells' own window listeners, so
  // stopping it here ends the event's journey.
  function onKeydown(event) {
    if (event.key === 'Escape' && preview) {
      event.stopImmediatePropagation();
      cancelPreview();
    }
  }

  function formatBytes(bytes) {
    if (bytes == null) return '';
    if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${bytes} B`;
  }

  function formatWhen(at) {
    return at ? new Date(at).toLocaleString() : '';
  }

  // The exact figures the confirmation shows: how many points, what time
  // span, and — for a file-size bound — how many bytes. When nothing would
  // remain under the new bound, say that plainly rather than leaving an
  // empty "the kept record begins …" clause.
  let previewText = $derived(
    preview
      ? `Applying this removes ${preview.figures.removed_count} recorded reading${preview.figures.removed_count === 1 ? '' : 's'}` +
        (preview.figures.removed_bytes != null ? ` (${formatBytes(preview.figures.removed_bytes)})` : '') +
        (preview.figures.oldest_kept != null
          ? `. The kept record begins ${formatWhen(preview.figures.oldest_kept)}`
          : '. No readings will remain') +
        (preview.figures.newest_removed != null
          ? `; the newest reading removed is ${formatWhen(preview.figures.newest_removed)}.`
          : '.')
      : ''
  );
</script>

<svelte:window onkeydown={onKeydown} />

<div class="history-retention">
  <label>Keep history
    <select class="retention-mode" bind:value={draft.mode} disabled={locked}>
      <option value="forever">Forever</option>
      <option value="age">By age</option>
      <option value="size">By file size</option>
    </select>
  </label>
  {#if draft.mode === 'age'}
    <label>for
      <input class="num retention-amount" type="number" min="0" bind:value={draft.amount} disabled={locked} />
      <select class="retention-unit" bind:value={draft.unit} disabled={locked}>
        {#each UNITS as unit (unit)}
          <option value={unit}>{unit}</option>
        {/each}
      </select>
    </label>
  {:else if draft.mode === 'size'}
    <label>up to
      <input class="num retention-mb" type="number" min="1" bind:value={draft.mb} disabled={locked} />
      MB
    </label>
  {/if}
  <button class="retention-apply" disabled={locked} onclick={requestApply}>Apply</button>
  {#if preview}
    <div
      class="cleaning-preview"
      role="alertdialog"
      aria-modal="true"
      aria-label="Cleaning preview"
      tabindex="-1"
      bind:this={previewEl}
    >
      <p>{previewText}</p>
      <p class="cleaning-warning">This deletes the recorded readings above. It cannot be undone.</p>
      <div class="cleaning-actions">
        <button class="cleaning-cancel" onclick={cancelPreview}>Cancel</button>
        <button class="cleaning-confirm" onclick={confirmApply}>Clean history</button>
      </div>
    </div>
  {/if}
  {#if error}<p class="note">{error}</p>{/if}
</div>
