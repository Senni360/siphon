import { mkdtemp, rm, readdir, stat, readFile } from 'node:fs/promises';
import { existsSync, createReadStream } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, dirname } from 'node:path';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { ROOT, Host, Report, frame, check, success, command, sleep, TERMINAL, skip } from './lib.mjs';
import { MEDIA } from './fixtures.mjs';

const args = new Set(process.argv.slice(2));
if (args.has('--help')) {
  console.log('node tests/e2e/host-protocol.mjs [--installed] [--live] [--local]\nDefault: offline protocol. --local: generated CC0 media over loopback; --live: CC/PD site probes and 8-second YouTube downloads. All output files are deleted.');
  process.exit(0);
}
const report = new Report(args.has('--installed') ? 'host-installed' : args.has('--live') ? 'host-live' : args.has('--local') ? 'host-local' : 'host-protocol');
const temp = await mkdtemp(join(tmpdir(), 'siphon-e2e-host-'));
const hosts = new Set(); let fixture;
function launch() {
  let host;
  if (args.has('--installed')) {
    const dir = join(process.env.LOCALAPPDATA, 'Siphon');
    const manifest = ['com.siphon.host.json', 'native-host.json'].map(f => join(dir, f)).find(existsSync);
    const launcher = manifest ? JSON.parse(requireText(manifest)).path : join(dir, 'siphon-host.cmd');
    check(existsSync(launcher), `Installed launcher missing: ${launcher}`);
    host = /\.(cmd|bat)$/i.test(launcher)
      ? new Host(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `""${launcher}" chrome-extension://bieofegakabfdjplcnpiaegdifmempmf/"`], { cwd: dirname(launcher), windowsVerbatimArguments: true })
      : new Host(launcher, ['chrome-extension://bieofegakabfdjplcnpiaegdifmempmf/'], { cwd: dirname(launcher) });
  } else {
    const python = resolve(ROOT, 'host/.venv/Scripts/python.exe');
    check(existsSync(python), `Missing ${python}`);
    host = new Host(python, ['-u', '-m', 'siphon_host'], { cwd: resolve(ROOT, 'host') });
  }
  hosts.add(host); return host;
}
// Read only this small installation manifest synchronously, before spawning.
import { readFileSync } from 'node:fs';
function requireText(path) { return readFileSync(path, 'utf8').replace(/^\uFEFF/, ''); }
const opts = (format, dir, extra = {}) => ({ format, quality: '320', outputDir: dir, filenameTemplate: '%(title)s.%(ext)s', embedMetadata: true, embedThumbnail: true, embedChapters: false, playlist: 'single', playlistSubfolder: false, cookiesFromBrowser: false, sponsorblockRemove: false, normalize: false, trim: { start: 0, end: 8 }, overwrite: false, rateLimit: null, ...extra });
function assertError(result, code) { check(result.type === 'result' && result.ok === false && result.error?.code === code && typeof result.error.message === 'string', `Expected ${code}: ${JSON.stringify(result)}`); }
function mediaInfo(info, playlist) {
  for (const key of ['url', 'webpageUrl', 'extractor', 'id', 'title', 'uploader']) check(typeof info[key] === 'string', `MediaInfo.${key} missing`);
  for (const key of ['duration', 'thumbnail']) check(key in info, `MediaInfo.${key} missing`);
  check(typeof info.isLive === 'boolean' && Array.isArray(info.formatsAudio), 'MediaInfo isLive/formatsAudio invalid');
  check(info.isPlaylist === playlist, `Expected isPlaylist=${playlist}, got ${info.isPlaylist}`);
  if (playlist) {
    check(Array.isArray(info.entries) && info.entries.length > 0 && info.entryCount >= info.entries.length, 'Missing flat entries/count');
    for (const entry of info.entries) check(typeof entry.url === 'string' && entry.url.length && typeof entry.title === 'string' && 'duration' in entry, `Invalid flat entry: ${JSON.stringify(entry)}`);
  }
}
async function files(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) out.push(...(entry.isDirectory() ? await files(join(dir, entry.name)) : [join(dir, entry.name)]));
  return out;
}
function progressCheck(host, jobId) {
  const events = host.messages.filter(m => m.type === 'job' && m.jobId === jobId);
  check(events.length > 0, 'No job events');
  let previous = 0;
  for (const event of events) {
    for (const key of ['state', 'progress', 'speed', 'eta', 'downloadedBytes', 'totalBytes', 'stage', 'item', 'file', 'files', 'error', 'title', 'thumbnail']) check(key in event, `job.${key} missing`);
    check(!('id' in event), 'Events must not contain request id');
    if (typeof event.progress === 'number') {
      check(event.progress >= 0 && event.progress <= 1, 'Progress outside [0,1]');
      check(event.progress + 0.000001 >= previous, `Progress regressed ${previous} → ${event.progress}`);
      previous = event.progress;
    }
  }
  // Count steady-state downloading progress separately from immediate state changes.
  const updates = events.filter((e, i) => e.state === 'downloading' && e.stage === 'downloading' && events[i - 1]?.state === 'downloading');
  for (let i = 5; i < updates.length; i++) check(updates[i].receivedMs - updates[i - 5].receivedMs >= 990, 'More than 5 progress updates in a rolling second');
  return `${events.length} events, ${updates.length} steady downloading updates`;
}
async function start(host, url, options) {
  const jobId = randomUUID(), at = performance.now();
  const accepted = success(await host.request('download', { jobId, url, options }, 10000));
  check(accepted.jobId === jobId && accepted.accepted === true, 'Invalid download acknowledgment');
  check(performance.now() - at < 3000, 'download acknowledgment took over 3s');
  return jobId;
}
const terminal = (host, id, timeout = 240000) => host.wait(m => m.type === 'job' && m.jobId === id && TERMINAL.has(m.state), timeout);
async function inspect(file, format, duration = 8, metadata = true, art = true) {
  check((await stat(file)).size > 0, 'Output is empty');
  const data = JSON.parse(await command('ffprobe', ['-v', 'error', '-show_format', '-show_streams', '-of', 'json', file]));
  const audio = data.streams.find(s => s.codec_type === 'audio'); check(audio, 'No audio stream');
  const expected = { mp3: 'mp3', m4a: 'aac', opus: 'opus', flac: 'flac', wav: 'pcm_s16le', best: null }[format];
  if (expected) check(audio.codec_name === expected, `${format}: codec=${audio.codec_name}, expected ${expected}`);
  check(data.streams.filter(s => s.codec_type === 'audio').length === 1, 'Unexpected audio stream count');
  const seconds = Number(data.format.duration || audio.duration);
  check(Math.abs(seconds - duration) < 1, `Duration ${seconds}, expected ${duration}±1s`);
  const bitrate = Number(audio.bit_rate || data.format.bit_rate);
  if (format === 'mp3') check(bitrate >= 300000 && bitrate <= 340000, `MP3 320 bitrate=${bitrate}`);
  if (['m4a', 'opus'].includes(format)) check(bitrate > 0, `Missing bitrate: ${format}`);
  const tags = Object.fromEntries(Object.entries({ ...data.format.tags, ...audio.tags }).map(([k, v]) => [k.toLowerCase(), v]));
  if (metadata) check(tags.title && tags.artist, `Missing title/artist tags: ${JSON.stringify(tags)}`);
  if (art && ['mp3', 'm4a', 'flac', 'opus'].includes(format)) check(data.streams.some(s => s.disposition?.attached_pic === 1), `No embedded cover art stream in ${format}`);
  return { codec: audio.codec_name, bitrate, seconds, title: tags.title, artist: tags.artist, attachedPicture: data.streams.some(s => s.disposition?.attached_pic === 1) };
}
async function localFixture() {
  const mp4 = join(temp, 'source.mp4'), mono = join(temp, 'mono.mp4'), wav = join(temp, 'slow.wav'), jpg = join(temp, 'cover.jpg');
  await command('ffmpeg', ['-v', 'error', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=16', '-ac', '2', '-ar', '48000', '-c:a', 'aac', '-b:a', '192k', '-metadata', 'artist=Siphon QA', '-metadata', 'title=Siphon CC0 tone', '-movflags', '+faststart', mp4]);
  await command('ffmpeg', ['-v', 'error', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=90', '-c:a', 'pcm_s16le', wav]);
  await command('ffmpeg', ['-v', 'error', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=16', '-c:a', 'aac', '-b:a', '128k', mono]);
  await command('ffmpeg', ['-v', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=128x128', '-frames:v', '1', jpg]);
  const server = createServer(async (req, res) => {
    try {
      const path = new URL(req.url, 'http://localhost').pathname;
      if (path === '/media' || path === '/mono') {
        const origin = `http://127.0.0.1:${server.address().port}`;
        const sourcePath = path === '/mono' ? 'mono.mp4' : 'source.mp4';
        res.setHeader('Content-Type', 'text/html');
        res.end(`<html><head><title>Siphon CC0 tone</title><meta property="og:title" content="Siphon CC0 tone"><meta property="og:image" content="${origin}/cover.jpg"><script type="application/ld+json">${JSON.stringify({ '@context': 'https://schema.org', '@type': 'VideoObject', name: 'Siphon CC0 tone', encodingFormat: 'audio/mp4', description: 'Generated sine wave, dedicated to CC0 by Siphon QA', uploadDate: '2026-01-01', duration: 'PT16S', thumbnailUrl: `${origin}/cover.jpg`, contentUrl: `${origin}/${sourcePath}`, author: { '@type': 'Person', name: 'Siphon QA' }, byArtist: { '@type': 'Person', name: 'Siphon QA' } })}</script></head><body>CC0 generated test media</body></html>`);
        return;
      }
      const selected = { '/source.mp4': mp4, '/mono.mp4': mono, '/slow.wav': wav, '/cover.jpg': jpg }[path];
      if (!selected) { res.setHeader('Content-Type', 'text/html'); res.end('<html><title>Unsupported plain page</title><body>No media here</body></html>'); return; }
      const size = (await stat(selected)).size;
      let start = 0, end = size - 1;
      const range = req.headers.range?.match(/bytes=(\d+)-(\d*)/);
      if (range) { start = Number(range[1]); end = range[2] ? Math.min(end, Number(range[2])) : end; res.statusCode = 206; res.setHeader('Content-Range', `bytes ${start}-${end}/${size}`); }
      res.setHeader('Accept-Ranges', 'bytes'); res.setHeader('Content-Length', end - start + 1);
      res.setHeader('Content-Type', path.endsWith('.jpg') ? 'image/jpeg' : path.endsWith('.wav') ? 'audio/wav' : 'video/mp4');
      if (req.method === 'HEAD') res.end();
      else { const stream = createReadStream(selected, { start, end }); res.on('close', () => stream.destroy()); stream.pipe(res); }
    } catch { res.destroy(); }
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}
let host;
try {
  await report.test('hello: required contract fields and clean framing', async () => {
    host = launch();
    const data = success(await host.request('hello', { extVersion: '1.0.0' }, 30000));
    for (const key of ['hostVersion', 'ytdlpVersion', 'defaultOutputDir', 'platform']) check(typeof data[key] === 'string' && data[key], `hello.${key} missing`);
    check(typeof data.ffmpeg?.found === 'boolean' && 'version' in data.ffmpeg && 'path' in data.ffmpeg, 'Invalid ffmpeg fields');
    check(data.ffmpeg.found, 'ffmpeg unavailable');
    check(data.jsRuntime === null || (typeof data.jsRuntime?.name === 'string' && typeof data.jsRuntime?.version === 'string'), 'Invalid jsRuntime');
    return JSON.stringify(data);
  });
  if (!host || host.failure) throw new Error('Host unavailable; remaining cases cannot run');
  await report.test('unknown request → BAD_REQUEST; host stays alive', async () => { assertError(await host.request('siphon.invalid'), 'BAD_REQUEST'); success(await host.request('hello', { extVersion: '1.0.0' })); });
  await report.test('listJobs shape', async () => { check(Array.isArray(success(await host.request('listJobs')).jobs), 'jobs must be an array'); });
  for (const [label, bytes] of [['invalid JSON', Buffer.from('{bad')], ['non-object JSON', Buffer.from('[]')], ['invalid UTF-8', Buffer.from([0xff])]]) {
    await report.test(`malformed frame: ${label} → BAD_REQUEST; process survives`, async () => {
      const h = launch(), after = h.messages.length; h.child.stdin.write(frame(bytes));
      const result = await h.wait(m => m.type === 'result', 10000, after); assertError(result, 'BAD_REQUEST');
      success(await h.request('hello', { extVersion: '1.0.0' })); await h.close();
    });
  }
  await report.test('zero-length frame → BAD_REQUEST without crash', async () => { const h = launch(); h.child.stdin.write(Buffer.alloc(4)); assertError(await h.wait(m => m.type === 'result', 10000), 'BAD_REQUEST'); check(!h.exited, 'Host crashed'); await h.close(); });
  await report.test('fragmented request frame', async () => {
    const h = launch(), id = randomUUID(), bytes = frame({ id, type: 'hello', extVersion: '1.0.0' });
    for (let i = 0; i < bytes.length; i += 3) h.child.stdin.write(bytes.subarray(i, i + 3));
    success(await h.wait(m => m.id === id, 30000)); await h.close();
  });
  if (args.has('--live')) {
    for (const [name, url] of Object.entries(MEDIA)) await report.test(`live probe: ${name}`, async () => { const data = success(await host.request('probe', { url, flatPlaylist: true }, 120000)); mediaInfo(data, ['youtubePlaylist', 'soundcloudSet', 'archive'].includes(name)); return `${data.extractor}: ${data.title}; ${data.entryCount ?? 'single'}`; });
  }
  if (args.has('--local') || args.has('--live')) {
    fixture = await localFixture();
    await report.test('unsupported HTML URL → UNSUPPORTED_URL', async () => assertError(await host.request('probe', { url: `${fixture.origin}/unsupported` }), 'UNSUPPORTED_URL'));
    const url = args.has('--live') ? MEDIA.youtube : `${fixture.origin}/media`;
    for (const format of ['mp3', 'm4a', 'opus', 'flac', 'wav', 'best']) {
      let id;
      await report.test(`${args.has('--live') ? 'live' : 'local'} download ${format}: codec, bitrate, duration, tags, artwork`, async () => {
        id = await start(host, url, opts(format, join(temp, format)));
        const done = await terminal(host, id); check(done.state === 'done', JSON.stringify(done.error));
        check(Array.isArray(done.files) && done.files.length && done.file === done.files.at(-1), 'Invalid terminal file/files');
        const output = resolve(done.file); check(output.startsWith(resolve(temp) + '\\') || output.startsWith(resolve(temp) + '/'), 'Host wrote outside temporary output directory');
        return JSON.stringify(await inspect(output, format));
      });
      if (id) await report.test(`${format}: progress monotonic and ≤5/s`, () => progressCheck(host, id));
    }
    await report.test('trim option 2..7s → 5s output', async () => { const id = await start(host, url, opts('wav', join(temp, 'trim'), { trim: { start: 2, end: 7 }, embedThumbnail: false })); const done = await terminal(host, id); check(done.state === 'done', JSON.stringify(done.error)); return JSON.stringify(await inspect(done.file, 'wav', 5)); });
    await report.test('best without trim preserves encoded audio packets', async () => {
      const id = await start(host, `${fixture.origin}/media`, opts('best', join(temp, 'best-preserve'), { trim: null }));
      const done = await terminal(host, id); check(done.state === 'done', JSON.stringify(done.error));
      const hash = file => command('ffmpeg', ['-v', 'error', '-i', file, '-map', '0:a:0', '-c', 'copy', '-f', 'hash', '-hash', 'sha256', '-']);
      const original = await hash(join(temp, 'source.mp4')), downloaded = await hash(done.file);
      check(original === downloaded, 'best re-encoded or changed native audio packets');
      return 'AAC packet SHA256 matches source';
    });
    await report.test('mono AAC → Opus with default 320 quality', async () => {
      const id = await start(host, `${fixture.origin}/mono`, opts('opus', join(temp, 'mono-opus')));
      const done = await terminal(host, id); check(done.state === 'done', JSON.stringify(done.error));
      return JSON.stringify(await inspect(done.file, 'opus'));
    });
    await report.test('cancel while downloading; listJobs active; no .part files', async () => {
      const dir = join(temp, 'cancel'), id = await start(host, `${fixture.origin}/slow.wav`, opts('best', dir, { trim: null, rateLimit: '32K', embedThumbnail: false }));
      try {
      await host.wait(m => m.jobId === id && m.state === 'downloading' && m.downloadedBytes > 0, 30000);
      // A sustained transfer exercises actual throttling, unlike ffmpeg's
      // start/finish-only range download hooks used by the short format clips.
      await sleep(4500);
      const steady = host.messages.filter(m => m.type === 'job' && m.jobId === id && m.state === 'downloading');
      check(steady.length >= 6, `Too few progress samples to validate throttling (${steady.length})`);
      check(success(await host.request('listJobs')).jobs.some(j => j.jobId === id && !TERMINAL.has(j.state)), 'Active job not listed');
      const result = success(await host.request('cancel', { jobId: id })); check(result.jobId === id && result.cancelled === true, 'Cancel not acknowledged');
      const done = await terminal(host, id, 15000); check(done.state === 'cancelled', `Cancellation state ${done.state}`);
      await sleep(500); check(!(await files(dir)).some(f => /\.part(?:$|\.)/.test(f)), 'Partial files left behind');
      return progressCheck(host, id);
      } finally { await host.request('cancel', { jobId: id }).catch(() => {}); await terminal(host, id, 15000).catch(() => {}); }
    });
    await report.test('concurrency: three downloading, fourth queued; queued cancellation', async () => {
      const ids = [];
      try {
        for (let i = 0; i < 4; i++) ids.push(await start(host, `${fixture.origin}/slow.wav`, opts('best', join(temp, `parallel-${i}`), { trim: null, rateLimit: '32K', embedThumbnail: false })));
        for (const id of ids.slice(0, 3)) await host.wait(m => m.jobId === id && m.state === 'downloading' && m.downloadedBytes > 0, 30000);
        const jobs = success(await host.request('listJobs')).jobs;
        check(jobs.filter(j => ids.slice(0, 3).includes(j.jobId) && j.state === 'downloading').length === 3, 'First three are not all active');
        check(jobs.some(j => j.jobId === ids[3] && j.state === 'queued'), 'Fourth job is not queued');
        const response = success(await host.request('cancel', { jobId: ids[3] })); check(response.cancelled, 'Queued cancel failed');
        check((await terminal(host, ids[3], 10000)).state === 'cancelled', 'Queued job not cancelled');
      } finally {
        for (const id of ids) { await host.request('cancel', { jobId: id }).catch(() => {}); await terminal(host, id, 15000).catch(() => {}); }
      }
      await sleep(300); check(!(await files(temp)).some(f => /\.part(?:$|\.)/.test(f)), 'Concurrent cancellation left .part files');
    });
  } else report.skip('media probes/downloads, trim, cancellation, concurrency', 'Run --local and --live');
} catch (error) { await report.test('suite prerequisites/orchestration', () => { throw error; }); }
finally {
  for (const h of hosts) await h.close();
  if (fixture) { fixture.server.closeAllConnections(); await new Promise(r => fixture.server.close(r)); }
  await rm(temp, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  await report.finish({ installed: args.has('--installed'), live: args.has('--live'), generatedFixture: Boolean(fixture), cleanedTemporaryOutput: !existsSync(temp) });
}
