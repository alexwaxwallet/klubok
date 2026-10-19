<script lang="ts">
  let {
    onsubmit,
    password = false,
    sensitive = false,
    disabled = false,
    complete,
  }: {
    onsubmit: (text: string) => void;
    /** Сервер спрашивает пароль: скрыть ввод и не запоминать его в истории. */
    password?: boolean;
    /** Ввод может содержать пароль (например, «Имя Пароль» на входе): не запоминать в истории. */
    sensitive?: boolean;
    disabled?: boolean;
    /** Слова для дополнения по Tab: подходящие под начало, сначала самые свежие. */
    complete?: (prefix: string) => string[];
  } = $props();

  const HISTORY_LIMIT = 200;

  let text = $state("");
  let input: HTMLInputElement;
  let history: string[] = [];
  /** Позиция в истории при листании стрелками; history.length — «новая строка». */
  let historyPos = 0;

  export function focus() {
    input?.focus();
  }

  function submit() {
    const command = text;
    onsubmit(command);
    if (password || sensitive) {
      text = "";
      return;
    }
    if (command !== "" && history[history.length - 1] !== command) {
      history.push(command);
      if (history.length > HISTORY_LIMIT) history.shift();
    }
    historyPos = history.length;
    // Как в JMC: строка очищается, прошлые команды — стрелками вверх/вниз.
    // (Оставлять команду выделенной опасно: Enter на «нажмите ENTER» повторит её.)
    text = "";
  }

  /** Дополнение по Tab: что было до слова, варианты и какой из них сейчас подставлен. */
  let completion: { before: string; options: string[]; index: number } | null = null;

  function tabComplete() {
    if (completion) {
      // Повторный Tab — следующий вариант по кругу.
      completion.index = (completion.index + 1) % completion.options.length;
    } else {
      const match = /(\S+)$/.exec(text);
      if (!match || !complete) return;
      const options = complete(match[1]);
      if (options.length === 0) return;
      completion = { before: text.slice(0, match.index), options, index: 0 };
    }
    text = completion.before + completion.options[completion.index];
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Tab") {
      event.preventDefault();
      if (!password) tabComplete();
      return;
    }
    completion = null;
    if (event.key === "Enter") {
      event.preventDefault();
      submit();
    } else if (event.key === "ArrowUp" && history.length > 0) {
      event.preventDefault();
      historyPos = Math.max(0, historyPos - 1);
      text = history[historyPos];
    } else if (event.key === "ArrowDown" && history.length > 0) {
      event.preventDefault();
      historyPos = Math.min(history.length, historyPos + 1);
      text = history[historyPos] ?? "";
    } else if (event.key === "Escape") {
      text = "";
      historyPos = history.length;
    }
  }
</script>

<div class="input-row">
  <input
    bind:this={input}
    bind:value={text}
    type={password ? "password" : "text"}
    placeholder={password ? "пароль" : "команда"}
    autocomplete="off"
    autocapitalize="off"
    spellcheck="false"
    {disabled}
    onkeydown={onKeydown}
  />
</div>

<style>
  .input-row {
    display: flex;
    padding: 6px 8px;
    border-top: 1px solid var(--panel-border);
    background: var(--panel);
  }

  input {
    flex: 1;
    min-width: 0;
    padding: 7px 10px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    font-family: var(--mono);
    font-size: var(--font-size);
    outline: none;
  }

  input:focus {
    border-color: var(--accent);
  }
</style>
