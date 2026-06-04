// Helpers for the app's naive-local ISO strings ("YYYY-MM-DDTHH:MM:SS").

const pad = (n: number) => String(n).padStart(2, "0");

/** Serialize a Date to a naive-local ISO string (no timezone). */
export function toLocalIso(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Parse a naive-local ISO string into a local Date (unambiguous). */
export function parseLocal(iso: string): Date {
  const [datePart, timePart = "00:00:00"] = iso.split("T");
  const [y, mo, da] = datePart.split("-").map(Number);
  const [h, mi, s] = timePart.split(":").map(Number);
  return new Date(y, mo - 1, da, h, mi, s || 0);
}

export function addMinutes(d: Date, mins: number): Date {
  return new Date(d.getTime() + mins * 60000);
}

export function minutesBetween(a: Date, b: Date): number {
  return Math.round((b.getTime() - a.getTime()) / 60000);
}

export function startOfWeek(d: Date): Date {
  // Monday-based week.
  const date = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const day = (date.getDay() + 6) % 7; // 0 = Monday
  date.setDate(date.getDate() - day);
  return date;
}

/** Add whole days via calendar arithmetic (DST-safe, unlike adding 1440 minutes). */
export function addDays(d: Date, days: number): Date {
  const date = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  date.setDate(date.getDate() + days);
  return date;
}

export function startOfMonth(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), 1);
}

export function sameMonth(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth();
}

/** Monday-based weekday index: 0 = Mon … 6 = Sun. */
export function mondayIndex(d: Date): number {
  return (d.getDay() + 6) % 7;
}

export function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

export function fmtTime(d: Date): string {
  return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

export function fmtDayLabel(d: Date): string {
  return d.toLocaleDateString([], { weekday: "short", day: "numeric" });
}

const HUMAN = (m: number) => {
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h}h ${r}m` : `${h}h`;
};
export const humanMinutes = HUMAN;
