// Evidence checker only; Node is not used by the Rust separation engine.
// Usage: node helper/tools/stems-verify.mjs input.wav stems-directory
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
const [input, directory] = process.argv.slice(2);
function wav(filename) {
  const b = fs.readFileSync(filename);
  if (b.toString('ascii', 0, 4) !== 'RIFF' || b.toString('ascii', 8, 12) !== 'WAVE') throw Error('Invalid WAV');
  let format, data;
  for (let p = 12; p + 8 <= b.length;) {
    const name = b.toString('ascii', p, p + 4), size = b.readUInt32LE(p + 4);
    p += 8;
    if (p + size > b.length) throw Error('Truncated WAV');
    if (name === 'fmt ') format = {codec:b.readUInt16LE(p),channels:b.readUInt16LE(p+2),rate:b.readUInt32LE(p+4),bits:b.readUInt16LE(p+14)};
    if (name === 'data') data = b.subarray(p, p + size);
    p += size + size % 2;
  }
  if (!data?.length || format?.codec !== 1 || format.channels !== 2 || format.rate !== 44100 || format.bits !== 16 || data.length % 4) throw Error('Expected stereo PCM16/44100');
  const samples = Float64Array.from({length:data.length/2}, (_,i)=>data.readInt16LE(i*2)/32768);
  const rms = Math.sqrt(samples.reduce((s,x)=>s+x*x,0)/samples.length);
  return {format,frames:samples.length/2,seconds:samples.length/88200,rms,sha256:crypto.createHash('sha256').update(b).digest('hex'),samples};
}
const original = wav(input);
const stems = Object.fromEntries(['vocals','drums','bass','other'].map(s=>[s,wav(path.join(directory,`${s}.wav`))]));
for (const stem of Object.values(stems)) {
  if (stem.frames !== original.frames || stem.rms < 1e-6) throw Error('Empty or mismatched stem');
}
if (new Set(Object.values(stems).map(s=>s.sha256)).size !== 4) throw Error('Identical stems');
let residual = 0;
for (let i=0;i<original.samples.length;i++) {
  const d = original.samples[i] - Object.values(stems).reduce((s,x)=>s+x.samples[i],0);
  residual += d*d;
}
const withoutSamples = ({samples,...rest})=>rest;
console.log(JSON.stringify({input:withoutSamples(original),stems:Object.fromEntries(Object.entries(stems).map(([k,v])=>[k,withoutSamples(v)])),sumResidualRms:Math.sqrt(residual/original.samples.length)},null,2));
