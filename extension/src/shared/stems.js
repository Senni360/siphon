// Matching keys only: keep the original history/host paths for native operations.
export function stemPathKey(path) {
  if (typeof path !== 'string') return '';
  let key = path.replace(/\\/g, '/');
  if (key.startsWith('//?/')) key = key.slice(4).replace(/^UNC\//i, '//');
  return /^(?:[a-z]:\/|\/\/)/i.test(key) ? key.toLowerCase() : path;
}
export function stemPaths(item) {
  const paths = [...(item.files || []), item.file].filter(path => typeof path === 'string' && path);
  return [...new Map(paths.map(path => [stemPathKey(path), path])).values()];
}
export const activeStem = job => ['queued', 'running'].includes(job.state);
export function latestStem(item, path, stems = []) {
  const time = value => typeof value === 'number' ? value : Date.parse(value) || 0;
  return stems.filter(job => job.sourceFile ? stemPathKey(job.sourceFile) === stemPathKey(path) : stemPaths(item).length === 1 && job.sourceId === item.id)
    .sort((a, b) => Number(activeStem(b)) - Number(activeStem(a)) || time(b.createdAt) - time(a.createdAt))[0];
}
