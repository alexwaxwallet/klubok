<script lang="ts" module>
  import type { Span } from "./session";

  /** Строка на экране. */
  export interface OutputLine {
    id: number;
    kind: "line" | "prompt" | "info" | "echo" | "warning" | "error";
    spans: Span[];
    /** Команда, которую игрок ввёл после этого промпта. */
    echo?: string;
  }

  /** Текст строки целиком (для поиска и дополнения слов). */
  export function lineText(line: OutputLine): string {
    return line.spans.map((s) => s.text).join("") + (line.echo ?? "");
  }
</script>

<script lang="ts">
  import { tick } from "svelte";

  let {
    lines,
    split = false,
    onclosesearch,
  }: {
    lines: OutputLine[];
    /** Как в JMC: при прокрутке вверх внизу остаётся окошко с последними строками. */
    split?: boolean;
    /** Поиск закрыт — вернуть фокус в строку ввода. */
    onclosesearch?: () => void;
  } = $props();

  /** Сколько последних строк держать в нижнем окошке. */
  const TAIL_LINES = 60;

  let viewport: HTMLDivElement;
  let tailView: HTMLDivElement | undefined = $state();
  /** Прокручено до конца — новые строки тянут прокрутку за собой. */
  let stickToBottom = $state(true);

  function onScroll() {
    stickToBottom = viewport.scrollTop + viewport.clientHeight >= viewport.scrollHeight - 8;
  }

  $effect(() => {
    // Перечитываем длину, чтобы эффект срабатывал на каждую новую строку.
    void lines.length;
    tick().then(() => {
      if (stickToBottom) viewport.scrollTop = viewport.scrollHeight;
    });
  });

  // Нижнее окошко всегда показывает самый конец: и когда появилось, и на каждую новую строку.
  $effect(() => {
    void tail.length;
    if (tailView) tailView.scrollTop = tailView.scrollHeight;
  });

  // Ширина поменялась (появилась боковая колонка, окно растянули) — строки
  // переносятся иначе, и низ уезжает. Если были внизу — остаёмся внизу.
  $effect(() => {
    const observer = new ResizeObserver(() => {
      if (stickToBottom) viewport.scrollTop = viewport.scrollHeight;
    });
    observer.observe(viewport);
    return () => observer.disconnect();
  });

  let showTail = $derived(split && !stickToBottom);
  let tail = $derived(showTail ? lines.slice(-TAIL_LINES) : []);

  /** Листать на страницу вверх (−1) или вниз (+1). */
  export function page(direction: number) {
    viewport.scrollBy({ top: direction * viewport.clientHeight * 0.9 });
  }

  export function toBottom() {
    viewport.scrollTop = viewport.scrollHeight;
    stickToBottom = true;
  }

  // ───────────── поиск ─────────────

  let searching = $state(false);
  let query = $state("");
  let searchInput: HTMLInputElement | undefined = $state();
  /** id найденной строки. */
  let found: number | null = $state(null);
  let notFound = $state(false);

  export function openSearch() {
    searching = true;
    tick().then(() => {
      searchInput?.focus();
      searchInput?.select();
    });
  }

  function closeSearch() {
    searching = false;
    found = null;
    notFound = false;
    onclosesearch?.();
  }

  /** Следующее совпадение: `older` — вверх (раньше), иначе вниз. */
  function findNext(older: boolean) {
    const q = query.trim().toLowerCase();
    if (!q) return;
    const from = found === null ? lines.length : lines.findIndex((l) => l.id === found);
    const start = from < 0 ? lines.length : from;
    const step = older ? -1 : 1;
    for (let i = start + step; i >= 0 && i < lines.length; i += step) {
      if (lineText(lines[i]).toLowerCase().includes(q)) {
        found = lines[i].id;
        notFound = false;
        tick().then(() =>
          viewport.querySelector(`[data-id="${lines[i].id}"]`)?.scrollIntoView({ block: "center" }),
        );
        return;
      }
    }
    notFound = true;
  }

  function searchKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      findNext(!e.shiftKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeSearch();
    }
  }

  function spanStyle(span: Span): string {
    const color = (c: string) => (c.startsWith("#") ? c : `var(--${c})`);
    let style = "";
    if (span.fg) style += `color:${color(span.fg)};`;
    if (span.bg) style += `background:${color(span.bg)};`;
    if (span.italic) style += "font-style:italic;";
    if (span.underline) style += "text-decoration:underline;";
    return style;
  }
</script>

{#snippet row(line: OutputLine)}
  <!-- Всё в одну строку: в режиме pre-wrap любой перенос в шаблоне стал бы пробелом на экране. -->
  <div class="line {line.kind}" class:found={line.id === found} data-id={line.id}>{#each line.spans as span, i (i)}<span style={spanStyle(span)}>{span.text}</span>{/each}{#if line.echo !== undefined}<span class="typed">{line.echo}</span>{/if}</div>
{/snippet}

<div class="wrap">
  {#if searching}
    <div class="search">
      <input
        bind:this={searchInput}
        bind:value={query}
        placeholder="найти в выводе"
        spellcheck="false"
        onkeydown={searchKey}
        oninput={() => (notFound = false)}
      />
      <button title="Раньше (Enter)" onclick={() => findNext(true)}>▲</button>
      <button title="Позже (Shift+Enter)" onclick={() => findNext(false)}>▼</button>
      {#if notFound}<span class="none">не найдено</span>{/if}
      <button title="Закрыть (Esc)" onclick={closeSearch}>✕</button>
    </div>
  {/if}
  <div class="output" bind:this={viewport} onscroll={onScroll}>
    {#each lines as line (line.id)}
      {@render row(line)}
    {/each}
  </div>
  {#if showTail}
    <button class="divider" title="К концу (Ctrl+End)" onclick={toBottom}>▼ последние строки ▼</button>
    <div class="output tail" bind:this={tailView}>
      {#each tail as line (line.id)}
        {@render row(line)}
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }

  .output {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 8px 12px;
    font-family: var(--mono);
    font-size: var(--font-size);
    line-height: 1.3;
    user-select: text;
  }

  .output.tail {
    flex: 0 0 32%;
    padding-top: 2px;
  }

  .divider {
    flex: none;
    border: none;
    border-top: 1px solid var(--accent);
    background: var(--panel);
    color: var(--muted);
    font-size: 11px;
    padding: 1px 0;
    cursor: pointer;
  }

  .search {
    position: absolute;
    top: 6px;
    right: 18px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    font-size: 12px;
  }

  .search input {
    width: 200px;
    padding: 3px 6px;
    border: 1px solid var(--panel-border);
    border-radius: 3px;
    background: var(--input-bg);
    outline: none;
  }

  .search input:focus {
    border-color: var(--accent);
  }

  .search button {
    min-width: 24px;
    height: 22px;
    border: 1px solid var(--panel-border);
    border-radius: 3px;
    background: var(--input-bg);
    cursor: pointer;
  }

  .search .none {
    color: var(--danger);
  }

  .line {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-height: 1.3em;
  }

  .line.found {
    background: color-mix(in srgb, var(--accent) 30%, transparent);
  }

  .info {
    color: var(--muted);
    font-style: italic;
  }

  .error {
    color: var(--danger);
  }

  .echo {
    color: var(--c6);
  }

  .warning {
    color: var(--c3);
  }

  .typed {
    color: var(--accent);
  }
</style>
