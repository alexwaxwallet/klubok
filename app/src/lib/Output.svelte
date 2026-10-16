<script lang="ts" module>
  import type { Span } from "./session";

  /** Строка на экране. */
  export interface OutputLine {
    id: number;
    kind: "line" | "prompt" | "info" | "error";
    spans: Span[];
    /** Команда, которую игрок ввёл после этого промпта. */
    echo?: string;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";

  let { lines }: { lines: OutputLine[] } = $props();

  let viewport: HTMLDivElement;
  /** Прокручено до конца — новые строки тянут прокрутку за собой. */
  let stickToBottom = true;

  function onScroll() {
    stickToBottom = viewport.scrollTop + viewport.clientHeight >= viewport.scrollHeight - 8;
  }

  $effect(() => {
    // Перечитываем длину, чтобы эффект срабатывал на каждую новую строку.
    void lines.length;
    if (stickToBottom) {
      tick().then(() => (viewport.scrollTop = viewport.scrollHeight));
    }
  });

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

<div class="output" bind:this={viewport} onscroll={onScroll}>
  {#each lines as line (line.id)}
    <div class="line {line.kind}">
      {#each line.spans as span, i (i)}<span style={spanStyle(span)}>{span.text}</span>{/each}
      {#if line.echo !== undefined}<span class="echo">{line.echo}</span>{/if}
    </div>
  {/each}
</div>

<style>
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

  .line {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-height: 1.3em;
  }

  .info {
    color: var(--muted);
    font-style: italic;
  }

  .error {
    color: var(--danger);
  }

  .echo {
    color: var(--accent);
  }
</style>
