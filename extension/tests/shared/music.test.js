import test from 'node:test';
import assert from 'node:assert/strict';
import { keyNames, keyColor, formatBpm, formatKey, formatSuffix, previewFilename, alternateBpm, confidenceHint, SAMPLE_ANALYSIS as a } from '../../src/shared/music.js';
import { DEFAULT_SETTINGS, toDownloadOptions } from '../../src/shared/settings.js';

test('all 24 keys map to Camelot, Open Key and twelve shared relative-key colors', () => {
  const major = ['B','Gb','Db','Ab','Eb','Bb','F','C','G','D','A','E'];
  const minor = ['G#m','D#m','Bbm','Fm','Cm','Gm','Dm','Am','Em','Bm','F#m','C#m'];
  const colors = new Set();
  for (let i = 0; i < 12; i++) {
    const ma = keyNames(major[i]), mi = keyNames(minor[i]);
    assert.equal(ma.camelot,`${i+1}B`); assert.equal(mi.camelot,`${i+1}A`);
    assert.equal(ma.openKey,`${(i+5)%12+1}d`); assert.equal(mi.openKey,`${(i+5)%12+1}m`);
    assert.equal(keyColor(ma.camelot),keyColor(mi.camelot)); colors.add(keyColor(ma.camelot));
  }
  assert.equal(colors.size,12); assert.equal(keyColor('13A'),null);
  assert.equal(keyNames('F♯ minor').keyShort,'F#m'); assert.equal(keyNames('C# major').keyShort,'Db');
});
test('BPM rounding, notation, confidence and half/double hints preserve uncertainty', () => {
  assert.equal(formatBpm(128.04),'128'); assert.equal(formatBpm(128.1),'128.1');
  assert.equal(formatBpm(128.25),'128.2'); assert.equal(formatBpm(128.75),'128.8');
  for (const v of [null,undefined,NaN,Infinity,0,-1,'128']) assert.equal(formatBpm(v),'');
  assert.equal(formatKey(a,'standard'),'Am'); assert.equal(formatKey(a,'camelot'),'8A'); assert.equal(formatKey(a,'openkey'),'1m');
  assert.deepEqual(alternateBpm(a),['64']);
  assert.deepEqual(alternateBpm({...a,bpmCandidates:[{bpm:128,score:1},{bpm:64,score:.2},{bpm:127,score:.9}]}),[]);
  assert.match(confidenceHint({...a,keyConfidence:.3}),/key.*lower confidence/);
});
test('suffix formatting matches host placeholders, braces, sanitization and bounds', () => {
  assert.equal(formatSuffix(a),' [128 BPM Am]');
  assert.equal(formatSuffix(a,undefined,'camelot'),' [128 BPM 8A]');
  assert.equal(formatSuffix(a,undefined,'openkey'),' [128 BPM 1m]');
  assert.equal(formatSuffix(a,' {keyShort} {keyLong} {camelot} {openkey}'),' Am A minor 8A 1m');
  assert.equal(formatSuffix(a,'{{{bpm}}}'),'{128}');
  assert.equal(formatSuffix(a,'{bpm:}'),'128');
  assert.equal(formatSuffix(a,' <>:"/\\|?*\x00\x1f\x7f .'),' ____________');
  assert.equal(formatSuffix(a,'x'.repeat(200)).length,160);
  assert.equal(formatSuffix({...a,bpm:128.3},'{bpm}'),'128');
  assert.equal(formatSuffix({...a,bpm:128.3},'{bpmExact}'),'128.3');
  assert.equal(formatSuffix({...a,bpm:127.5},'{bpm}'),'128');
  for (const template of ['{unknown}','{bpm:.1f}','{key!r}','{key.foo}','{','}']) assert.throws(() => formatSuffix(a,template));
  assert.throws(() => formatSuffix(a,'{key}','invalid'));
});
test('filename preview appends before extension and honors download settings', () => {
  assert.equal(previewFilename(DEFAULT_SETTINGS),'Nils Frahm - Says [128 BPM Am].mp3');
  assert.equal(previewFilename({...DEFAULT_SETTINGS,keyNotation:'camelot',format:'flac'}),'Nils Frahm - Says [128 BPM 8A].flac');
  assert.equal(previewFilename({...DEFAULT_SETTINGS,analyze:false}),'Nils Frahm - Says.mp3');
  assert.equal(previewFilename({...DEFAULT_SETTINGS,appendBpmKey:false}),'Nils Frahm - Says.mp3');
  const o = toDownloadOptions(DEFAULT_SETTINGS);
  for (const key of ['analyze','appendBpmKey','keyNotation','bpmKeySuffix','writeBpmKeyTags','bpmRange']) assert.deepEqual(o[key],DEFAULT_SETTINGS[key]);
});
