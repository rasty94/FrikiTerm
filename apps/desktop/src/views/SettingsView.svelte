<script lang="ts">
  import { app, updateSettings } from "../lib/state.svelte";
  import { FONT_MAX, FONT_MIN, SCROLLBACK_MAX } from "../lib/settings";
  import { isMac, SHORTCUTS, shortcutLabel } from "../lib/shortcuts";
  import type { DesktopSettings } from "../lib/types";

  const mac = isMac();
  const s = $derived(app.settings);

  const num = (e: Event) => (e.currentTarget as HTMLInputElement).valueAsNumber;
  const chk = (e: Event) => (e.currentTarget as HTMLInputElement).checked;
  const val = (e: Event) => (e.currentTarget as HTMLInputElement | HTMLSelectElement).value;
  const set = (patch: Partial<DesktopSettings>) => void updateSettings(patch);
</script>

{#snippet seg<T extends string>(label: string, value: T, options: [T, string][], pick: (v: T) => void)}
  <div class="seg" role="radiogroup" aria-label={label}>
    {#each options as [v, text] (v)}
      <button type="button" role="radio" aria-checked={value === v} onclick={() => pick(v)}>{text}</button>
    {/each}
  </div>
{/snippet}

<div class="view settings">
  <header class="view-head">
    <div class="titles">
      <h1>Ajustes</h1>
      <span class="sub">Se guardan al momento y se aplican a todas las sesiones.</span>
    </div>
  </header>

  <section class="panel">
    <h2>Apariencia</h2>
    <div class="row">
      <span class="lbl">Tema</span>
      {@render seg("Tema", s.theme, [["system", "Sistema"], ["light", "Claro"], ["dark", "Oscuro"]], (v) => set({ theme: v }))}
    </div>
  </section>

  <section class="panel">
    <h2>Terminal</h2>
    <label class="row">
      <span class="lbl">Fuente</span>
      <input class="wide" value={s.fontFamily} placeholder="Predeterminada (p. ej. Fira Code)" onchange={(e) => set({ fontFamily: val(e) })} />
    </label>
    <label class="row">
      <span class="lbl">Tamaño de fuente <small>{FONT_MIN}–{FONT_MAX} px</small></span>
      <input class="narrow" type="number" min={FONT_MIN} max={FONT_MAX} value={s.fontSize} onchange={(e) => set({ fontSize: num(e) })} />
    </label>
    <div class="row">
      <span class="lbl">Cursor</span>
      {@render seg("Cursor", s.cursorStyle, [["block", "Bloque"], ["underline", "Subrayado"], ["bar", "Barra"]], (v) => set({ cursorStyle: v }))}
    </div>
    <label class="row">
      <span class="lbl">Cursor parpadeante</span>
      <input type="checkbox" checked={s.cursorBlink} onchange={(e) => set({ cursorBlink: chk(e) })} />
    </label>
    <label class="row">
      <span class="lbl">Scrollback <small>líneas</small></span>
      <input class="narrow" type="number" min="0" max={SCROLLBACK_MAX} step="1000" value={s.scrollback} onchange={(e) => set({ scrollback: num(e) })} />
    </label>
    <label class="row">
      <span class="lbl">Copiar al seleccionar</span>
      <input type="checkbox" checked={s.copyOnSelect} onchange={(e) => set({ copyOnSelect: chk(e) })} />
    </label>
    <div class="row">
      <span class="lbl">Clic derecho</span>
      {@render seg("Clic derecho", s.rightClick, [["menu", "Menú contextual"], ["paste", "Pegar"]], (v) => set({ rightClick: v }))}
    </div>
  </section>

  <section class="panel">
    <h2>Sesiones</h2>
    <label class="row">
      <span class="lbl">Confirmar antes de cerrar una pestaña con sesión activa</span>
      <input type="checkbox" checked={s.confirmClose} onchange={(e) => set({ confirmClose: chk(e) })} />
    </label>
  </section>

  <section class="panel">
    <h2>Atajos de teclado</h2>
    <p class="hint">
      {mac ? "En macOS se usa Cmd." : "Se usa Ctrl+Shift para no chocar con los atajos de la shell."}
      Copiar con {mac ? "⌘C" : "Ctrl+C"} solo si hay selección (si no, se envía ^C); pegar con {mac ? "⌘V" : "Ctrl+V"}.
    </p>
    <dl>
      {#each SHORTCUTS as d (d.action)}
        <div class="row"><dt>{d.label}</dt><dd><kbd>{shortcutLabel(d, mac)}</kbd></dd></div>
      {/each}
    </dl>
  </section>
</div>

<style>
  .settings { max-width: 720px; width: 100%; }
  section { padding: 4px 16px; }
  h2 { font-size: 13.5px; padding: 12px 0 8px; }
  .row { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: 46px; border-top: 1px solid var(--border); }
  h2 + .row { border-top: 0; }
  .lbl { display: grid; gap: 1px; color: var(--fg); }
  .lbl small { color: var(--fg-faint); font-size: 11.5px; }
  .wide { width: 280px; }
  .narrow { width: 96px; text-align: right; font-variant-numeric: tabular-nums; }
  .hint { margin: 0 0 8px; color: var(--fg-dim); font-size: 12.5px; line-height: 1.5; }
  dl { margin: 0; }
  dl .row { min-height: 38px; border-top: 1px solid var(--border); }
  dt { color: var(--fg); }
  dd { margin: 0; }
</style>
