<script lang="ts">
  // Редактор алиасов, триггеров, переменных и прочего. Работает с текстом настроек JMC:
  // разбирает его на строки-поля, а при сохранении собирает обратно.
  import { onMount } from "svelte";
  import { KINDS, balanced, build, newItem, parse, type Item, type Parsed } from "./automation";
  import { getAutomation, setAutomation } from "./session";

  let {
    profileId,
    profileName,
    onclose,
  }: { profileId: string; profileName: string; onclose: () => void } = $props();

  const TEXT_TAB = "text";
  const OTHER_TAB = "other";

  let parsed: Parsed = $state({ items: [], other: "" });
  let tab = $state("alias");
  let filter = $state("");
  /** Текст на вкладке «Весь текст» (разбирается при уходе с неё). */
  let rawText = $state("");
  let loaded = $state(false);
  let error = $state("");
  let busy = $state(false);

  onMount(async () => {
    try {
      parsed = parse(await getAutomation(profileId));
      loaded = true;
    } catch (e) {
      error = String(e);
    }
  });

  function count(kind: string): number {
    return parsed.items.filter((i) => i.kind === kind).length;
  }

  let kind = $derived(KINDS.find((k) => k.id === tab));
  let shown = $derived.by(() => {
    const f = filter.trim().toLowerCase();
    return parsed.items.filter(
      (i) => i.kind === tab && (f === "" || i.values.some((v) => v.toLowerCase().includes(f))),
    );
  });
  let broken = $derived(parsed.items.filter((i) => !i.values.every(balanced)));

  function switchTab(next: string) {
    if (tab === TEXT_TAB) parsed = parse(rawText);
    if (next === TEXT_TAB) rawText = build(parsed);
    tab = next;
  }

  function add() {
    if (!kind) return;
    const item = newItem(kind);
    parsed.items.push(item);
    filter = "";
    // Фокус на первое поле новой строки.
    queueMicrotask(() =>
      document.querySelector<HTMLInputElement>(`[data-key="${item.key}"] input`)?.focus(),
    );
  }

  function remove(item: Item) {
    parsed.items = parsed.items.filter((i) => i.key !== item.key);
  }

  async function save() {
    if (tab === TEXT_TAB) parsed = parse(rawText);
    if (broken.length > 0) {
      error = "Непарные фигурные скобки в строках, отмеченных красным.";
      return;
    }
    busy = true;
    try {
      await setAutomation(profileId, build($state.snapshot(parsed)));
      onclose();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-label="Настройки автоматизации">
    <header>
      <h2>Автоматизация — {profileName}</h2>
      <button class="close" title="Закрыть без сохранения" onclick={onclose}>✕</button>
    </header>

    <nav>
      {#each KINDS as k (k.id)}
        <button class:active={tab === k.id} onclick={() => switchTab(k.id)}>
          {k.title} <span class="n">{count(k.id)}</span>
        </button>
      {/each}
      <button class:active={tab === OTHER_TAB} onclick={() => switchTab(OTHER_TAB)}>Прочее</button>
      <button class:active={tab === TEXT_TAB} onclick={() => switchTab(TEXT_TAB)}>Весь текст</button>
    </nav>

    <div class="body">
      {#if !loaded && !error}
        <p class="hint">Загрузка…</p>
      {:else if kind}
        <div class="tools">
          <input class="filter" placeholder="Поиск" bind:value={filter} />
          <button onclick={add}>＋ Добавить</button>
        </div>
        <div class="table" style:--cols={kind.fields.map((_, i) => (i === kind.wide ? "3fr" : "1fr")).join(" ")}>
          <div class="row head">
            {#each kind.fields as f (f)}<span>{f}</span>{/each}
            <span></span>
          </div>
          {#each shown as item (item.key)}
            <div class="row" class:broken={!item.values.every(balanced)} data-key={item.key}>
              {#each kind.fields as f, i (f)}
                <input bind:value={item.values[i]} aria-label={f} spellcheck="false" />
              {/each}
              <button class="del" title="Удалить" onclick={() => remove(item)}>✕</button>
            </div>
          {:else}
            <p class="hint">{filter ? "Ничего не найдено." : "Пусто."}</p>
          {/each}
        </div>
      {:else if tab === OTHER_TAB}
        <p class="hint">
          Группы, настройки JMC и команды, которые редактор не разбирает на поля. По одной на строку.
        </p>
        <textarea bind:value={parsed.other} spellcheck="false"></textarea>
      {:else}
        <p class="hint">Все настройки текстом JMC, по одной команде на строку.</p>
        <textarea bind:value={rawText} spellcheck="false"></textarea>
      {/if}
    </div>

    {#if error}<p class="error">{error}</p>{/if}
    <footer>
      <span class="hint">Параметры: %0…%9, переменные: $имя. Шаблон /…/ — регулярное выражение.</span>
      <span class="spacer"></span>
      <button onclick={onclose}>Отмена</button>
      <button class="primary" disabled={busy || !loaded} onclick={save}>Сохранить</button>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }

  .dialog {
    width: min(1000px, 100vw - 24px);
    height: min(720px, 100dvh - 24px);
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--panel-border);
    border-radius: 6px;
    color: var(--text);
  }

  header {
    display: flex;
    align-items: center;
    padding: 10px 14px 6px;
  }

  h2 {
    flex: 1;
    margin: 0;
    font-size: 16px;
  }

  button {
    padding: 4px 10px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    color: var(--text);
    cursor: pointer;
  }

  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #1a1308;
    font-weight: 600;
  }

  button:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .close {
    border: none;
    background: none;
  }

  nav {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 0 14px 8px;
    border-bottom: 1px solid var(--panel-border);
  }

  nav button.active {
    border-color: var(--accent);
    color: var(--accent);
  }

  .n {
    color: var(--muted);
    font-size: 11px;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 8px 14px;
    gap: 6px;
  }

  .tools {
    display: flex;
    gap: 6px;
  }

  input,
  textarea {
    padding: 4px 6px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    color: var(--text);
    font-family: var(--mono);
    font-size: 13px;
    min-width: 0;
  }

  .filter {
    flex: 1;
  }

  .table {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .row {
    display: grid;
    grid-template-columns: var(--cols) 28px;
    gap: 4px;
    margin-bottom: 3px;
  }

  .row.head {
    position: sticky;
    top: 0;
    background: var(--panel);
    color: var(--muted);
    font-size: 12px;
    padding-bottom: 2px;
  }

  .row.broken input {
    border-color: var(--danger);
  }

  .del {
    padding: 0;
    border: none;
    background: none;
    color: var(--muted);
  }

  .del:hover {
    color: var(--danger);
  }

  textarea {
    flex: 1;
    resize: none;
    white-space: pre;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px 12px;
    border-top: 1px solid var(--panel-border);
  }

  .spacer {
    flex: 1;
  }

  .hint {
    color: var(--muted);
    font-size: 12px;
    margin: 0;
  }

  .error {
    color: var(--danger);
    margin: 0 14px;
  }

  @media (max-width: 640px) {
    .row {
      grid-template-columns: 1fr;
      padding-bottom: 6px;
      border-bottom: 1px solid var(--panel-border);
    }

    .row.head {
      display: none;
    }

    footer .hint {
      display: none;
    }
  }
</style>
