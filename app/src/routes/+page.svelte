<script lang="ts">
  import { onMount } from "svelte";
  import Output, { type OutputLine } from "$lib/Output.svelte";
  import CommandInput from "$lib/CommandInput.svelte";
  import {
    connect,
    disconnect,
    onSession,
    plainText,
    send,
    type Exit,
    type SessionEvent,
  } from "$lib/session";

  /** Сколько строк держать на экране (дальше — старые выбрасываются). */
  const SCROLLBACK = 5000;
  const SETTINGS_KEY = "klubok.connection";

  /** Выходы из MSDP: подпись на кнопке и команда, которую она отправляет. */
  const DIRECTIONS: Record<string, { label: string; command: string }> = {
    n: { label: "С", command: "север" },
    e: { label: "В", command: "восток" },
    s: { label: "Ю", command: "юг" },
    w: { label: "З", command: "запад" },
    u: { label: "↑", command: "вверх" },
    d: { label: "↓", command: "вниз" },
  };

  interface Room {
    vnum: string;
    name: string;
    area: string;
    exits: Exit[];
  }

  // $state.raw: массив заменяется целиком, Svelte не следит за каждым элементом — так быстрее.
  let lines: OutputLine[] = $state.raw([]);
  let connected = $state(false);
  let room: Room | null = $state(null);
  let host = $state("127.0.0.1");
  let port = $state(4000);
  let charset = $state("utf8");
  let nextId = 0;
  let commandInput: CommandInput;

  /** Последний промпт спрашивает пароль. */
  let askingPassword = $derived.by(() => {
    const last = lines[lines.length - 1];
    return last?.kind === "prompt" && /парол|password/i.test(plainText(last.spans));
  });

  /** Последний промпт спрашивает имя: на него можно ответить «Имя Пароль» одной строкой. */
  let askingName = $derived.by(() => {
    const last = lines[lines.length - 1];
    return last?.kind === "prompt" && /введите имя/i.test(plainText(last.spans));
  });

  function append(newLines: OutputLine[]) {
    const all = lines.concat(newLines);
    lines = all.length > SCROLLBACK ? all.slice(all.length - SCROLLBACK) : all;
  }

  function note(kind: "info" | "error", text: string) {
    append([{ id: nextId++, kind, spans: [{ text }] }]);
  }

  function handleBatch(batch: SessionEvent[]) {
    const fresh: OutputLine[] = [];
    for (const event of batch) {
      switch (event.type) {
        case "line":
        case "prompt":
          fresh.push({ id: nextId++, kind: event.type, spans: event.spans });
          break;
        case "room":
          room = event;
          break;
        case "error":
          fresh.push({ id: nextId++, kind: "error", spans: [{ text: event.text }] });
          connected = false;
          break;
        case "disconnected":
          fresh.push({ id: nextId++, kind: "info", spans: [{ text: "Соединение закрыто." }] });
          connected = false;
          break;
        case "info":
          break; // служебные сообщения пока не показываем
      }
    }
    if (fresh.length > 0) append(fresh);
  }

  async function doConnect() {
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify({ host, port, charset }));
    } catch {
      // хранилище недоступно — не страшно
    }
    note("info", `Подключаюсь к ${host}:${port}…`);
    room = null;
    try {
      await connect(host, port, charset);
      connected = true;
      commandInput.focus();
    } catch (error) {
      note("error", String(error));
    }
  }

  async function doDisconnect() {
    await disconnect();
  }

  async function submit(text: string) {
    if (!connected) {
      note("error", "Нет подключения.");
      return;
    }
    // Показываем введённое после промпта (пароль — звёздочками).
    let shown = text;
    if (askingPassword) {
      shown = "*".repeat(text.length);
    } else if (askingName) {
      // «Имя Пароль» — прячем всё после имени.
      shown = text.replace(/^(\S+)\s+(.+)$/, (_, name, pass) => `${name} ${"*".repeat(pass.length)}`);
    }
    const last = lines[lines.length - 1];
    if (last?.kind === "prompt" && last.echo === undefined) {
      lines = [...lines.slice(0, -1), { ...last, echo: shown }];
    } else {
      append([{ id: nextId++, kind: "line", spans: [], echo: shown }]);
    }
    try {
      await send(text);
    } catch (error) {
      note("error", String(error));
    }
  }

  /** Любая буква, набранная вне поля ввода, уходит в поле ввода. */
  function onWindowKeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (target.closest("input, select, button")) return;
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key.length === 1 || event.key === "Enter" || event.key.startsWith("Arrow")) {
      commandInput.focus();
    }
  }

  onMount(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "null");
      if (saved) ({ host, port, charset } = saved);
    } catch {
      // нет сохранённых настроек
    }
    const unlisten = onSession(handleBatch);
    commandInput.focus();
    return () => {
      unlisten.then((stop) => stop());
    };
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div class="app">
  <header>
    <span class="logo">Клубок</span>
    <form
      class="connection"
      onsubmit={(e) => {
        e.preventDefault();
        if (connected) doDisconnect();
        else doConnect();
      }}
    >
      <input class="host" bind:value={host} disabled={connected} aria-label="Адрес" />
      <input
        class="port"
        type="number"
        bind:value={port}
        disabled={connected}
        aria-label="Порт"
      />
      <select bind:value={charset} disabled={connected} aria-label="Кодировка">
        <option value="utf8">UTF-8</option>
        <option value="koi8r">KOI8-R</option>
        <option value="cp1251">CP1251</option>
      </select>
      <button type="submit" class:active={connected}>
        {connected ? "Отключиться" : "Подключиться"}
      </button>
    </form>
  </header>

  <main>
    <section class="terminal">
      <Output {lines} />
      <CommandInput
        bind:this={commandInput}
        onsubmit={submit}
        password={askingPassword}
        sensitive={askingName}
      />
    </section>

    {#if room}
      <aside class="room">
        <div class="room-name">{room.name}</div>
        <div class="room-meta">[{room.vnum}] {room.area}</div>
        <div class="exits">
          {#each room.exits as exit (exit.dir)}
            <button
              class="exit"
              title="{exit.dir} → {exit.vnum}"
              onclick={() => submit(DIRECTIONS[exit.dir]?.command ?? exit.dir)}
              >{DIRECTIONS[exit.dir]?.label ?? exit.dir}</button
            >
          {/each}
        </div>
      </aside>
    {/if}
  </main>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
    height: 100dvh;
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 10px;
    background: var(--panel);
    border-bottom: 1px solid var(--panel-border);
    flex-wrap: wrap;
  }

  .logo {
    font-weight: 600;
    color: var(--accent);
    letter-spacing: 0.02em;
  }

  .connection {
    display: flex;
    gap: 6px;
    flex: 1;
    min-width: 0;
    justify-content: flex-end;
  }

  .connection input,
  .connection select,
  .connection button {
    padding: 4px 8px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
  }

  .host {
    width: 160px;
    min-width: 0;
    flex-shrink: 1;
  }

  .port {
    width: 72px;
  }

  .connection button {
    cursor: pointer;
    background: var(--accent);
    color: #1a1308;
    border-color: var(--accent);
    font-weight: 600;
  }

  .connection button.active {
    background: var(--input-bg);
    color: var(--text);
    border-color: var(--panel-border);
  }

  main {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .terminal {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .room {
    width: 240px;
    padding: 12px;
    border-left: 1px solid var(--panel-border);
    background: var(--panel);
    overflow-y: auto;
  }

  .room-name {
    font-weight: 600;
    color: var(--c14);
    margin-bottom: 4px;
  }

  .room-meta {
    color: var(--muted);
    font-size: 12px;
    margin-bottom: 12px;
  }

  .exits {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .exit {
    min-width: 36px;
    padding: 6px 8px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    cursor: pointer;
    font-family: var(--mono);
  }

  .exit:hover {
    border-color: var(--accent);
  }

  /* Узкий экран (телефон): панель комнаты под выводом, поля подключения сжимаются. */
  @media (max-width: 640px) {
    main {
      flex-direction: column;
    }

    .room {
      width: auto;
      border-left: none;
      border-top: 1px solid var(--panel-border);
      max-height: 30vh;
    }

    .host {
      width: 110px;
    }

    .port {
      width: 60px;
    }
  }
</style>
