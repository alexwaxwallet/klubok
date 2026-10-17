<script lang="ts">
  // Карта текущей зоны: комнаты — квадраты, выходы — линии. Текущая комната всегда в центре.
  // Однонаправленный выход рисуется стрелкой, скрытый — пунктиром,
  // выход в другую зону или на неисследованное — короткой «культёй».
  import type { MapRoom, MapView } from "./session";

  let { view, ongo }: { view: MapView; ongo: (vnum: number) => void } = $props();

  /** Шаг сетки в пикселях при масштабе 1. */
  const CELL = 30;
  const ZOOM_KEY = "klubok.mapZoom";

  let width = $state(300);
  let height = $state(300);
  let zoom = $state(loadZoom());

  function loadZoom(): number {
    try {
      const z = Number(localStorage.getItem(ZOOM_KEY));
      return z >= 0.4 && z <= 3 ? z : 1;
    } catch {
      return 1;
    }
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
  let byVnum = $derived(new Map(view.rooms.map((r) => [r.vnum, r])));
  let here = $derived(byVnum.get(view.current));
  let onPath = $derived(new Set(view.path));

  /** Комнаты, которые стоят в одной клетке с другими: номер по порядку в клетке (0 — первая). */
  let stacked = $derived.by(() => {
    const seen = new Map<string, number>();
    const order = new Map<number, number>();
    for (const r of view.rooms) {
      const key = `${r.x},${r.y}`;
      const n = seen.get(key) ?? 0;
      seen.set(key, n + 1);
      order.set(r.vnum, n);
    }
    // Оставляем только клетки, где комнат больше одной.
    for (const r of view.rooms) {
      if ((seen.get(`${r.x},${r.y}`) ?? 0) < 2) order.delete(r.vnum);
    }
    return order;
  });

  /** Экранные координаты комнаты (центр карты — текущая комната).
   *  Слипшиеся комнаты рисуются со сдвигом, чтобы было видно обе (`#map fix` разведёт). */
  function pos(r: MapRoom): [number, number] {
    const cx = here?.x ?? 0;
    const cy = here?.y ?? 0;
    const k = (stacked.get(r.vnum) ?? 0) * 0.35;
    return [(r.x - cx + k) * s, -(r.y - cy - k) * s];
  }

  interface Line {
    key: string;
    x1: number;
    y1: number;
    x2: number;
    y2: number;
    oneWay: boolean;
    hidden: boolean;
    /** Выход в другую зону/уровень или на неисследованное. */
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
          // Двусторонний выход рисуем один раз.
          if (back && target.vnum < room.vnum) continue;
          const [x2, y2] = pos(target);
          out.push({ key: `${room.vnum}${exit.dir}`, x1, y1, x2, y2, oneWay: !back, stub: false, ...base });
        } else {
          out.push({
            key: `${room.vnum}${exit.dir}`,
            x1,
            y1,
            x2: x1 + step[0] * s * 0.5,
            y2: y1 + step[1] * s * 0.5,
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
    if (r.flags?.length) t += `\n${r.flags.map(flagName).join(", ")}`;
    if (r.note) t += `\n${r.note}`;
    if (stacked.has(r.vnum)) t += "\nв одной клетке с другой комнатой — #map fix";
    return t;
  }

  function flagName(f: string): string {
    return { dt: "ДТ", aggro: "агро", flydt: "ДТ без полёта", nostep: "не ходить" }[f] ?? f;
  }

  function hasExit(r: MapRoom, dir: string): boolean {
    return r.exits.some((e) => e.dir === dir);
  }
</script>

<div class="map">
  <div class="bar">
    <span class="area" title="зона {view.zone}">{view.area || `зона ${view.zone}`}</span>
    <span class="level">ур. {view.z}</span>
    <button title="Мельче" onclick={() => setZoom(zoom / 1.25)}>−</button>
    <button title="Крупнее" onclick={() => setZoom(zoom * 1.25)}>+</button>
  </div>
  <div class="canvas" bind:clientWidth={width} bind:clientHeight={height}>
    <svg
      viewBox="{-width / 2} {-height / 2} {width} {height}"
      {width}
      {height}
      role="img"
      aria-label="Карта"
      onwheel={(e) => {
        e.preventDefault();
        setZoom(zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1));
      }}
    >
      <defs>
        <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5" markerHeight="5" orient="auto">
          <path d="M0,0 L10,5 L0,10 z" class="arrowhead" />
        </marker>
      </defs>
      {#each lines as l (l.key)}
        <line
          x1={l.x1}
          y1={l.y1}
          x2={l.x2}
          y2={l.y2}
          class="exit"
          class:hidden={l.hidden}
          class:stub={l.stub}
          class:door={l.door}
          marker-end={l.oneWay ? "url(#arrow)" : undefined}
        />
      {/each}
      {#each view.rooms as r (r.vnum)}
        {@const [x, y] = pos(r)}
        {@const half = s * 0.3}
        <g
          class="room"
          class:current={r.vnum === view.current}
          class:path={onPath.has(r.vnum)}
          class:dt={r.flags?.includes("dt") || r.flags?.includes("flydt")}
          class:aggro={r.flags?.includes("aggro")}
          class:stacked={stacked.has(r.vnum)}
          transform="translate({x} {y})"
          role="button"
          tabindex="-1"
          onclick={() => ongo(r.vnum)}
          onkeydown={() => {}}
        >
          <title>{title(r)}</title>
          <rect x={-half} y={-half} width={half * 2} height={half * 2} rx="2" style:fill={r.color || null} />
          {#if hasExit(r, "u")}<path class="updown" d="M{-half + 2},{-half + 6} l3,-4 l3,4 z" />{/if}
          {#if hasExit(r, "d")}<path class="updown" d="M{half - 8},{half - 6} l3,4 l3,-4 z" />{/if}
          {#if r.note}<circle class="note" cx={half - 3} cy={-half + 3} r="2" />{/if}
        </g>
      {/each}
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

  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 0;
    font-size: 12px;
    color: var(--muted);
  }

  .area {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bar button {
    width: 24px;
    height: 22px;
    padding: 0;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--input-bg);
    cursor: pointer;
  }

  .canvas {
    flex: 1;
    min-height: 160px;
    overflow: hidden;
    border: 1px solid var(--panel-border);
    border-radius: 4px;
    background: var(--bg);
  }

  svg {
    display: block;
  }

  .exit {
    stroke: var(--map-exit);
    stroke-width: 1.5;
  }

  .exit.hidden {
    stroke-dasharray: 3 3;
  }

  .exit.stub {
    stroke: var(--muted);
  }

  .exit.door {
    stroke: var(--c3);
  }

  .arrowhead {
    fill: var(--map-exit);
  }

  .room {
    cursor: pointer;
  }

  .room rect {
    fill: var(--map-room);
    stroke: var(--map-room-border);
    stroke-width: 1;
  }

  .room.aggro rect {
    stroke: var(--c9);
    stroke-width: 2;
  }

  .room.dt rect {
    fill: var(--c1);
  }

  .room.stacked rect {
    stroke: var(--danger);
    stroke-dasharray: 2 2;
  }

  .room.path rect {
    stroke: var(--c11);
    stroke-width: 2;
  }

  .room.current rect {
    fill: var(--accent);
    stroke: var(--text);
    stroke-width: 2;
  }

  .room:hover rect {
    stroke: var(--text);
  }

  .updown {
    fill: var(--text);
  }

  .note {
    fill: var(--c14);
  }
</style>
