<script lang="ts">
  // Состояние персонажа по MSDP: полоски жизни и бодрости, уровень, опыт, деньги и группа.
  import type { CharStatus } from "./session";

  let { status }: { status: CharStatus } = $props();

  const fmt = (n: number | null) => (n === null ? "?" : n.toLocaleString("ru-RU"));

  /** Доля 0…1 для полоски; без максимума — полная. */
  function part(cur: number | null, max: number | null): number {
    if (cur === null) return 0;
    if (!max || max <= 0) return 1;
    return Math.max(0, Math.min(1, cur / max));
  }

  /** Цвет жизни: зелёный → жёлтый → красный. */
  function healthColor(p: number): string {
    if (p > 0.66) return "var(--c10)";
    if (p > 0.33) return "var(--c11)";
    return "var(--c9)";
  }

  /** Секунды запоминания → «1:05». */
  function memTime(sec: number): string {
    if (sec <= 0) return "";
    return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;
  }

  let hp = $derived(part(status.hp, status.max_hp));
  let mv = $derived(part(status.moves, status.max_moves));
  /** В группе сервер присылает и самого игрока — показываем список, только если есть кто-то ещё. */
  let group = $derived(status.group.length > 1 ? status.group : []);
</script>

<section class="status">
  {#if status.hp !== null}
    <div class="bar" title="Жизнь">
      <div class="fill" style:width="{hp * 100}%" style:background={healthColor(hp)}></div>
      <span>Жизнь {fmt(status.hp)}{status.max_hp !== null ? ` / ${fmt(status.max_hp)}` : ""}</span>
    </div>
  {/if}
  {#if status.moves !== null}
    <div class="bar" title="Бодрость">
      <div class="fill moves" style:width="{mv * 100}%"></div>
      <span>Бодрость {fmt(status.moves)}{status.max_moves !== null ? ` / ${fmt(status.max_moves)}` : ""}</span>
    </div>
  {/if}
  <div class="facts">
    {#if status.level !== null}<span title="Уровень">ур. {status.level}</span>{/if}
    {#if status.mana !== null}<span title="Мана">мана {fmt(status.mana)}</span>{/if}
    {#if status.exp !== null}<span title="Опыт">опыт {fmt(status.exp)}</span>{/if}
    {#if status.gold !== null}
      <span title="Золото: в кармане / в банке"
        >кун {fmt(status.gold)}{status.bank ? ` / банк ${fmt(status.bank)}` : ""}</span
      >
    {/if}
  </div>
  {#if group.length}
    <table class="group">
      <tbody>
        {#each group as m, i (i)}
          <tr class:away={!m.here} title="{m.position}{m.here ? '' : ', не здесь'}">
            <td class="name">{m.role === "leader" ? "★ " : ""}{m.name}</td>
            <td class="mini">
              <div class="mini-bar">
                <div class="fill" style:width="{m.health}%" style:background={healthColor(m.health / 100)}></div>
              </div>
              <div class="mini-bar">
                <div class="fill moves" style:width="{m.moves}%"></div>
              </div>
            </td>
            <td class="aff" title="Аффекты">{m.affects}</td>
            <td class="mem" title="До конца запоминания">{memTime(m.mem_time)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .status {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
  }

  .bar {
    position: relative;
    height: 16px;
    background: var(--input-bg);
    border: 1px solid var(--panel-border);
    border-radius: 2px;
    overflow: hidden;
  }

  .bar .fill {
    position: absolute;
    inset: 0 auto 0 0;
    opacity: 0.55;
  }

  .fill.moves {
    background: var(--c12);
  }

  .bar span {
    position: relative;
    padding-left: 5px;
    line-height: 16px;
    color: var(--c15);
    white-space: nowrap;
  }

  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
    color: var(--muted);
  }

  .group {
    border-collapse: collapse;
    width: 100%;
  }

  .group td {
    padding: 1px 3px 1px 0;
    white-space: nowrap;
  }

  .group .name {
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .group tr.away {
    opacity: 0.5;
  }

  .mini {
    width: 60px;
  }

  .mini-bar {
    position: relative;
    height: 4px;
    margin: 1px 0;
    background: var(--input-bg);
  }

  .mini-bar .fill {
    position: absolute;
    inset: 0 auto 0 0;
  }

  .aff {
    color: var(--c14);
  }

  .mem {
    color: var(--muted);
    text-align: right;
  }
</style>
