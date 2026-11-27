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

<div class="settings">
  <h1>Ajustes</h1>

  <section>
    <h2>Apariencia</h2>
    <label>Tema
      <select value={s.theme} onchange={(e) => set({ theme: val(e) as DesktopSettings["theme"] })}>
        <option value="system">Automático (sistema)</option>
        <option value="light">Claro</option>
        <option value="dark">Oscuro</option>
      </select>
    </label>
  </section>

  <section>
    <h2>Terminal</h2>
    <label>Fuente
      <input value={s.fontFamily} placeholder="Predeterminada del sistema (p. ej. Fira Code)" onchange={(e) => set({ fontFamily: val(e) })} />
    </label>
    <label>Tamaño de fuente
      <input type="number" min={FONT_MIN} max={FONT_MAX} value={s.fontSize} onchange={(e) => set({ fontSize: num(e) })} />
    </label>
    <label>Cursor
      <select value={s.cursorStyle} onchange={(e) => set({ cursorStyle: val(e) as DesktopSettings["cursorStyle"] })}>
        <option value="block">Bloque</option>
        <option value="underline">Subrayado</option>
        <option value="bar">Barra</option>
      </select>
    </label>
    <label class="check"><input type="checkbox" checked={s.cursorBlink} onchange={(e) => set({ cursorBlink: chk(e) })} /> Cursor parpadeante</label>
    <label>Scrollback (líneas)
      <input type="number" min="0" max={SCROLLBACK_MAX} step="1000" value={s.scrollback} onchange={(e) => set({ scrollback: num(e) })} />
    </label>
    <label class="check"><input type="checkbox" checked={s.copyOnSelect} onchange={(e) => set({ copyOnSelect: chk(e) })} /> Copiar al seleccionar</label>
    <label>Clic derecho
      <select value={s.rightClick} onchange={(e) => set({ rightClick: val(e) as DesktopSettings["rightClick"] })}>
        <option value="menu">Menú contextual</option>
        <option value="paste">Pegar</option>
      </select>
    </label>
  </section>

  <section>
    <h2>Sesiones</h2>
    <label class="check"><input type="checkbox" checked={s.confirmClose} onchange={(e) => set({ confirmClose: chk(e) })} /> Confirmar antes de cerrar una pestaña con sesión activa</label>
  </section>

  <section>
    <h2>Atajos</h2>
    <p class="hint">
      {mac ? "En macOS se usa Cmd." : "Se usa Ctrl+Shift para no chocar con los atajos de la shell."}
      Copiar con {mac ? "⌘C" : "Ctrl+C"} solo si hay selección (si no, se envía ^C); pegar con {mac ? "⌘V" : "Ctrl+V"}.
    </p>
    <dl>
      {#each SHORTCUTS as d (d.action)}
        <dt>{d.label}</dt>
        <dd><kbd>{shortcutLabel(d, mac)}</kbd></dd>
      {/each}
    </dl>
  </section>
</div>

<style>
  .settings { max-width: 640px; display: grid; gap: 20px; align-content: start; }
  h1 { margin: 0; font-size: 20px; }
  h2 { margin: 0 0 8px; font-size: 13px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-dim); }
  section { display: grid; gap: 10px; border-top: 1px solid var(--border); padding-top: 12px; }
  label { display: grid; grid-template-columns: 200px minmax(0, 1fr); align-items: center; gap: 12px; }
  label.check { display: flex; align-items: center; gap: 8px; }
  input[type="number"], select { max-width: 220px; }
  .hint { margin: 0; color: var(--fg-dim); font-size: 12px; }
  dl { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 4px 16px; margin: 0; }
  dt { color: var(--fg); }
  dd { margin: 0; text-align: right; }
  kbd { font-family: var(--font-mono); font-size: 12px; background: var(--bg-elev); border: 1px solid var(--border); border-radius: 4px; padding: 1px 6px; }
</style>
