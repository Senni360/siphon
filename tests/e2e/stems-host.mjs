// Actual native protocol + model inference. Uses a licensed local fixture and
// the managed runtime cache; never changes the installed helper registration.
// SIPHON_STEMS_INPUT=<short stereo WAV>; SIPHON_HOST_BINARY=<release host>.
import { mkdtemp, copyFile, readFile, readdir, rm } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { Host, Report, ROOT, check, success, command } from './lib.mjs';

const binary = resolve(process.env.SIPHON_HOST_BINARY || join(ROOT, 'helper/target/distribution/x86_64-pc-windows-msvc/release/siphon-host.exe'));
const fixture = resolve(process.env.SIPHON_STEMS_INPUT || join(tmpdir(), 'siphon-parity/music.wav'));
const root = await mkdtemp(join(tmpdir(), 'siphon-stems-integration-'));
const input = join(root, 'Música & test.track.wav');
const report = new Report('rust-stems-live');
const host = new Host(binary, [], { cwd: ROOT });
const digest = async path => createHash('sha256').update(await readFile(path)).digest('hex');
let original, completed;
const first = randomUUID();
try {
  await copyFile(fixture, input);
  original = await digest(input);
  await report.test('capability and strict local-input validation', async () => {
    check(success(await host.request('hello', { protocolVersion: 1, extVersion: 'stems-test' })).stems === true, 'No stem capability');
    for (const path of ['relative.wav', root, join(root, 'missing.wav')]) {
      check((await host.request('splitStems', { jobId: randomUUID(), path })).ok === false, `Accepted ${path}`);
    }
  });
  await report.test('real split admission, responsive protocol, queued cancellation', async () => {
    success(await host.request('splitStems', { jobId: first, path: input }));
    await host.wait(m => m.type === 'stem' && m.jobId === first && m.state === 'running');
    const start = performance.now();
    success(await host.request('listJobs'));
    check(performance.now() - start < 2000, 'Protocol stalled during separation');
    check((await host.request('splitStems', { jobId: first, path: input })).ok === false, 'Accepted duplicate job id');
    const queued = randomUUID();
    success(await host.request('splitStems', { jobId: queued, path: input }));
    check(success(await host.request('cancelStems', { jobId: queued })).cancelled, 'Could not cancel queue entry');
    await host.wait(m => m.type === 'stem' && m.jobId === queued && m.state === 'cancelled');
    check(host.messages.findIndex(m => m.type === 'result' && m.data?.jobId === first) < host.messages.findIndex(m => m.type === 'stem' && m.jobId === first), 'Event preceded admission');
  });
  await report.test('real four-stem output, matching duration, source preservation', async () => {
    completed = await host.wait(m => m.type === 'stem' && m.jobId === first && ['done', 'error', 'cancelled'].includes(m.state), 600000);
    check(completed.state === 'done', JSON.stringify(completed.error));
    check(completed.files.length === 4, 'Wrong output count');
    check((await readdir(completed.outputDir)).sort().join() === ['bass.wav', 'drums.wav', 'other.wav', 'vocals.wav'].join(), 'Unexpected output files');
    check(await digest(input) === original, 'Original changed');
    const evidence = JSON.parse(await command(process.execPath, [join(ROOT, 'helper/tools/stems-verify.mjs'), input, completed.outputDir]));
    check(evidence.input.frames > 0 && evidence.sumResidualRms < .05, 'Unexpected reconstruction error');
    return JSON.stringify({ frames: evidence.input.frames, seconds: evidence.input.seconds, sumResidualRms: evidence.sumResidualRms, stages: [...new Set(host.messages.filter(m => m.type === 'stem' && m.jobId === first).map(m => m.stage))] });
  });
  await report.test('running inference cancellation cleans staging and preserves finished output', async () => {
    check(completed?.state === 'done', 'Successful split prerequisite failed');
    const id = randomUUID();
    success(await host.request('splitStems', { jobId: id, path: input }));
    await host.wait(m => m.type === 'stem' && m.jobId === id && /separating/.test(m.stage), 180000);
    const started = performance.now();
    check(success(await host.request('cancelStems', { jobId: id })).cancelled, 'Cancellation not accepted');
    await host.wait(m => m.type === 'stem' && m.jobId === id && m.state === 'cancelled', 20000);
    check(performance.now() - started < 20000, 'Slow cancellation');
    check(!(await readdir(root)).some(name => name.startsWith('.siphon-stems-')), 'Staging leaked');
    check((await readdir(completed.outputDir)).length === 4, 'Previous output damaged');
    check(await digest(input) === original, 'Input changed on cancellation');
    const events = host.messages.filter(m => m.type === 'stem' && m.jobId === id);
    check(events.at(-1).state === 'cancelled', 'Late progress after terminal event');
  });
} finally {
  await host.close();
  await report.finish({ binary, fixture, sourceSha256: original });
  const absolute = resolve(root);
  if (!absolute.startsWith(resolve(tmpdir()) + '\\') || !absolute.includes('siphon-stems-integration-')) throw Error('Unsafe test cleanup path');
  await rm(absolute, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
}
