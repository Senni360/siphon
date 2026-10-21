import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const TERMINAL = new Set(['done', 'error', 'cancelled']);
export const sleep = ms => new Promise(r => setTimeout(r, ms));
export function check(condition, message) { if (!condition) throw new Error(message); }
export function frame(value) {
  const body = Buffer.isBuffer(value) ? value : Buffer.from(JSON.stringify(value));
  const header = Buffer.alloc(4); header.writeUInt32LE(body.length);
  return Buffer.concat([header, body]);
}
export class FrameDecoder {
  buffer = Buffer.alloc(0);
  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    const messages = [];
    while (this.buffer.length >= 4) {
      const size = this.buffer.readUInt32LE();
      check(size > 0 && size <= 1048576, `Invalid response frame length ${size}; stdout may be polluted`);
      if (this.buffer.length < size + 4) break;
      messages.push(JSON.parse(this.buffer.subarray(4, size + 4).toString('utf8')));
      this.buffer = this.buffer.subarray(size + 4);
    }
    return messages;
  }
}
export class Host extends EventEmitter {
  messages = []; stderr = ''; pending = new Map(); decoder = new FrameDecoder();
  constructor(command, args, options) {
    super(); this.started = performance.now();
    this.child = spawn(command, args, { windowsHide: true, ...options, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.stdin.on('error', e => this.fail(e));
    this.child.on('error', e => this.fail(e));
    this.child.stderr.on('data', b => { this.stderr = (this.stderr + b).slice(-12000); });
    this.child.stdout.on('data', b => {
      try {
        for (const message of this.decoder.push(b)) {
          const entry = { ...message, receivedMs: performance.now() - this.started };
          this.messages.push(entry); this.emit('message', entry);
          const pending = this.pending.get(message.id);
          if (message.type === 'result' && pending) { clearTimeout(pending.timer); this.pending.delete(message.id); pending.resolve(message); }
        }
      } catch (error) { this.fail(error); }
    });
    this.child.on('exit', (code, signal) => {
      this.exited = true;
      this.fail(new Error(`Host exited ${code ?? signal}. ${this.stderr}`));
    });
  }
  fail(error) {
    this.failure = error;
    for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); }
    this.pending.clear(); this.emit('failure', error);
  }
  request(type, payload = {}, timeout = 60000) {
    if (this.failure) return Promise.reject(this.failure);
    const id = randomUUID();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`${type} timed out after ${timeout}ms. ${this.stderr}`)); }, timeout);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(frame({ id, type, ...payload }));
    });
  }
  wait(predicate, timeout = 180000, after = 0) {
    const existing = this.messages.slice(after).find(predicate);
    if (existing) return Promise.resolve(existing);
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => {
      const cleanup = () => { clearTimeout(timer); this.off('message', listener); this.off('failure', fail); };
      const listener = message => { if (predicate(message)) { cleanup(); resolve(message); } };
      const fail = error => { cleanup(); reject(error); };
      const timer = setTimeout(() => { cleanup(); reject(new Error(`Event timed out after ${timeout}ms`)); }, timeout);
      this.on('message', listener); this.on('failure', fail);
    });
  }
  async close() {
    if (this.exited) return;
    this.child.stdin.end();
    for (let i = 0; i < 30 && !this.exited; i++) await sleep(100);
    if (!this.exited) this.child.kill();
    for (let i = 0; i < 20 && !this.exited; i++) await sleep(100);
  }
}
export class Report {
  rows = []; started = performance.now();
  constructor(name) { this.name = name; }
  async test(name, fn) {
    const start = performance.now();
    let status = 'PASS', detail = '';
    try { detail = String(await fn() ?? ''); } catch (e) { status = e.skip ? 'SKIP' : 'FAIL'; detail = e.stack || e.message; }
    const row = { test: name, status, seconds: +((performance.now() - start) / 1000).toFixed(2), detail };
    this.rows.push(row); console.log(`${status.padEnd(4)} ${name} (${row.seconds}s) ${detail.split('\n')[0]}`);
  }
  skip(name, detail) { this.rows.push({ test: name, status: 'SKIP', seconds: 0, detail }); console.log(`SKIP ${name}: ${detail}`); }
  async finish(extra = {}) {
    console.table(this.rows.map(({ test, status, seconds }) => ({ test, status, seconds })));
    const result = { suite: this.name, at: new Date().toISOString(), seconds: +((performance.now() - this.started) / 1000).toFixed(2), rows: this.rows, ...extra };
    const dir = resolve(ROOT, 'tests/artifacts'); await mkdir(dir, { recursive: true });
    await writeFile(resolve(dir, `${this.name}.json`), JSON.stringify(result, null, 2));
    process.exitCode = this.rows.some(r => r.status === 'FAIL') ? 1 : 0;
    return result;
  }
}
export function skip(message) { const e = new Error(message); e.skip = true; throw e; }
export function success(result) { check(result.type === 'result' && result.ok === true, JSON.stringify(result)); return result.data; }
export async function command(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { windowsHide: true, ...options });
    let stdout = '', stderr = '';
    child.stdout.on('data', b => stdout += b); child.stderr.on('data', b => stderr += b);
    child.on('error', reject);
    child.on('exit', code => code === 0 ? resolve(stdout) : reject(new Error(`${command} exited ${code}: ${stderr}`)));
  });
}
