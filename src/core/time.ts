/**
 * 时间分隔与时间文案（FR-19 / §3.10）。
 *
 * 关键约束（踩坑清单 #11）：**不做周期性重绘**。所以这里全部是静态文本函数，
 * 不提供「几分钟前」这类需要每秒刷新的相对时间。
 */

/** 相邻消息间隔超过 5 分钟才插入时间分隔 */
export const SEPARATOR_GAP_MS = 5 * 60 * 1000;

export type Separator = { kind: 'day' | 'time'; text: string } | null;

const DAY_NAMES = ['日', '一', '二', '三', '四', '五', '六'];

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

export function formatClock(ts: number): string {
  const d = new Date(ts);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

export function formatDay(ts: number, now = Date.now()): string {
  const d = new Date(ts);
  const n = new Date(now);
  const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round((startOfDay(n) - startOfDay(d)) / 86_400_000);

  if (diffDays === 0) return '今天';
  if (diffDays === 1) return '昨天';
  if (diffDays === 2) return '前天';
  if (diffDays > 2 && diffDays < 7) return `周${DAY_NAMES[d.getDay()]}`;
  if (d.getFullYear() === n.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日`;
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`;
}

export function sameDay(a: number, b: number): boolean {
  const x = new Date(a);
  const y = new Date(b);
  return (
    x.getFullYear() === y.getFullYear() &&
    x.getMonth() === y.getMonth() &&
    x.getDate() === y.getDate()
  );
}

/**
 * 计算某条消息上方需要什么分隔。
 * @param prevTs 上一条消息的时间戳；该消息是本页第一条时传 null
 *
 * 规则：跨天（或本页首条）→ 日期分隔并带上时刻；同天但间隔 > 5 分钟 → 只显示时刻。
 */
export function separatorFor(prevTs: number | null, ts: number, now = Date.now()): Separator {
  if (prevTs === null || !sameDay(prevTs, ts)) {
    return { kind: 'day', text: `${formatDay(ts, now)} ${formatClock(ts)}` };
  }
  if (ts - prevTs > SEPARATOR_GAP_MS) {
    return { kind: 'time', text: formatClock(ts) };
  }
  return null;
}

/**
 * 消息列表的锚点记忆（FR-20）。
 * 向上翻页插入历史后，用「插入高度」把滚动位置顶回去，保持视口稳定。
 */
export function prependScrollTop(prevScrollTop: number, insertedHeight: number): number {
  return Math.max(0, prevScrollTop + insertedHeight);
}
