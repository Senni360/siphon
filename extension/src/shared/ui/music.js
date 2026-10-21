import { formatBpm, formatKey, keyColor, confidenceHint, alternateBpm } from '../music.js';

export function analysisChips(analysis, notation = 'standard', detailed = false) {
  const row = document.createElement('div'); row.className = 'analysis-chips';
  if (!analysis) return row;
  const chip = (text, className) => { const node = document.createElement('span'); node.className = `music-chip ${className}`; node.textContent = text; row.append(node); return node; };
  const bpm = formatBpm(analysis.bpm);
  if (bpm) {
    const node = chip(`${analysis.bpmConfidence < .6 ? '~' : ''}${bpm}`, analysis.bpmConfidence < .6 ? 'low-confidence' : '');
    const unit = document.createElement('span'); unit.className = 'unit'; unit.textContent = ' BPM'; node.append(unit);
  }
  const key = formatKey(analysis,notation);
  if (key) {
    const label = [key, notation === 'camelot' ? analysis.keyShort : analysis.camelot].filter(Boolean).join(' · ');
    const node = chip(`${analysis.keyConfidence < .6 ? '~' : ''}${label}`, 'key-chip');
    // Camelot hue stays functional (harmonic neighbours share it) but small: a dot, not a fill.
    const dot = document.createElement('span'); dot.className = 'key-dot'; dot.setAttribute('aria-hidden', 'true');
    const color = keyColor(analysis.camelot); if (color) dot.style.background = color;
    node.prepend(dot);
    node.classList.toggle('low-confidence',analysis.keyConfidence < .6);
  }
  row.title = confidenceHint(analysis);
  if (detailed) {
    const alternates = alternateBpm(analysis);
    if (alternates.length) chip(`or ${alternates.join(' / ')}`, 'alternate-bpm');
    const hint = document.createElement('span'); hint.className = 'analysis-hint'; hint.textContent = confidenceHint(analysis); row.append(hint);
  }
  return row;
}
