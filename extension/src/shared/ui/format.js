/** Stable, dependency-free display helpers. Durations/ETA use seconds; bytes use SI units. */
export function formatBytes(value, decimals = 1) {
  if (value == null || !Number.isFinite(Number(value)) || Number(value) < 0) return '—';
  const n = Number(value);
  const unit = Math.min(4, Math.floor(Math.log10(Math.max(1, n)) / 3));
  return `${(n / 1000 ** unit).toFixed(unit ? decimals : 0)} ${['B', 'KB', 'MB', 'GB', 'TB'][unit]}`;
}
export function formatDuration(value) {
  if (value == null || !Number.isFinite(Number(value)) || Number(value) < 0) return '—';
  const n = Math.floor(Number(value));
  const s = String(n % 60).padStart(2, '0');
  return n >= 3600 ? `${Math.floor(n / 3600)}:${String(Math.floor(n / 60) % 60).padStart(2, '0')}:${s}` : `${Math.floor(n / 60)}:${s}`;
}
export function formatEta(value) { return value == null ? '—' : formatDuration(value); }
export function formatSpeed(value) { return value == null ? '—' : `${formatBytes(value)}/s`; }
export function formatPercent(value) { return value == null || !Number.isFinite(Number(value)) ? '—' : `${Math.round(Math.max(0, Math.min(1, Number(value))) * 100)}%`; }
/** Empty => null; invalid => NaN. Supports seconds, mm:ss, or hh:mm:ss. */
export function parseTime(value) {
  const s = String(value).trim();
  if (!s) return null;
  if (!/^\d+(?::[0-5]\d){0,2}$/.test(s)) return NaN;
  const seconds = s.split(':').reduce((n, part) => n * 60 + Number(part), 0);
  return Number.isSafeInteger(seconds) ? seconds : NaN;
}
