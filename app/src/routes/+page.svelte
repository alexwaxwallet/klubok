<script lang="ts">
  import { onMount } from "svelte";
  import Output, { lineText, type OutputLine } from "$lib/Output.svelte";
  import CommandInput from "$lib/CommandInput.svelte";
  import ProfileDialog from "$lib/ProfileDialog.svelte";
  import MapPanel from "$lib/MapPanel.svelte";
  import StatusPanel from "$lib/StatusPanel.svelte";
  import AutomationEditor from "$lib/AutomationEditor.svelte";
  import { describeEvent, eventKeyName, normalizeSpec } from "$lib/hotkeys";
  import {
    connect,
    disconnect,
    listProfiles,
    onSession,
    plainText,
    send,
    type CharStatus,
    type FoundRoom,
    type Exit,
    type Hotkey,
    type MapView,
    mapEdit,
    type ProfileInfo,
    type SessionEvent,
  } from "$lib/session";

  /** Сколько строк держать на экране (дальше — старые выбрасываются). */
  const SCROLLBACK = 5000;
  const LAST_PROFILE_KEY = "klubok.lastProfile";

  /** Выходы из MSDP: подпись на кнопке и команда, которую она отправляет. */
  const DIRECTIONS: Record<string, { label: string; command: string }> = {
    n: { label: "С", command: "север" },
    e: { label: "В", command: "восток" },
    s: { label: "Ю", command: "юг" },
    w: { label: "З", command: "запад" },
    u: { label: "↑", command: "вверх" },
    d: { label: "↓", command: "вниз" },
  };

  /** Цифровой блок без своих настроек ходит, как в JMC. */
  const DEFAULT_HOTKEYS: Record<string, string> = {
    NUM8: "север",
    NUM2: "юг",
    NUM4: "запад",
    NUM6: "восток",
    NUM9: "вверх",
    NUM3: "вниз",
    NUM5: "смотреть",
  };

  /** Клавиши, которые панель карты забирает себе в режиме правки. */
  const MAP_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Delete", "Escape", "a", "A", "ф", "Ф"]);

  /** Сколько строк хранить в окне вывода. */
  const WINDOW_SCROLLBACK = 500;

  /** Окно вывода JMC (`#woutput`). */
  interface OutWindow {
    name: string;
    lines: OutputLine[];
    visible: boolean;
  }

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
  let mapView: MapView | null = $state.raw(null);
  let status: CharStatus | null = $state.raw(null);
  /** Результат поиска комнат в панели карты (`null` — не искали). */
  let mapFound: FoundRoom[] | null = $state.raw(null);
  let profiles: ProfileInfo[] = $state([]);
  let passwordStorage = $state("");
  let selectedId = $state("");
  /** Открытый диалог профиля: undefined — закрыт, null — новый профиль. */
  let editing: ProfileInfo | null | undefined = $state(undefined);
  /** Открыт редактор автоматизации выбранного профиля. */
  let editingAutomation = $state(false);
  /** Горячие клавиши: каноническое имя (`Ctrl+NUM7`) → команды. */
  let hotkeys = new Map<string, string>(Object.entries(DEFAULT_HOTKEYS));
  /** Окна вывода 0…9. */
  let windows: OutWindow[] = $state(
    Array.from({ length: 10 }, () => ({ name: "", lines: [], visible: false })),
  );
  let shownWindows = $derived(
    windows.map((w, i) => ({ ...w, index: i })).filter((w) => w.visible),
  );
  /** Режим #keytest: показывать, что пришло от каждой нажатой клавиши. */
  let keytest = false;
  /** До какого момента глушить ввод символа после Alt+цифровой блок (Alt-коды Windows). */
  let swallowInputUntil = 0;
  let nextId = 0;
  let commandInput: CommandInput;
  let output: Output;

  /** Слова из последних строк вывода для дополнения по Tab (сначала свежие). */
  function completeWord(prefix: string): string[] {
    const p = prefix.toLowerCase();
    const lower = prefix === p;
    const seen = new Set<string>();
    for (let i = lines.length - 1; i >= 0 && i >= lines.length - 500; i--) {
      const words = lineText(lines[i]).split(/[^\p{L}\p{N}-]+/u).reverse();
      for (const w of words) {
        if (w.length <= prefix.length || !w.toLowerCase().startsWith(p)) continue;
        seen.add(lower ? w.toLowerCase() : w);
      }
    }
    return [...seen];
  }
  let audio: AudioContext | null = null;

  let selected = $derived(profiles.find((p) => p.id === selectedId));

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

  function note(kind: OutputLine["kind"], text: string) {
    append(text.split("\n").map((t) => ({ id: nextId++, kind, spans: [{ text: t }] })));
  }

  function setHotkeys(list: Hotkey[]) {
    hotkeys = new Map(Object.entries(DEFAULT_HOTKEYS));
    for (const h of list) {
      const name = normalizeSpec(h.key);
      if (!name) continue;
      hotkeys.set(name, h.body);
      // «Ctrl+RETURN» в JMC — любой Enter с Ctrl; голый RETURN — только Enter цифрового блока,
      // потому что обычный Enter отправляет команду.
      if (name.endsWith("RETURN") && name !== "RETURN") {
        hotkeys.set(name.replace(/RETURN$/, "ENTER"), h.body);
      }
    }
  }

  function bell() {
    try {
      audio ??= new AudioContext();
      const osc = audio.createOscillator();
      const gain = audio.createGain();
      osc.frequency.value = 880;
      gain.gain.setValueAtTime(0.15, audio.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.001, audio.currentTime + 0.25);
      osc.connect(gain).connect(audio.destination);
      osc.start();
      osc.stop(audio.currentTime + 0.25);
    } catch {
      // звук недоступен — не страшно
    }
  }

  function handleBatch(batch: SessionEvent[]) {
    const fresh: OutputLine[] = [];
    const add = (kind: OutputLine["kind"], text: string) => {
      for (const t of text.split("\n")) fresh.push({ id: nextId++, kind, spans: [{ text: t }] });
    };
    for (const event of batch) {
      switch (event.type) {
        case "line":
        case "prompt":
          fresh.push({ id: nextId++, kind: event.type, spans: event.spans });
          break;
        case "room":
          room = event;
          break;
        case "map":
          mapView = event;
          break;
        case "status":
          status = event;
          break;
        case "mapfound":
          mapFound = event.rooms;
          break;
        case "woutput": {
          const w = windows[event.window];
          const all = w.lines.concat([{ id: nextId++, kind: "line", spans: event.spans }]);
          w.lines = all.length > WINDOW_SCROLLBACK ? all.slice(-WINDOW_SCROLLBACK) : all;
          w.visible = true;
          break;
        }
        case "wname":
          windows[event.window].name = event.name;
          break;
        case "wshow": {
          const w = windows[event.window];
          w.visible = event.mode === "toggle" ? !w.visible : event.mode === "show";
          break;
        }
        case "wclear":
          windows[event.window].lines = [];
          break;
        case "echo":
          add("echo", event.text);
          break;
        case "warning":
          add("warning", event.text);
          break;
        case "error":
          add("error", event.text);
          connected = false;
          break;
        case "bell":
          bell();
          break;
        case "hotkeys":
          setHotkeys(event.hotkeys);
          break;
        case "disconnected":
          add("info", "Соединение закрыто.");
          connected = false;
          break;
        case "info":
          break; // служебные сообщения пока не показываем
      }
    }
    if (fresh.length > 0) append(fresh);
  }

  async function refreshProfiles() {
    try {
      const list = await listProfiles();
      profiles = list.profiles;
      passwordStorage = list.password_storage;
      if (!profiles.some((p) => p.id === selectedId)) {
        selectedId = profiles[0]?.id ?? "";
      }
    } catch (error) {
      note("error", String(error));
    }
  }

  async function doConnect() {
    if (!selected) {
      editing = null;
      return;
    }
    try {
      localStorage.setItem(LAST_PROFILE_KEY, selectedId);
    } catch {
      // хранилище недоступно — не страшно
    }
    const { profile } = selected;
    note("info", `Подключаюсь к ${profile.host}:${profile.port} (${profile.name})…`);
    room = null;
    mapView = null;
    status = null;
    try {
      await connect(selectedId);
      connected = true;
      commandInput.focus();
    } catch (error) {
      note("error", String(error));
    }
  }

  /** Показывает введённое после промпта. */
  function echo(shown: string) {
    const last = lines[lines.length - 1];
    if (last?.kind === "prompt" && last.echo === undefined) {
      lines = [...lines.slice(0, -1), { ...last, echo: shown }];
    } else {
      append([{ id: nextId++, kind: "line", spans: [], echo: shown }]);
    }
  }

  async function submit(text: string) {
    if (text.trim().toLowerCase() === "#keytest") {
      keytest = !keytest;
      note(
        "echo",
        keytest
          ? "Проверка клавиш включена: нажимайте клавиши, повторите #keytest, чтобы выключить."
          : "Проверка клавиш выключена.",
      );
      return;
    }
    if (!connected) {
      // #connect / #con из JMC — подключиться выбранным профилем.
      if (/^#con(n|ne|nec|nect)?\b/i.test(text.trim())) {
        doConnect();
        return;
      }
      note("error", "Нет подключения. Подключиться: кнопка вверху или #connect.");
      return;
    }
    // Пароль и «Имя Пароль» на входе уходят как есть: в пароле могут быть «;» и «#».
    const raw = askingPassword || askingName;
    let shown = text;
    if (askingPassword) {
      shown = "*".repeat(text.length);
    } else if (askingName) {
      shown = text.replace(/^(\S+)\s+(.+)$/, (_, name, pass) => `${name} ${"*".repeat(pass.length)}`);
    }
    echo(shown);
    try {
      await send(text, raw);
    } catch (error) {
      note("error", String(error));
    }
  }

  /** Горячие клавиши проверяются до поля ввода, чтобы цифровой блок не печатал цифры. */
  function onKeydownCapture(event: KeyboardEvent) {
    if (editing !== undefined || editingAutomation) return;
    // Карта в режиме правки сама ловит стрелки, Delete, Esc и Ctrl+A — но не цифровой блок:
    // он всегда остаётся горячими клавишами (иначе цифра уходит в строку ввода).
    if (
      event.location !== 3 &&
      MAP_KEYS.has(event.key) &&
      (event.target as HTMLElement).closest?.("[data-no-hotkeys]")
    ) {
      return;
    }
    const name = eventKeyName(event);
    if (keytest && name !== null) {
      const bound = hotkeys.get(name);
      note("echo", `[клавиша] ${name}  ←  ${describeEvent(event)}${bound !== undefined ? `  →  ${bound}` : ""}`);
    }
    if (!connected || name === null) return;
    // Ctrl+C с выделенным текстом — копирование, а не горячая клавиша.
    if (name === "Ctrl+C" && window.getSelection()?.toString()) return;
    const body = hotkeys.get(name);
    if (body === undefined) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.altKey) swallowInputUntil = performance.now() + 1000;
    echo(body);
    send(body).catch((error) => note("error", String(error)));
  }

  /** Windows после Alt+цифра на цифровом блоке вставляет символ по Alt-коду — не пускаем его. */
  function onBeforeInputCapture(event: InputEvent) {
    if (performance.now() < swallowInputUntil) {
      event.preventDefault();
      swallowInputUntil = 0;
    }
  }

  function onKeyupCapture(event: KeyboardEvent) {
    // Alt отпущен — Alt-код уже не придёт; через мгновение снимаем запрет.
    if (event.key === "Alt" && swallowInputUntil > 0) {
      setTimeout(() => (swallowInputUntil = 0), 50);
    }
  }

  /** Любая буква, набранная вне поля ввода, уходит в поле ввода. */
  function onWindowKeydown(event: KeyboardEvent) {
    if (editing !== undefined || editingAutomation) return;
    // Прокрутка вывода и поиск — откуда угодно (свои горячие клавиши на них срабатывают раньше).
    if (event.key === "PageUp" || event.key === "PageDown") {
      event.preventDefault();
      output.page(event.key === "PageUp" ? -1 : 1);
      return;
    }
    if (event.ctrlKey && event.key === "End") {
      event.preventDefault();
      output.toBottom();
      return;
    }
    if (event.ctrlKey && event.code === "KeyF") {
      event.preventDefault();
      output.openSearch();
      return;
    }
    const target = event.target as HTMLElement;
    if (target.closest("input, select, button, textarea")) return;
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    // Цифровой блок вне строки ввода — только горячие клавиши, цифры в строку не печатаем.
    if (event.location === 3) return;
    if (event.key.length === 1 || event.key === "Enter" || event.key.startsWith("Arrow")) {
      commandInput.focus();
    }
  }

  async function closeDialog(savedId?: string | null) {
    editing = undefined;
    await refreshProfiles();
    if (savedId) selectedId = savedId;
    commandInput.focus();
  }

  onMount(() => {
    try {
      selectedId = localStorage.getItem(LAST_PROFILE_KEY) ?? "";
    } catch {
      // нет сохранённого выбора
    }
    refreshProfiles().then(() => {
      if (profiles.length === 0) editing = null; // первый запуск — сразу создать профиль
    });
    const unlisten = onSession(handleBatch);
    commandInput.focus();
    return () => {
      unlisten.then((stop) => stop());
    };
  });
</script>

<svelte:window
  onkeydowncapture={onKeydownCapture}
  onkeyupcapture={onKeyupCapture}
  onbeforeinputcapture={onBeforeInputCapture}
  onkeydown={onWindowKeydown}
/>

<div class="app">
  <header>
    <span class="logo">Клубок</span>
    <div class="connection">
      <select bind:value={selectedId} disabled={connected} aria-label="Профиль">
        {#each profiles as p (p.id)}
          <option value={p.id}>{p.profile.name}</option>
        {:else}
          <option value="">нет профилей</option>
        {/each}
      </select>
      <button
        class="icon"
        title="Настройки профиля"
        disabled={!selected}
        onclick={() => (editing = selected ?? null)}>⚙</button
      >
      <button
        class="icon"
        title="Алиасы, триггеры, переменные, клавиши"
        disabled={!selected}
        onclick={() => (editingAutomation = true)}>ƒ</button
      >
      <button class="icon" title="Новый профиль" onclick={() => (editing = null)}>＋</button>
      <button class="main" class:active={connected} onclick={() => (connected ? disconnect() : doConnect())}>
        {connected ? "Отключиться" : "Подключиться"}
      </button>
    </div>
  </header>

  <main>
    <section class="terminal">
      <Output {lines} split bind:this={output} onclosesearch={() => commandInput.focus()} />
      <CommandInput
        bind:this={commandInput}
        onsubmit={submit}
        password={askingPassword}
        sensitive={askingName}
        complete={completeWord}
      />
    </section>

    {#if room || mapView || status || shownWindows.length > 0}
      <aside class="side">
        {#if status}<StatusPanel {status} />{/if}
        {#if room}
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
        {/if}
        {#each shownWindows as w (w.index)}
          <section class="window">
            <div class="window-bar">
              <span>{w.name || `Окно ${w.index}`}</span>
              <button title="Очистить" onclick={() => (windows[w.index].lines = [])}>⌫</button>
              <button title="Скрыть (#wshow {w.index})" onclick={() => (windows[w.index].visible = false)}
                >✕</button
              >
            </div>
            <div class="window-body"><Output lines={w.lines} /></div>
          </section>
        {/each}
        {#if mapView}
          <MapPanel
            view={mapView}
            found={mapFound}
            onclearfound={() => (mapFound = null)}
            ongo={(vnum) => submit(`#go ${vnum}`)}
            onedit={(edit) => mapEdit(edit).catch((e) => note("error", String(e)))}
          />
        {:else if room}
          <p class="map-hint">Карта появится, когда сервер пришлёт комнату.</p>
        {/if}
      </aside>
    {/if}
  </main>
</div>

{#if editingAutomation && selected}
  <AutomationEditor
    profileId={selected.id}
    profileName={selected.profile.name}
    onclose={() => {
      editingAutomation = false;
      commandInput.focus();
    }}
  />
{/if}

{#if editing !== undefined}
  <ProfileDialog info={editing} {passwordStorage} onclose={closeDialog} />
{/if}

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

  .connection select,
  .connection button {
    padding: 4px 8px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    cursor: pointer;
  }

  .connection select {
    min-width: 0;
    max-width: 220px;
  }

  .connection button.icon {
    width: 32px;
    padding: 4px 0;
  }

  .connection button.main {
    background: var(--accent);
    color: #1a1308;
    border-color: var(--accent);
    font-weight: 600;
  }

  .connection button.main.active {
    background: var(--input-bg);
    color: var(--text);
    border-color: var(--panel-border);
  }

  button:disabled {
    opacity: 0.45;
    cursor: default;
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

  .side {
    width: 320px;
    padding: 12px;
    border-left: 1px solid var(--panel-border);
    background: var(--panel);
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }

  .window {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--bg);
    font-size: 13px;
  }

  .window-bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 6px;
    color: var(--muted);
    font-size: 12px;
    border-bottom: 1px solid var(--panel-border);
  }

  .window-bar span {
    flex: 1;
  }

  .window-bar button {
    border: none;
    background: none;
    color: var(--muted);
    cursor: pointer;
  }

  /* Высоту окна можно тянуть за нижний правый угол. */
  .window-body {
    height: 140px;
    min-height: 60px;
    resize: vertical;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .map-hint {
    color: var(--muted);
    font-size: 12px;
  }

  .room-name {
    font-weight: 600;
    color: var(--c14);
    margin-bottom: 4px;
  }

  .room-meta {
    color: var(--muted);
    font-size: 12px;
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

  /* Узкий экран (телефон): панель комнаты под выводом. */
  @media (max-width: 640px) {
    main {
      flex-direction: column;
    }

    .side {
      width: auto;
      border-left: none;
      border-top: 1px solid var(--panel-border);
      max-height: 40vh;
    }

    .logo {
      display: none;
    }
  }
</style>
