<script lang="ts" module>
  // Вид окна вывода: шрифт, размер, межстрочный интервал. Хранится в браузерном хранилище
  // окна (на этом компьютере), применяется через CSS-переменные `--mono`, `--font-size`, `--line-height`.

  export interface ViewPrefs {
    font: string;
    size: number;
    lineHeight: number;
  }

  const KEY = "klubok.view";

  export const FONTS = [
    { value: "", label: "по умолчанию" },
    { value: '"Cascadia Mono"', label: "Cascadia Mono" },
    { value: "Consolas", label: "Consolas" },
    { value: '"Lucida Console"', label: "Lucida Console" },
    { value: '"Courier New"', label: "Courier New" },
    { value: '"DejaVu Sans Mono"', label: "DejaVu Sans Mono" },
  ];

  const DEFAULTS: ViewPrefs = { font: "", size: 15, lineHeight: 1.3 };

  export function loadPrefs(): ViewPrefs {
    try {
      const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
      return { ...DEFAULTS, ...saved };
    } catch {
      return { ...DEFAULTS };
    }
  }

  /** Применить к странице и запомнить. */
  export function applyPrefs(p: ViewPrefs) {
    const root = document.documentElement.style;
    root.setProperty("--font-size", `${p.size}px`);
    root.setProperty("--line-height", String(p.lineHeight));
    if (p.font) root.setProperty("--mono", `${p.font}, monospace`);
    else root.removeProperty("--mono");
    try {
      localStorage.setItem(KEY, JSON.stringify(p));
    } catch {
      // без сохранения — не страшно
    }
  }

  /** Ctrl+= / Ctrl+− : крупнее/мельче. */
  export function changeSize(delta: number) {
    const p = loadPrefs();
    p.size = Math.min(28, Math.max(10, p.size + delta));
    applyPrefs(p);
  }
</script>

<script lang="ts">
  let { onclose }: { onclose: () => void } = $props();

  let prefs = $state(loadPrefs());

  $effect(() => {
    applyPrefs({ font: prefs.font, size: prefs.size, lineHeight: prefs.lineHeight });
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="popover" onkeydown={(e) => e.key === "Escape" && onclose()}>
  <label>
    Шрифт
    <select bind:value={prefs.font}>
      {#each FONTS as f (f.value)}
        <option value={f.value}>{f.label}</option>
      {/each}
    </select>
  </label>
  <label>
    Размер
    <span class="row">
      <button onclick={() => (prefs.size = Math.max(10, prefs.size - 1))}>−</button>
      <b>{prefs.size}</b>
      <button onclick={() => (prefs.size = Math.min(28, prefs.size + 1))}>+</button>
    </span>
  </label>
  <label>
    Между строк
    <input type="range" min="1" max="1.8" step="0.05" bind:value={prefs.lineHeight} />
  </label>
  <p class="hint">Ctrl + и Ctrl − тоже меняют размер.</p>
  <button class="close" onclick={onclose}>Готово</button>
</div>

<style>
  .popover {
    position: absolute;
    top: 44px;
    right: 8px;
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 270px;
    padding: 10px;
    background: var(--panel);
    border: 1px solid var(--panel-border);
    border-radius: 6px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
    font-size: 13px;
  }

  label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  select,
  input[type="range"] {
    width: 130px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .row button {
    width: 26px;
  }

  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 11px;
  }

  .close {
    align-self: flex-end;
  }
</style>
