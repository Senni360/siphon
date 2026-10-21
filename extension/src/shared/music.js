import { siteForUrl } from './urls.js';

/** Pure notation/filename helpers. Keep suffix rules aligned with host analysis/notation.py. */
const TONICS = { major: ['C','Db','D','Eb','E','F','Gb','G','Ab','A','Bb','B'], minor: ['C','C#','D','D#','E','F','F#','G','G#','A','Bb','B'] };
const WHEEL = { major: [8,3,10,5,12,7,2,9,4,11,6,1], minor: [5,12,7,2,9,4,11,6,1,8,3,10] };
export const SAMPLE_ANALYSIS = Object.freeze({ bpm:128, bpmConfidence:.91, bpmCandidates:[{bpm:128,score:.91},{bpm:64,score:.76}], key:'A minor', keyShort:'Am', tonic:'A', mode:'minor', camelot:'8A', openKey:'1m', keyConfidence:.86, durationAnalyzed:150, engine:'siphon-analysis/1' });

export function keyNames(value) {
  const match = /^([A-Ga-g])([#b]?)(?:\s*(major|minor|maj|min|m|M))?$/.exec(String(value).trim().replaceAll('♯','#').replaceAll('♭','b'));
  if (!match) throw new Error('Choose a valid musical key.');
  const mode = ['minor','min','m'].includes(match[3]) ? 'minor' : 'major';
  const pitch = ({C:0,D:2,E:4,F:5,G:7,A:9,B:11}[match[1].toUpperCase()] + ({'#':1,b:-1}[match[2]] || 0) + 12) % 12;
  const tonic = TONICS[mode][pitch], n = WHEEL[mode][pitch];
  return { tonic, mode, key:`${tonic} ${mode}`, keyShort:tonic + (mode === 'minor' ? 'm' : ''), camelot:`${n}${mode === 'minor' ? 'A' : 'B'}`, openKey:`${(n + 4) % 12 + 1}${mode === 'minor' ? 'm' : 'd'}` };
}
export function formatBpm(bpm) {
  if (typeof bpm !== 'number' || !Number.isFinite(bpm) || bpm <= 0) return '';
  if (Math.abs(bpm - Math.round(bpm)) < .05) return String(Math.round(bpm));
  // Python's .1f uses ties-to-even. Exact binary quarter ties ending in .25
  // round down; all other positive ties agree with JS toFixed(1).
  return (bpm % 1 === .25 ? bpm - .05 : bpm).toFixed(1);
}
/** Drag-tray card for a finished download: { files, title, subtitle }. */
export function dropItem(item, notation = 'standard') {
  const files = item.files?.length ? item.files : item.file ? [item.file] : [];
  const format = item.format === 'best' ? (files[0]?.split('.').pop() || 'audio').toUpperCase() : (item.format || 'audio').toUpperCase();
  const a = item.analysis;
  const subtitle = [a && formatBpm(a.bpm) && `${formatBpm(a.bpm)} BPM`, a && formatKey(a, notation), files.length > 1 ? `${files.length} files` : format].filter(Boolean).join(' · ');
  return { files, title: item.title || files[0]?.split(/[\\/]/).pop() || 'Audio', subtitle, service: siteForUrl(item.url || '') };
}
export function formatKey(analysis, notation = 'standard') {
  if (!analysis) return '';
  return analysis[{standard:'keyShort',camelot:'camelot',openkey:'openKey'}[notation] || 'keyShort'] || '';
}
// Camelot's twelve circle-of-fifths hues; relative major/minor share a color.
export function keyColor(camelot) {
  const match = /^(1[0-2]|[1-9])[AB]$/.exec(camelot || '');
  return match ? `hsl(${(Number(match[1]) * 30 + 240) % 360} 65% 78%)` : null;
}
export function confidenceHint(analysis) {
  if (!analysis) return '';
  const low = [];
  if (analysis.bpmConfidence < .6) low.push('tempo');
  if (analysis.keyConfidence < .6) low.push('key');
  return low.length ? `Estimated ${low.join(' & ')} · lower confidence` : 'Tempo & key estimate';
}
export function alternateBpm(analysis) {
  return (analysis?.bpmCandidates || []).filter(c => c.bpm > 0 && c.score >= (analysis.bpmCandidates[0]?.score || 1) * .65 && [0.5,2].some(r => Math.abs(c.bpm / analysis.bpm - r) < .03)).map(c => formatBpm(c.bpm)).filter((v,i,a) => a.indexOf(v) === i);
}
export function formatSuffix(analysis, template = ' [{bpm} BPM {key}]', notation = 'standard') {
  if (!['standard','camelot','openkey'].includes(notation)) throw new Error('Choose a valid key notation.');
  const bpm = formatBpm(analysis?.bpm);
  if (!bpm) throw new Error('BPM must be finite and positive.');
  const names = keyNames(analysis.keyShort || analysis.key);
  // {bpm} is a whole number (filenames read better); {bpmExact} keeps one decimal.
  const fields = {bpm:String(Math.floor(analysis.bpm + .5)), bpmExact:bpm, key:formatKey(names,notation), keyShort:names.keyShort,keyLong:names.key,camelot:names.camelot,openkey:names.openKey};
  let result = '';
  for (let i = 0; i < template.length;) {
    if (template.slice(i,i+2) === '{{') { result += '{'; i += 2; }
    else if (template.slice(i,i+2) === '}}') { result += '}'; i += 2; }
    else if (template[i] === '{') {
      const end = template.indexOf('}',i), field = template.slice(i+1,end).replace(/:$/,'');
      if (end < 0 || !Object.hasOwn(fields,field)) throw new Error('Use only the supported suffix placeholders.');
      result += fields[field]; i = end + 1;
    } else if (template[i] === '}') throw new Error('Unmatched brace in suffix template.');
    else result += template[i++];
  }
  return [...result.replace(/[<>:"/\\|?*\x00-\x1f\x7f]/g,'_')].slice(0,160).join('').replace(/[ .]+$/,'');
}
export function previewFilename(settings, analysis = SAMPLE_ANALYSIS) {
  const sample = {artist:'Nils Frahm',uploader:'Nils Frahm',title:'Says',ext:settings.format === 'best' ? 'webm' : settings.format,playlist_title:'Evening listening',playlist_index:'03'};
  const filename = settings.filenameTemplate.replace(/%\(([^)]+)\)(?:0?\d+)?[sd]/g, (_, keys) => sample[keys.split(',').find(k => k in sample)] ?? 'sample');
  if (!settings.analyze || !settings.appendBpmKey) return filename;
  const suffix = formatSuffix(analysis,settings.bpmKeySuffix,settings.keyNotation);
  const dot = filename.lastIndexOf('.');
  return dot > 0 ? filename.slice(0,dot) + suffix + filename.slice(dot) : filename + suffix;
}
