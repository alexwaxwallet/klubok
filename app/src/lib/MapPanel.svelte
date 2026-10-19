<script lang="ts">
  // Карта текущей зоны в духе MudMapper: клетки-кнопки на тёмно-синем поле, соседние
  // клетки соединены толстыми перемычками. Однонаправленный выход — стрелка, скрытый —
  // пунктир, выход в другую зону или на неисследованное — короткий «обрубок».
  //
  // Обычный режим: клик по клетке — идти туда (#go), перетаскивание поля — сдвиг вида.
  // Режим правки (✎): клик — выделить (Shift — добавить), рамка по пустому месту — выделить
  // группу, перетаскивание выделенного — сдвинуть клетки; стрелки, Delete, цвет, пометки.
  import { tick } from "svelte";
  import type { FoundRoom, MapEdit, MapGhost, MapRoom, MapView } from "./session";

  let {
    view,
    ongo,
    onedit,
    found = null,
    onclearfound,
  }: {
    view: MapView;
    ongo: (vnum: number) => void;
    onedit: (edit: MapEdit) => void;
    /** Ответ на поиск (`null` — не искали). */
    found?: FoundRoom[] | null;
    onclearfound?: () => void;
  } = $props();

  /** Шаг сетки в пикселях при масштабе 1. */
  const CELL = 34;
  const ZOOM_KEY = "klubok.mapZoom";

  /** Цвета клеток, как в палитре MudMapper. "" — снять цвет. */
  const PALETTE = ["", "#ffff80", "#80ffff", "#80ff80", "#ff8080", "#ff80ff", "#8080ff", "#ffb050", "#808080"];

  /** Пометки: код в карте, значок, подпись. */
  const FLAGS: { flag: string; icon: string; title: string }[] = [
    { flag: "dt", icon: "☠", title: "ДТ — смертельная ловушка" },
    { flag: "flydt", icon: "☁", title: "ДТ, если не летать" },
    { flag: "aggro", icon: "⚔", title: "Агрессивные мобы" },
    { flag: "nostep", icon: "✕", title: "Сюда не ходить" },
    { flag: "peace", icon: "☮", title: "Мирная комната" },
  ];

  let width = $state(300);
  let height = $state(300);
  let zoom = $state(loadZoom());
  let editing = $state(false);
  let selected: Set<number> = $state(new Set());
  /** Сдвиг вида (перетаскиванием поля), в пикселях. */
  let pan = $state({ x: 0, y: 0 });
  let svg: SVGSVGElement;

  interface Drag {
    mode: "move" | "box" | "pan";
    x0: number;
    y0: number;
    x: number;
    y: number;
    moved: boolean;
    room?: number;
  }
  let drag = $state<Drag | null>(null);

  // Сменилась комната в центре (игрок шагнул или выбрали найденную) — центрируем на ней.
  let lastCenter = 0;
  $effect(() => {
    if (view.center !== lastCenter) {
      lastCenter = view.center;
      pan = { x: 0, y: 0 };
    }
  });

  /** Смотрим не на игрока, а на комнату из поиска. */
  let browsing = $derived(view.center !== view.current);

  // ───────────── поиск ─────────────

  let searching = $state(false);
  let query = $state("");
  let searchInput: HTMLInputElement | undefined = $state();

  function toggleSearch() {
    searching = !searching;
    if (searching) tick().then(() => searchInput?.focus());
    else onclearfound?.();
  }

  function searchKey(e: KeyboardEvent) {
    if (e.key === "Enter" && query.trim()) onedit({ type: "find", query: query.trim() });
    if (e.key === "Escape") toggleSearch();
  }

  // ───────────── заметка ─────────────

  /** Комната, заметку которой правим. */
  let noteRoom: number | null = $state(null);
  let noteText = $state("");
  let noteInput: HTMLInputElement | undefined = $state();

  function startNote() {
    const vnum = [...selected][0];
    noteRoom = vnum;
    noteText = byVnum.get(vnum)?.note ?? "";
    tick().then(() => noteInput?.focus());
  }

  function noteKey(e: KeyboardEvent) {
    if (e.key === "Enter" && noteRoom !== null) {
      onedit({ type: "note", vnum: noteRoom, note: noteText });
      noteRoom = null;
    }
    if (e.key === "Escape") noteRoom = null;
  }

  function loadZoom(): number {
    try {
      const z = Number(localStorage.getItem(ZOOM_KEY));
      return z >= 0.4 && z <= 3 ? z : 1;
    } catch {
      return 1;
    }
  }

  /** ◎: центр на игроке и его уровень. */
  function toMe() {
    pan = { x: 0, y: 0 };
    if (browsing) onedit({ type: "focus", vnum: null });
    else if (view.z !== view.own_z) onedit({ type: "level", dz: 0 });
  }

  function setZoom(z: number) {
    zoom = Math.min(3, Math.max(0.4, z));
    try {
      localStorage.setItem(ZOOM_KEY, String(zoom));
    } catch {
      // без сохранения — не страшно
    }
  }

  const STEP: Record<string, [number, number]> = {
    n: [0, -1],
    e: [1, 0],
    s: [0, 1],
    w: [-1, 0],
  };

  let s = $derived(CELL * zoom);
  let half = $derived(s * 0.31);
  let byVnum = $derived(new Map(view.rooms.map((r) => [r.vnum, r])));
  /** Комната в центре панели (обычно — игрок). */
  let here = $derived(byVnum.get(view.center));
  let onPath = $derived(new Set(view.path));

  /** Комнаты, стоящие в одной клетке с другими: номер по порядку в клетке. */
  let stacked = $derived.by(() => {
    const count = new Map<string, number>();
    const order = new Map<number, number>();
    for (const r of view.rooms) {
      const key = `${r.x},${r.y}`;
      const n = count.get(key) ?? 0;
      count.set(key, n + 1);
      order.set(r.vnum, n);
    }
    for (const r of view.rooms) if ((count.get(`${r.x},${r.y}`) ?? 0) < 2) order.delete(r.vnum);
    return order;
  });

  /** Сдвиг перетаскиваемых клеток, в клетках (y — вниз по экрану). */
  let dragCells = $derived(
    drag?.mode === "move" && drag.moved
      ? [Math.round((drag.x - drag.x0) / s), Math.round((drag.y - drag.y0) / s)]
      : [0, 0],
  );

  let offset = $derived(
    drag?.mode === "pan" ? { x: pan.x + drag.x - drag.x0, y: pan.y + drag.y - drag.y0 } : pan,
  );

  /** Экранные координаты центра комнаты. */
  function pos(r: MapRoom): [number, number] {
    const k = (stacked.get(r.vnum) ?? 0) * 0.35;
    let x = (r.x - (here?.x ?? 0) + k) * s + offset.x;
    let y = -(r.y - (here?.y ?? 0) - k) * s + offset.y;
    if (selected.has(r.vnum)) {
      x += dragCells[0] * s;
      y += dragCells[1] * s;
    }
    return [x, y];
  }

  interface Line {
    key: string;
    x1: number;
    y1: number;
    x2: number;
    y2: number;
    /** Соседние клетки — толстая перемычка. */
    near: boolean;
    oneWay: boolean;
    hidden: boolean;
    stub: boolean;
    door: boolean;
  }

  let lines = $derived.by(() => {
    const out: Line[] = [];
    for (const room of view.rooms) {
      const [x1, y1] = pos(room);
      for (const exit of room.exits) {
        const step = STEP[exit.dir];
        if (!step) continue;
        const target = exit.to !== undefined ? byVnum.get(exit.to) : undefined;
        const base = { hidden: !!exit.hidden, door: !!exit.door };
        if (target) {
          const back = target.exits.some((e) => e.to === room.vnum);
          if (back && target.vnum < room.vnum) continue; // двусторонний — один раз
          const [x2, y2] = pos(target);
          const near = Math.abs(x2 - x1) + Math.abs(y2 - y1) <= s * 1.01;
          out.push({ key: `${room.vnum}${exit.dir}`, x1, y1, x2, y2, near, oneWay: !back, stub: false, ...base });
        } else {
          out.push({
            key: `${room.vnum}${exit.dir}`,
            x1,
            y1,
            x2: x1 + step[0] * s * 0.55,
            y2: y1 + step[1] * s * 0.55,
            near: true,
            oneWay: false,
            stub: true,
            ...base,
          });
        }
      }
    }
    return out;
  });

  function title(r: MapRoom): string {
    let t = `${r.name} [${r.vnum}]`;
    const flags = FLAGS.filter((f) => r.flags?.includes(f.flag)).map((f) => f.title);
    if (flags.length) t += `\n${flags.join(", ")}`;
    if (r.note) t += `\n${r.note}`;
    if (stacked.has(r.vnum)) t += "\nв одной клетке с другой комнатой — передвиньте в режиме правки";
    return t;
  }

  function hasExit(r: MapRoom, dir: string): boolean {
    return r.exits.some((e) => e.dir === dir);
  }

  /** Стороны клетки: где выход — проём, где нет — стена. `lit` — светлая грань объёма. */
  const SIDES = [
    { dir: "n", lit: true },
    { dir: "w", lit: true },
    { dir: "s", lit: false },
    { dir: "e", lit: false },
  ];

  /** Полуширина проёма — как у перемычки между клетками. */
  let gap = $derived(Math.max(1.5, s * 0.09));

  /** Путь стороны клетки: целиком (`open` = false) или два куска по краям проёма. */
  function side(dir: string, open: boolean): string {
    const h = half;
    // Концы стороны и её середина.
    const [ax, ay, bx, by] = {
      n: [-h, -h, h, -h],
      s: [-h, h, h, h],
      w: [-h, -h, -h, h],
      e: [h, -h, h, h],
    }[dir as "n" | "s" | "w" | "e"];
    if (!open) return `M${ax},${ay} L${bx},${by}`;
    const horizontal = ay === by;
    const [m1x, m1y] = horizontal ? [-gap, ay] : [ax, -gap];
    const [m2x, m2y] = horizontal ? [gap, ay] : [ax, gap];
    return `M${ax},${ay} L${m1x},${m1y} M${m2x},${m2y} L${bx},${by}`;
  }

  /** Сдвиг клетки соседнего уровня: выше — вправо-вверх, ниже — влево-вниз. */
  function ghostPos(g: MapGhost): [number, number] {
    const k = (g.z > view.z ? 1 : -1) * s * 0.22;
    const x = (g.x - (here?.x ?? 0)) * s + offset.x + k;
    const y = -(g.y - (here?.y ?? 0)) * s + offset.y - k;
    return [x, y];
  }

  function icon(r: MapRoom): string {
    return FLAGS.find((f) => r.flags?.includes(f.flag))?.icon ?? "";
  }

  // ───────────── мышь ─────────────

  /** Координаты события в системе SVG (центр поля — 0,0). */
  function local(e: PointerEvent): [number, number] {
    const rect = svg.getBoundingClientRect();
    return [e.clientX - rect.left - width / 2, e.clientY - rect.top - height / 2];
  }

  function down(e: PointerEvent, room?: number) {
    if (e.button !== 0) return;
    e.stopPropagation();
    svg.setPointerCapture(e.pointerId);
    svg.focus();
    const [x, y] = local(e);
    const mode = editing ? (room !== undefined ? "move" : "box") : "pan";
    drag = { mode, x0: x, y0: y, x, y, moved: false, room };
    if (editing && room !== undefined) {
      if (e.shiftKey) {
        const next = new Set(selected);
        if (next.has(room)) next.delete(room);
        else next.add(room);
        selected = next;
      } else if (!selected.has(room)) {
        selected = new Set([room]);
      }
    }
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const [x, y] = local(e);
    drag.x = x;
    drag.y = y;
    if (Math.abs(x - drag.x0) + Math.abs(y - drag.y0) > 4) drag.moved = true;
  }

  function up() {
    const d = drag;
    if (!d) return;
    if (!d.moved) {
      if (!editing && d.room !== undefined) ongo(d.room);
      if (editing && d.mode === "box") selected = new Set();
    } else if (d.mode === "move") {
      const [dx, dy] = dragCells;
      if (dx !== 0 || dy !== 0) moveSelected(dx, -dy, 0);
    } else if (d.mode === "box") {
      const [x1, x2] = [Math.min(d.x0, d.x), Math.max(d.x0, d.x)];
      const [y1, y2] = [Math.min(d.y0, d.y), Math.max(d.y0, d.y)];
      const inside = view.rooms.filter((r) => {
        const [x, y] = pos(r);
        return x >= x1 && x <= x2 && y >= y1 && y <= y2;
      });
      selected = new Set(inside.map((r) => r.vnum));
    } else if (d.mode === "pan") {
      pan = { x: pan.x + d.x - d.x0, y: pan.y + d.y - d.y0 };
    }
    drag = null;
  }

  // ───────────── правка ─────────────

  function moveSelected(dx: number, dy: number, dz: number) {
    if (selected.size === 0) return;
    onedit({ type: "move", rooms: [...selected], dx, dy, dz });
    // На другой уровень — клетки уходят из вида, снимаем выделение.
    if (dz !== 0) selected = new Set();
  }

  function deleteSelected() {
    if (selected.size === 0) return;
    if (!confirm(`Удалить с карты клеток: ${selected.size}?`)) return;
    onedit({ type: "delete", rooms: [...selected] });
    selected = new Set();
  }

  function paint(color: string) {
    if (selected.size > 0) onedit({ type: "color", rooms: [...selected], color });
  }

  function toggleFlag(flag: string) {
    if (selected.size === 0) return;
    const rooms = view.rooms.filter((r) => selected.has(r.vnum));
    const all = rooms.every((r) => r.flags?.includes(flag));
    onedit({ type: "flag", rooms: [...selected], flag, on: !all });
  }

  function key(e: KeyboardEvent) {
    // Цифровой блок — горячие клавиши (ходьба), карта его не трогает. В полях ввода — обычный ввод.
    if (!editing || e.location === 3 || (e.target as HTMLElement).tagName === "INPUT") return;
    const arrows: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, 1],
      ArrowDown: [0, -1],
    };
    if (arrows[e.key]) {
      moveSelected(...arrows[e.key], 0);
    } else if (e.key === "Delete") {
      deleteSelected();
    } else if (e.key === "Escape") {
      selected = new Set();
    } else if (e.code === "KeyA" && e.ctrlKey) {
      selected = new Set(view.rooms.map((r) => r.vnum));
    } else {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function toggleEditing() {
    editing = !editing;
    selected = new Set();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="map" onkeydown={key} data-no-hotkeys={editing ? "" : undefined}>
  <div class="bar">
    <span class="area" title="зона {view.zone}">{view.area || `зона ${view.zone}`}</span>
    <button title="Посмотреть уровень ниже" onclick={() => onedit({ type: "level", dz: -1 })}>▼</button>
    <span class="level" class:away={view.z !== view.own_z} title={view.z !== view.own_z ? `вы на уровне ${view.own_z}` : "ваш уровень"}>ур. {view.z}</span>
    <button title="Посмотреть уровень выше" onclick={() => onedit({ type: "level", dz: 1 })}>▲</button>
    <button title="Мельче" onclick={() => setZoom(zoom / 1.25)}>−</button>
    <button title="Крупнее" onclick={() => setZoom(zoom * 1.25)}>+</button>
    <button class:on={searching} title="Найти комнату" onclick={toggleSearch}>⌕</button>
    <button class:on={browsing} title={browsing ? "Вернуться к себе" : "К себе"} onclick={toMe}>◎</button>
    <button class:on={editing} title="Правка карты" onclick={toggleEditing}>✎</button>
  </div>
  {#if browsing}
    <div class="browse">просмотр [{view.center}] — клик по клетке: идти туда; ◎ — к себе</div>
  {/if}
  {#if searching}
    <div class="search">
      <input
        bind:this={searchInput}
        bind:value={query}
        placeholder="название, номер или заметка — Enter"
        spellcheck="false"
        onkeydown={searchKey}
      />
      {#if found}
        <div class="found">
          {#each found as f (f.vnum)}
            <div class="found-row" class:active={f.vnum === view.center}>
              <button
                class="found-name"
                title="Показать на карте{f.note ? `\n${f.note}` : ''}"
                onclick={() => onedit({ type: "focus", vnum: f.vnum })}
                >[{f.vnum}] {f.name} <span class="found-area">{f.area}</span></button
              >
              <button title="Идти туда (#go {f.vnum})" onclick={() => ongo(f.vnum)}>→</button>
            </div>
          {:else}
            <div class="found-none">ничего не найдено</div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
  {#if editing}
    <div class="tools">
      <span class="sel">{selected.size ? `выделено ${selected.size}` : "выделите клетки"}</span>
      <button title="Влево (←)" onclick={() => moveSelected(-1, 0, 0)}>←</button>
      <button title="Вверх (↑)" onclick={() => moveSelected(0, 1, 0)}>↑</button>
      <button title="Вниз (↓)" onclick={() => moveSelected(0, -1, 0)}>↓</button>
      <button title="Вправо (→)" onclick={() => moveSelected(1, 0, 0)}>→</button>
      <button title="На уровень выше" onclick={() => moveSelected(0, 0, 1)}>⤒</button>
      <button title="На уровень ниже" onclick={() => moveSelected(0, 0, -1)}>⤓</button>
      <button class="danger" title="Удалить с карты (Delete)" onclick={deleteSelected}>Del</button>
      <span class="break"></span>
      {#each PALETTE as color (color)}
        <button
          class="swatch"
          title={color ? `Покрасить ${color}` : "Снять цвет"}
          style:background={color || null}
          onclick={() => paint(color)}>{color ? "" : "∅"}</button
        >
      {/each}
      {#each FLAGS as f (f.flag)}
        <button title={f.title} onclick={() => toggleFlag(f.flag)}>{f.icon}</button>
      {/each}
      <button title="Заметка к комнате" disabled={selected.size !== 1} onclick={startNote}>✍</button>
      {#if noteRoom !== null}
        <input
          class="note-input"
          bind:this={noteInput}
          bind:value={noteText}
          placeholder="заметка к [{noteRoom}] — Enter, пусто — стереть"
          onkeydown={noteKey}
        />
      {/if}
    </div>
  {/if}
  <div class="canvas" bind:clientWidth={width} bind:clientHeight={height}>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <svg
      bind:this={svg}
      viewBox="{-width / 2} {-height / 2} {width} {height}"
      {width}
      {height}
      role="application"
      aria-label="Карта"
      tabindex="0"
      class:editing
      onpointerdown={(e) => down(e)}
      onpointermove={move}
      onpointerup={up}
      onpointercancel={() => (drag = null)}
      onwheel={(e) => {
        e.preventDefault();
        setZoom(zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1));
      }}
    >
      <defs>
        <!-- штриховка для комнат соседних уровней, как в MudMapper -->
        <pattern id="ghost-hatch" width="4" height="4" patternUnits="userSpaceOnUse">
          <rect width="4" height="4" class="ghost-bg" />
          <path d="M0,4 L4,0 M-1,1 L1,-1 M3,5 L5,3" class="ghost-line" />
        </pattern>
        <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="4" markerHeight="4" orient="auto">
          <path d="M0,0 L10,5 L0,10 z" class="arrowhead" />
        </marker>
      </defs>
      {#each view.ghosts ?? [] as g (g.vnum)}
        {@const [x, y] = ghostPos(g)}
        <rect class="ghost" x={x - half} y={y - half} width={half * 2} height={half * 2}>
          <title>{g.z > view.z ? "уровнем выше" : "уровнем ниже"} [{g.vnum}]</title>
        </rect>
      {/each}
      {#each lines as l (l.key)}
        <line
          x1={l.x1}
          y1={l.y1}
          x2={l.x2}
          y2={l.y2}
          class="exit"
          class:near={l.near}
          class:hidden={l.hidden}
          class:stub={l.stub}
          class:door={l.door}
          style:stroke-width={l.near && !l.hidden ? `${Math.max(3, s * 0.18)}px` : null}
          marker-end={l.oneWay ? "url(#arrow)" : undefined}
        />
      {/each}
      {#each view.rooms as r (r.vnum)}
        {@const [x, y] = pos(r)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <g
          class="room"
          class:current={r.vnum === view.current}
          class:path={onPath.has(r.vnum)}
          class:selected={selected.has(r.vnum)}
          class:stacked={stacked.has(r.vnum)}
          transform="translate({x} {y})"
          onpointerdown={(e) => down(e, r.vnum)}
        >
          <title>{title(r)}</title>
          <rect class="face" x={-half} y={-half} width={half * 2} height={half * 2} style:fill={r.color || null} />
          <!-- стороны: проём там, где выход; стена — где выхода нет -->
          {#each SIDES as sd (sd.dir)}
            {#if hasExit(r, sd.dir)}
              <path class={sd.lit ? "light" : "dark"} d={side(sd.dir, true)} />
            {:else}
              <path class="wall" d={side(sd.dir, false)} style:stroke-width="{Math.max(2, s * 0.08)}px" />
            {/if}
          {/each}
          {#if hasExit(r, "u")}<path class="updown" d="M{half - 1},{-half + 1} h{-half * 0.7} l{half * 0.7},{half * 0.7} z" />{/if}
          {#if hasExit(r, "d")}<path class="updown" d="M{-half + 1},{half - 1} h{half * 0.7} l{-half * 0.7},{-half * 0.7} z" />{/if}
          {#if icon(r)}
            <text class="icon" y={half * 0.38} font-size={half * 1.25}>{icon(r)}</text>
          {/if}
          {#if r.note}<circle class="note" cx={half - 3} cy={half - 3} r="2" />{/if}
          {#if browsing && r.vnum === view.center}
            <rect class="focus" x={-half - 4} y={-half - 4} width={half * 2 + 8} height={half * 2 + 8} rx="2" />
          {/if}
          {#if r.vnum === view.current}
            <rect class="here" x={-half - 3} y={-half - 3} width={half * 2 + 6} height={half * 2 + 6} rx="2" />
          {/if}
        </g>
      {/each}
      {#if drag?.mode === "box" && drag.moved}
        <rect
          class="box"
          x={Math.min(drag.x0, drag.x)}
          y={Math.min(drag.y0, drag.y)}
          width={Math.abs(drag.x - drag.x0)}
          height={Math.abs(drag.y - drag.y0)}
        />
      {/if}
    </svg>
  </div>
</div>

<style>
  .map {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
  }

  .bar,
  .tools {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 0;
    font-size: 12px;
    color: var(--muted);
  }

  .tools {
    flex-wrap: wrap;
    padding-top: 0;
  }

  .tools .sel {
    width: 100%;
  }

  .tools .break {
    width: 100%;
    height: 0;
  }

  .level {
    white-space: nowrap;
  }

  .browse {
    padding: 2px 4px;
    margin-bottom: 4px;
    font-size: 11px;
    color: var(--accent);
    border: 1px dashed var(--accent);
    border-radius: 3px;
  }

  .search {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-bottom: 4px;
    font-size: 12px;
  }

  .search input,
  .note-input {
    width: 100%;
    padding: 3px 6px;
    border: 1px solid var(--panel-border);
    border-radius: 3px;
    background: var(--input-bg);
    outline: none;
  }

  .search input:focus,
  .note-input:focus {
    border-color: var(--accent);
  }

  .found {
    max-height: 160px;
    overflow-y: auto;
    border: 1px solid var(--panel-border);
    border-radius: 3px;
  }

  .found-row {
    display: flex;
    align-items: center;
  }

  .found-row.active {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
  }

  .found-row .found-name {
    flex: 1;
    min-width: 0;
    height: auto;
    padding: 2px 4px;
    border: none;
    background: none;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .found-area {
    color: var(--muted);
  }

  .found-none {
    padding: 2px 4px;
    color: var(--muted);
  }
  .level.away {
    color: var(--accent);
    font-weight: 600;
  }
  .area {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button {
    min-width: 24px;
    height: 22px;
    padding: 0 4px;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    color: var(--text);
    cursor: pointer;
    font-size: 13px;
    line-height: 1;
  }

  button.on {
    border-color: var(--accent);
    color: var(--accent);
  }

  button.swatch {
    width: 22px;
    min-width: 22px;
  }

  .canvas {
    flex: 1;
    min-height: 160px;
    overflow: hidden;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--map-bg);
  }

  svg {
    display: block;
    outline: none;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }

  svg.editing {
    cursor: crosshair;
  }

  .exit {
    stroke: var(--map-link);
    stroke-width: 1.5;
    stroke-linecap: square;
  }

  .exit.hidden {
    stroke-dasharray: 4 3;
    stroke-linecap: butt;
  }

  .exit.stub {
    stroke: var(--map-stub);
  }

  .exit.door {
    stroke: var(--map-door);
  }

  .arrowhead {
    fill: var(--map-link);
  }

  .room {
    cursor: pointer;
  }

  .face {
    fill: var(--map-room);
  }

  .light,
  .dark {
    fill: none;
    stroke-width: 1.5;
  }

  .light {
    stroke: var(--map-room-light);
  }

  .dark {
    stroke: var(--map-room-dark);
  }

  .wall {
    fill: none;
    stroke: var(--map-wall);
    stroke-linecap: square;
  }

  .ghost {
    fill: url(#ghost-hatch);
    opacity: 0.8;
    pointer-events: none;
  }

  .ghost-bg {
    fill: var(--map-ghost-bg);
  }

  .ghost-line {
    stroke: var(--map-ghost-line);
    stroke-width: 1;
  }

  .updown {
    fill: var(--map-room-dark);
  }

  .icon {
    text-anchor: middle;
    fill: #111;
    pointer-events: none;
  }

  .note {
    fill: #1a5fd0;
  }

  .focus {
    fill: none;
    stroke: #00e0ff;
    stroke-width: 2;
    stroke-dasharray: 4 3;
  }

  .here {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
  }

  .room.path .face {
    stroke: var(--c11);
    stroke-width: 2;
  }

  .room.stacked .face {
    stroke: var(--danger);
    stroke-width: 2;
    stroke-dasharray: 2 2;
  }

  .room.selected .face {
    stroke: #00e0ff;
    stroke-width: 3;
  }

  .box {
    fill: rgba(0, 224, 255, 0.12);
    stroke: #00e0ff;
    stroke-dasharray: 4 3;
  }
</style>
