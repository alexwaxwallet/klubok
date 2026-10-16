<script lang="ts">
  import {
    deleteProfile,
    importMm2,
    importSet,
    newProfile,
    saveProfile,
    type ImportResult,
    type MapImportResult,
    type ProfileInfo,
  } from "./session";

  let {
    info,
    passwordStorage,
    onclose,
  }: {
    /** Редактируемый профиль; null — новый. */
    info: ProfileInfo | null;
    passwordStorage: string;
    /** savedId — id сохранённого профиля, null — удалён, undefined — отмена. */
    onclose: (savedId?: string | null) => void;
  } = $props();

  // Копия, чтобы «Отмена» ничего не меняла. Профиль в диалоге живёт отдельно от списка.
  // svelte-ignore state_referenced_locally
  let profile = $state(structuredClone(info ? $state.snapshot(info.profile) : newProfile()));
  // svelte-ignore state_referenced_locally
  let id: string | null = $state(info?.id ?? null);
  // svelte-ignore state_referenced_locally
  let hasPassword = $state(info?.has_password ?? false);
  let password = $state("");
  let forgetPassword = $state(false);
  let error = $state("");
  let busy = $state(false);
  let importResult: ImportResult | null = $state(null);
  let fileInput: HTMLInputElement;
  let mapResult: MapImportResult | null = $state(null);
  let mapBusy = $state(false);

  async function importMap() {
    const target = id ?? (await save());
    if (!target) return;
    mapBusy = true;
    try {
      // Имя карты могли поменять — сначала сохраняем профиль.
      await save();
      mapResult = await importMm2(target);
      error = "";
    } catch (e) {
      error = String(e);
    } finally {
      mapBusy = false;
    }
  }

  async function save(): Promise<string | null> {
    error = "";
    busy = true;
    try {
      const newPassword = forgetPassword ? "" : password !== "" ? password : undefined;
      id = await saveProfile(id, $state.snapshot(profile), newPassword);
      if (newPassword !== undefined) hasPassword = newPassword !== "";
      password = "";
      forgetPassword = false;
      return id;
    } catch (e) {
      error = String(e);
      return null;
    } finally {
      busy = false;
    }
  }

  async function saveAndClose() {
    const saved = await save();
    if (saved) onclose(saved);
  }

  async function remove() {
    if (!id || !confirm(`Удалить профиль «${profile.name}» вместе с его настройками и паролем?`)) return;
    try {
      await deleteProfile(id);
      onclose(null);
    } catch (e) {
      error = String(e);
    }
  }

  async function onFile(event: Event) {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;
    // Новый профиль сначала сохраняем — импортировать некуда.
    const target = id ?? (await save());
    if (!target) return;
    try {
      importResult = await importSet(target, new Uint8Array(await file.arrayBuffer()));
      error = "";
    } catch (e) {
      error = String(e);
    }
    fileInput.value = "";
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <form
    class="dialog"
    onsubmit={(e) => {
      e.preventDefault();
      saveAndClose();
    }}
  >
    <h2>{id ? "Профиль" : "Новый профиль"}</h2>

    <label>
      Название
      <input bind:value={profile.name} placeholder="Колдун" required />
    </label>

    <div class="row">
      <label class="grow">
        Сервер
        <input bind:value={profile.host} required />
      </label>
      <label class="port">
        Порт
        <input type="number" bind:value={profile.port} min="1" max="65535" required />
      </label>
      <label>
        Кодировка
        <select bind:value={profile.charset}>
          <option value="utf8">UTF-8</option>
          <option value="koi8r">KOI8-R</option>
          <option value="cp1251">CP1251</option>
        </select>
      </label>
    </div>

    <fieldset>
      <legend>Автовход</legend>
      <div class="row">
        <label class="grow">
          Имя персонажа
          <input bind:value={profile.character} placeholder="пусто — входить вручную" />
        </label>
        <label class="grow">
          Пароль
          <input
            type="password"
            bind:value={password}
            disabled={forgetPassword}
            autocomplete="new-password"
            placeholder={hasPassword ? "сохранён — пусто, чтобы не менять" : "не сохранён"}
          />
        </label>
      </div>
      {#if hasPassword}
        <label class="check">
          <input type="checkbox" bind:checked={forgetPassword} /> забыть сохранённый пароль
        </label>
      {/if}
      <p class="hint">Пароль хранится: {passwordStorage}.</p>
    </fieldset>

    <fieldset>
      <legend>Пустые строки</legend>
      <label class="check">
        Не больше
        <input class="small" type="number" min="0" max="9" bind:value={profile.blank_lines.max_consecutive} />
        подряд (0 — убирать все)
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={profile.blank_lines.trim_before_prompt} /> убирать перед промптом
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={profile.blank_lines.trim_after_prompt} /> убирать после промпта
      </label>
    </fieldset>

    <fieldset>
      <legend>Алиасы, триггеры, переменные, горячие клавиши</legend>
      <p class="hint">
        Добавить из файла настроек JMC (<code>.set</code>). Можно и командами в строке ввода:
        <code>#alias</code>, <code>#action</code>, <code>#var</code>, <code>#hot</code>…
      </p>
      <input bind:this={fileInput} type="file" accept=".set,.txt" hidden onchange={onFile} />
      <button type="button" onclick={() => fileInput.click()} disabled={busy}>Импорт из JMC…</button>
      {#if importResult}
        <p class="report">
          Добавлено: алиасов {importResult.aliases}, триггеров {importResult.actions},
          переменных {importResult.variables}, подсветок {importResult.highlights},
          горячих клавиш {importResult.hotkeys}, замен {importResult.substitutions}.
        </p>
        {#if importResult.login_actions.length > 0}
          <div class="warning">
            <p>
              Найдено триггеров входа: {importResult.login_actions.length}. Пока идёт вход, Клубок
              их не запускает — входит сам, по имени и паролю выше.
            </p>
            <p>
              Если в них записан пароль, он хранится в настройках открытым текстом. Лучше удалить
              их командой в строке ввода:
            </p>
            {#each importResult.login_actions as pattern (pattern)}
              <code class="cmd">#unaction {"{"}{pattern}{"}"}</code>
            {/each}
          </div>
        {/if}
        {#if importResult.unsupported.length > 0}
          <p class="hint">
            Сохранено, но пока не работает:
            {importResult.unsupported.map(([name, n]) => `${name} × ${n}`).join(", ")}.
          </p>
        {/if}
      {/if}
    </fieldset>

    <fieldset>
      <legend>Карта</legend>
      <label>
        Имя карты
        <input bind:value={profile.map} placeholder="bylins" />
      </label>
      <p class="hint">
        Профили с одинаковым именем карты делят одну карту. Карта строится сама по ходу игры;
        можно добавить старые карты MudMapper — выберите папку с файлами <code>.mm2</code>.
        Уже известные комнаты не перезаписываются.
      </p>
      <button type="button" onclick={importMap} disabled={busy || mapBusy}>
        {mapBusy ? "Импорт…" : "Импорт карт MudMapper…"}
      </button>
      {#if mapResult}
        <p class="report">
          Файлов {mapResult.files}, комнат в них {mapResult.rooms_total}, добавлено
          {mapResult.rooms_added}. Без номера комнаты: {mapResult.synthetic}, повторов номера:
          {mapResult.duplicates}, выходов без цели: {mapResult.unresolved_exits}.
        </p>
        {#if mapResult.errors.length > 0}
          <p class="hint">Не прочитаны: {mapResult.errors.join("; ")}</p>
        {/if}
      {/if}
    </fieldset>

    {#if error}<p class="error">{error}</p>{/if}

    <div class="buttons">
      {#if id}
        <button type="button" class="danger" onclick={remove}>Удалить</button>
      {/if}
      <span class="spacer"></span>
      <button type="button" onclick={() => onclose()}>Отмена</button>
      <button type="submit" class="primary" disabled={busy}>Сохранить</button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 40px 16px;
    overflow-y: auto;
    z-index: 10;
  }

  .dialog {
    width: 100%;
    max-width: 560px;
    background: var(--panel);
    border: 1px solid var(--panel-border);
    border-radius: 8px;
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  h2 {
    margin: 0;
    font-size: 17px;
    color: var(--accent);
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--muted);
    font-size: 12px;
  }

  label.check {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text);
  }

  .row {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }

  .grow {
    flex: 1;
    min-width: 140px;
  }

  .port input {
    width: 80px;
  }

  input,
  select,
  button {
    padding: 6px 9px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    color: var(--text);
    font-size: 14px;
  }

  input::placeholder {
    color: var(--muted);
    opacity: 0.7;
  }

  input:focus,
  select:focus {
    outline: none;
    border-color: var(--accent);
  }

  input.small {
    width: 52px;
  }

  input[type="checkbox"] {
    padding: 0;
  }

  fieldset {
    border: 1px solid var(--panel-border);
    border-radius: 6px;
    padding: 10px 12px 12px;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  legend {
    padding: 0 6px;
    color: var(--muted);
    font-size: 12px;
  }

  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .report {
    margin: 0;
    font-size: 13px;
  }

  code {
    font-family: var(--mono);
    color: var(--text);
  }

  .error {
    margin: 0;
    color: var(--danger);
  }

  .warning {
    border-left: 3px solid var(--c3);
    padding: 4px 10px;
    font-size: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .warning p {
    margin: 0;
  }

  .cmd {
    display: block;
    overflow-wrap: anywhere;
    user-select: all;
  }

  .buttons {
    display: flex;
    gap: 8px;
  }

  .spacer {
    flex: 1;
  }

  button {
    cursor: pointer;
  }

  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #1a1308;
    font-weight: 600;
  }

  button.danger {
    color: var(--danger);
  }

  fieldset > button {
    align-self: flex-start;
  }
</style>
