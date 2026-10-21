// Node 24 treats an explicit directory passed to --test as a module entry.
// Keep `node --test tests/` working while discovering each agent's test files.
import { readdir } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('.', import.meta.url));
const entries = await readdir(root, { recursive: true, withFileTypes: true });
for (const entry of entries.filter(entry => entry.isFile() && /\.test\.[cm]?js$/.test(entry.name)).sort((a, b) => join(a.parentPath, a.name).localeCompare(join(b.parentPath, b.name)))) {
  await import(pathToFileURL(join(entry.parentPath, entry.name)).href);
}
