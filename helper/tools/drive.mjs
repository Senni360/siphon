// Drive a Siphon helper for one download: node drive.mjs <exe> <url> <outputDir> [optionsJSON]
// Prints every message; exits when the job is terminal and pending analyses have arrived.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const [exe, url, outputDir, extra = '{}'] = process.argv.slice(2);
if (!exe || !url || !outputDir) {
  console.error('Usage: node drive.mjs <exe> <url> <outputDir> [optionsJSON]');
  process.exit(1);
}
const optionsOverride = JSON.parse(extra);
const child = spawn(exe, ['chrome-extension://test/'], { windowsHide: true, stdio: ['pipe', 'pipe', 'inherit'] });
let buffer = Buffer.alloc(0), done = false, pending = 0, closing = false;
const deadline = setTimeout(() => finish(1, 'Host did not finish within five minutes.'), 300000);
function finish(code, message) {
  if (closing) return;
  closing = true;
  process.exitCode = code;
  if (message) console.error(message);
  clearTimeout(deadline);
  child.stdin.end();
  // EOF should close the native host and its process trees. Bound failure to exit.
  const force = setTimeout(() => child.kill(), 5000);
  force.unref();
}
child.on('error', error => finish(1, error.message));
child.stdin.on('error', error => { if (!closing) finish(1, error.message); });
child.on('close', (code, signal) => {
  clearTimeout(deadline);
  if (!closing || code !== 0) {
    process.exitCode = 1;
    console.error(`Host exited before clean completion (${code ?? signal}).`);
  }
});
const send = message => { const body = Buffer.from(JSON.stringify(message)); const header = Buffer.alloc(4); header.writeUInt32LE(body.length); child.stdin.write(Buffer.concat([header, body])); };
child.stdout.on('data', chunk => {
  try {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const size = buffer.readUInt32LE(0);
      if (size === 0 || size > 1048576) throw new Error('Invalid native message length.');
      if (buffer.length < size + 4) break;
      const message = JSON.parse(buffer.subarray(4, 4 + size)); buffer = buffer.subarray(4 + size);
      const { options, analyses, ...rest } = message;
      console.log(JSON.stringify(rest).slice(0, 600));
      if (message.type === 'result' && !message.ok) { finish(1, message.error?.message || 'Request rejected.'); return; }
      if (message.type === 'result' && message.id === 'hello') send({ id: 'dl', type: 'download', jobId: randomUUID(), url, options: { outputDir, ...optionsOverride } });
      if (message.type === 'job' && ['error', 'cancelled'].includes(message.state)) { finish(1, message.error?.message || 'Download cancelled.'); return; }
      if (message.type === 'job' && message.state === 'done') { done = true; pending = message.analysisPending || 0; }
      if (message.type === 'analysis') pending = message.pending;
      if (done && !pending) { finish(0); return; }
    }
  } catch (error) { finish(1, error.message); }
});
send({ id: 'hello', type: 'hello', extVersion: 'drive', protocolVersion: 1 });
