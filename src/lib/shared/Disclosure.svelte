<script>
  // Shared section disclosure (issue #184). Settings renders every top-level
  // section through this: the heading is all that shows until it is opened,
  // so a visit starts on a page of headings instead of a page of scroll.
  //
  // `open` is bindable and `ontoggle` reports each change, so a flow whose
  // result lands inside a section — adding an account, an import, an inbound
  // LAN transfer — can reveal that section from outside it.
  //
  // The heading button reuses the per-account disclosure's styling (and the
  // 44px touch target mobile.css gives it) so every expander in Settings
  // reads and feels the same way.
  let { id, title, open = $bindable(false), ontoggle, children } = $props();

  function toggle() {
    open = !open;
    ontoggle?.(open);
  }
</script>

<section class="disclosure">
  <h2>
    <button
      type="button"
      class="provider-disclosure"
      id="{id}-toggle"
      aria-controls="{id}-panel"
      aria-expanded={open}
      onclick={toggle}
    ><span class="chevron" class:open>▸</span> {title}</button>
  </h2>
  {#if open}
    <div class="disclosure-panel" id="{id}-panel" role="region" aria-labelledby="{id}-toggle">
      {@render children?.()}
    </div>
  {/if}
</section>
