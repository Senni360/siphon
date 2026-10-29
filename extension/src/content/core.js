/* Classic script: intentionally no imports, page-world bridge, or remote code. */
(() => {
  'use strict';
  if (globalThis.SiphonContent) return;
  // Siphon's mark (two nested U strokes), and the done/chevron glyphs.
  const GLYPH = 'M6 4v8a6 6 0 0 0 12 0V4M10 4v8a2 2 0 0 0 4 0V4';
  const DONE = 'm5 12.5 4.5 4.5L19 7';
  const CHEVRON = 'm7 10 5 5 5-5';
  const ANALYZE = 'M4 10v4m4-7v10m4-13v16m4-13v10m4-7v4';
  // Host-level layout is !important: page rules otherwise beat :host and zero our spacing.
  // data-slot tunes the silhouette to the toolbar the control sits in.
  const CSS = `
    :host{--acc:#f4f4f5;--glass:rgb(255 255 255/.1);--glass-hover:rgb(255 255 255/.2);--line:rgb(255 255 255/.12);--spec:rgb(255 255 255/.1);--fg:#f4f4f5;--fg2:#a1a1aa;--pop:rgb(28 28 32/.82);--h:36px;--r:18px;display:inline-flex!important;vertical-align:middle;flex-shrink:0!important;align-self:center!important;margin:0 0 0 8px!important;position:relative!important;font-family:"Siphon Geist",system-ui,sans-serif;color-scheme:dark;letter-spacing:-.005em}
    :host([data-light=true]){--glass:rgb(0 0 0/.05);--glass-hover:rgb(0 0 0/.1);--line:rgb(0 0 0/.07);--spec:rgb(255 255 255/.8);--fg:#0f0f0f;--fg2:#5c5c66;--pop:rgb(255 255 255/.86);color-scheme:light}
    :host([data-site=youtube]),:host([data-site=youtubeMusic]){--acc:#ff0033}
    :host([data-slot=yt]){--h:40px;--r:20px}
    :host([data-slot=ytm]){margin:0!important}
    :host([data-slot=ytm-header]){display:flex!important;width:max-content;margin:12px auto 0!important}
    :host([data-site=soundcloud]){--acc:#ff5500;--h:32px;--r:4px}
    :host([data-slot=sc]),:host([data-slot=sc-row]){margin:0!important;vertical-align:top}
    :host([data-slot=sc-wrap]){display:flex!important;width:max-content;margin:8px 0 0!important}
    :host([data-slot=sc-bar]){--h:28px}
    :host([data-site=bandcamp]){--acc:#1da0c3;--h:26px;--r:4px}
    :host([data-slot=bc]){display:flex!important;width:max-content;margin:10px 0 0!important}
    *{box-sizing:border-box}
    .group{position:relative;display:inline-flex;align-items:stretch;height:var(--h);border-radius:var(--r);background:var(--native,var(--glass));color:var(--fg);box-shadow:inset 0 0 0 1px var(--line),inset 0 1px 0 var(--spec);backdrop-filter:blur(16px) saturate(1.4);overflow:hidden;transition:background .14s}
    :host([data-site=soundcloud]) .group{box-shadow:none;backdrop-filter:none}
    button{font:520 14px/1 "Siphon Geist",system-ui,sans-serif;letter-spacing:inherit;color:inherit;background:none;border:0;cursor:pointer;height:100%;padding:0 12px 0 10px;display:inline-flex;align-items:center;justify-content:center;gap:7px;white-space:nowrap;transition:background .14s}
    :host([data-site=soundcloud]) button{font-weight:600;padding:0 12px;gap:8px}
    :host([data-site=bandcamp]) button{font-size:12px;padding:0 9px 0 7px;gap:5px}
    button:hover:not(:disabled){background:var(--glass-hover)}button:focus-visible{outline:2px solid var(--acc);outline-offset:-2px}button:disabled{cursor:default}
    .main svg{width:18px;height:18px;stroke:var(--acc);fill:none;stroke-width:2;stroke-linecap:round;stroke-linejoin:round;flex-shrink:0;transition:transform .3s cubic-bezier(.34,1.4,.5,1)}
    :host([data-site=soundcloud]) .main svg{width:16px;height:16px}
    :host([data-site=bandcamp]) .main svg{width:15px;height:15px}
    .label:empty{display:none}
    :host([data-slot=sc-row]) .analysis-summary,:host([data-slot=sc-wrap]) .analysis-summary,:host([data-slot=sc-bar]) :is(.more,.analyze,.analysis-summary){display:none}
    .main:active:not(:disabled) svg{transform:translateY(1px) scale(.94)}
    button.more{width:var(--h);padding:0;flex-shrink:0;border-left:1px solid var(--line)}.more svg{width:14px;height:14px;stroke:var(--fg2);fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
    button.analyze{width:var(--h);padding:0;flex-shrink:0;border-left:1px solid var(--line)}
    :host .group button.more,:host .group button.analyze{padding:0}
    .analyze svg{width:16px;height:16px;stroke:var(--fg2);fill:none;stroke-width:1.8;stroke-linecap:round}
    .analyze[aria-expanded=true] svg,.analyze[data-ready=true] svg{stroke:var(--acc)}
    .analyze[aria-busy=true] svg{animation:listening 1s ease-in-out infinite alternate}
    .analyze:disabled{opacity:.4}
    .analysis-summary{display:inline-flex;align-items:center;padding:0 12px 0 6px;gap:8px;font:520 13px/1 "Siphon Geist Mono",ui-monospace,monospace;white-space:nowrap;font-variant-numeric:tabular-nums}
    .analysis-summary[hidden]{display:none}.analysis-summary[data-state=pending],.analysis-summary[data-state=error]{color:var(--fg2);font-family:"Siphon Geist",system-ui,sans-serif}
    .summary-key{padding-left:8px;border-left:1px solid var(--line)}
    .summary-key:empty{display:none}
    @keyframes listening{to{opacity:.35;transform:scaleY(.65)}}
    .label{max-width:180px;overflow:hidden;text-overflow:ellipsis}
    :host(:not([data-state=idle])) .label{font-family:"Siphon Geist Mono",ui-monospace,monospace;font-variant-numeric:tabular-nums;font-size:.92em;letter-spacing:-.02em}
    .progress{position:absolute;left:0;bottom:0;height:2px;width:100%;background:var(--acc);transform-origin:left;transform:scaleX(var(--p,0));opacity:0;transition:transform .4s cubic-bezier(.16,1,.3,1),opacity .3s}
    :host([data-state=downloading]) .progress,:host([data-state=converting]) .progress,:host([data-state=tagging]) .progress,:host([data-state=analyzing]) .progress,:host([data-state=probing]) .progress,:host([data-state=queued]) .progress{opacity:1}
    :host([data-state=error]) .label,:host([data-state=cancelled]) .label{color:var(--fg2)}
    .menu{position:fixed;z-index:2147483647;display:grid;min-width:180px;padding:5px;border-radius:14px;background:var(--pop);color:var(--fg);box-shadow:inset 0 0 0 1px var(--line),inset 0 1px 0 var(--spec),0 18px 40px -16px rgb(0 0 0/.55),0 2px 6px rgb(0 0 0/.18);backdrop-filter:blur(28px) saturate(1.6);transform-origin:top left;animation:pop .22s cubic-bezier(.16,1,.3,1)}
    @keyframes pop{from{opacity:0;transform:scale(.96) translateY(-4px)}}
    .menu[hidden]{display:none}.menu button{justify-content:flex-start;border-radius:9px;height:34px;padding:0 11px;font-size:13px;font-weight:480}
    .menu button:hover,.menu button:focus-visible{background:var(--glass-hover);outline:none}
    .menu .ext{margin-left:auto;padding-left:16px;font-size:12px;font-weight:400;color:var(--fg2)}
    .analysis-panel{max-width:min(280px,calc(100vw - 16px));min-width:210px;padding:14px;gap:10px;font:13px/1.45 "Siphon Geist",system-ui,sans-serif;white-space:normal}
    .analysis-heading{font-size:11px;color:var(--fg2);font-weight:520}
    .analysis-result{font-family:"Siphon Geist Mono",ui-monospace,monospace;font-size:16px;font-variant-numeric:tabular-nums}
    .analysis-note{font-size:11px;color:var(--fg2)}
    :host([data-compact=true]){display:flex!important;margin:0 auto 16px!important;--h:auto;--r:24px}:host([data-compact=true]) .group{flex-direction:column;width:48px}
    :host([data-compact=true]) .main{padding:12px 0 8px;flex-direction:column;gap:4px}
    :host([data-compact=true]) .label{display:none}:host([data-compact=true]) button.more{width:48px;height:48px;border-left:0;border-top:1px solid var(--line)}
    :host([data-compact=true]) button.analyze{width:48px;height:48px;border-left:0;border-top:1px solid var(--line)}
    :host([data-compact=true]) .analysis-summary{padding:0 3px 12px;flex-direction:column;gap:5px;white-space:normal;text-align:center;font-size:10px;line-height:1.2;overflow-wrap:anywhere}
    :host([data-compact=true]) .summary-key{padding:0;border:0}
    :host([data-compact=true]:not([data-state=idle])) .label{display:block;max-width:42px;font-size:10px}
    @media (prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important}}
  `;
  // Fonts for shadow roots must be registered on the document; scoped names avoid page collisions.
  function loadFonts() {
    if (globalThis.__siphonFonts || !document.fonts) return;
    globalThis.__siphonFonts = true;
    for (const [family, file] of [['Siphon Geist', 'Geist-Variable.woff2'], ['Siphon Geist Mono', 'GeistMono-Variable.woff2']]) {
      try { const face = new FontFace(family, `url(${chrome.runtime.getURL(`fonts/${file}`)})`, { weight: '100 900' }); document.fonts.add(face); void face.load().catch(() => {}); } catch { /* system font fallback */ }
    }
  }
  const svgIcon = d => { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true'); const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', d); svg.append(path); return svg; };
  // Match the page, not the OS: read the rendered background behind the content.
  function pageIsDark() {
    for (const node of [document.body, document.documentElement]) {
      const match = node && getComputedStyle(node).backgroundColor.match(/[\d.]+/g);
      if (match && (match[3] === undefined || Number(match[3]) > .5)) return (.2126 * match[0] + .7152 * match[1] + .0722 * match[2]) / 255 < .45;
    }
    return false;
  }
  const element = (tag, text) => { const e = document.createElement(tag); if (text) e.textContent = text; return e; };
  const text = (root, selector) => root?.querySelector(selector)?.textContent?.trim() || '';
  const href = (root, selector) => root?.querySelector(selector)?.getAttribute('href') || '';
  function start(adapter) {
    if (globalThis.__siphonContentStarted) return;
    globalThis.__siphonContentStarted = true;
    loadFonts();
    const records = new Map(), jobs = new Map();
    let enabled = false, timer = null, lastURL = location.href, openMenu = null, openAnalysis = null;
    function closeAnalysis(focus = false) {
      if (!openAnalysis) return;
      const r = openAnalysis; r.analysisPanel.hidden = true; r.analyze.setAttribute('aria-expanded', 'false'); r.host.style.zIndex = '';
      openAnalysis = null; if (focus) r.analyze.focus();
    }
    function positionPanel(panel, anchor) {
      const rect = anchor.getBoundingClientRect();
      panel.style.left = `${Math.max(8, Math.min(rect.left, innerWidth - panel.offsetWidth - 8))}px`;
      panel.style.top = `${Math.max(8, rect.bottom + panel.offsetHeight + 8 > innerHeight ? rect.top - panel.offsetHeight - 6 : rect.bottom + 6)}px`;
    }
    function renderAnalysis(r) {
      r.analyze.disabled = Boolean(r.descriptor.playlist);
      r.analyze.title = r.descriptor.playlist ? 'Open a track to analyze BPM and key' : 'Analyze BPM and key';
      r.analyze.setAttribute('aria-busy', String(Boolean(r.analysisPending)));
      r.analyze.dataset.ready = String(Boolean(r.analysis));
      const a = r.analysis;
      const bpm = a && Number.isFinite(a.bpm) ? `${Number(a.bpm.toFixed(1))} BPM` : 'Tempo unavailable';
      const key = a?.key || a?.keyShort || 'Key unavailable';
      r.analysisSummary.hidden = !r.analysisPending && !r.analysisError && !a;
      r.analysisSummary.dataset.state = r.analysisPending ? 'pending' : r.analysisError ? 'error' : 'ready';
      r.summaryTempo.textContent = r.analysisPending ? 'Analyzing…' : r.analysisError ? 'Retry analysis' : a ? bpm : '';
      r.summaryKey.textContent = !r.analysisPending && !r.analysisError && a ? (r.descriptor.compact ? a.keyShort || key : key) : '';
      r.analysisSummary.title = r.analysisError || (a ? `${bpm} · ${key}` : 'Finding BPM and key');
      r.analyze.setAttribute('aria-label', a ? `BPM and key: ${bpm}, ${key}. Show details` : 'Analyze BPM and key');
      r.analysisResult.textContent = r.analysisPending ? 'Analyzing…' : r.analysisError || (a ? `${bpm} · ${a.key || a.keyShort || 'Key unavailable'}` : '');
      r.analysisResult.className = a ? 'analysis-result' : '';
      r.analysisNote.textContent = r.analysisPending ? 'Finding tempo and key. No song is saved.' : r.analysisError ? 'Click the analyzer to retry.' : a && (a.bpmConfidence < .5 || a.keyConfidence < .5) ? 'Low confidence — treat this as an estimate.' : 'Estimated tempo and key';
      if (openAnalysis === r) positionPanel(r.analysisPanel, r.analyze);
    }
    function resetAnalysis(r) {
      r.analysisToken = (r.analysisToken || 0) + 1;
      r.analysis = null; r.analysisError = ''; r.analysisPending = false;
      if (openAnalysis === r) closeAnalysis();
      renderAnalysis(r);
    }
    async function analyze(r) {
      const media = r.descriptor.resolve();
      if (r.descriptor.playlist || !media?.url) return;
      if (r.media?.url !== media.url) { r.media = media; r.jobId = null; render(r); resetAnalysis(r); }
      if (openAnalysis === r && !r.analysisError) { closeAnalysis(); return; }
      closeMenu(); closeAnalysis(); openAnalysis = r; r.analysisPanel.hidden = false;
      r.host.style.zIndex = '2147483647'; r.analyze.setAttribute('aria-expanded', 'true');
      if (r.analysisPending || r.analysis) { renderAnalysis(r); return; }
      const token = r.analysisToken = (r.analysisToken || 0) + 1;
      r.analysisPending = true; r.analysisError = ''; renderAnalysis(r);
      try {
        const result = await chrome.runtime.sendMessage({ cmd: 'analyze', url: media.url });
        if (token !== r.analysisToken || !r.host.isConnected || r.descriptor.resolve()?.url !== media.url) return;
        if (result?.error || !result?.analysis) throw new Error(result?.error?.message || 'No tempo or key found.');
        r.analysis = result.analysis;
      } catch (error) {
        if (token !== r.analysisToken || !r.host.isConnected || r.descriptor.resolve()?.url !== media.url) return;
        r.analysisError = error.message || 'Analysis failed.';
      } finally {
        if (token === r.analysisToken) { r.analysisPending = false; renderAnalysis(r); }
      }
    }
    function closeMenu(focus = false) {
      if (!openMenu) return;
      const r = openMenu; r.menu.hidden = true; r.more.setAttribute('aria-expanded', 'false'); r.host.style.zIndex = '';
      openMenu = null; if (focus) r.more.focus();
    }
    function render(r, job) {
      r.job = job;
      const state = job?.state;
      const active = ['queued', 'probing', 'downloading', 'converting', 'tagging', 'analyzing'].includes(state);
      const progress = Number.isFinite(job?.progress) ? `${Math.round(Math.max(0, Math.min(1, job.progress)) * 100)}%` : 'Downloading…';
      const labels = { queued: 'Queued', probing: 'Preparing', downloading: progress, converting: 'Converting', tagging: 'Tagging', analyzing: 'Analyzing', done: 'Saved', error: 'Retry', cancelled: 'Retry' };
      const status = labels[state] || r.descriptor.label || 'Siphon';
      r.host.dataset.state = state || 'idle';
      // Icon-only controls show text only for progress and retry; the glyph and bar carry the rest.
      const brief = r.descriptor.icon ? { idle: '', queued: '', probing: '', converting: '', tagging: '', analyzing: '', done: '' } : r.descriptor.compact ? { probing: '…', converting: '…', tagging: '…', analyzing: '…', done: '' } : {};
      r.label.textContent = brief[state || 'idle'] ?? status;
      r.glyph.setAttribute('d', state === 'done' ? DONE : GLYPH);
      const fraction = state === 'downloading' && Number.isFinite(job?.progress) ? job.progress * .85 : { queued: .04, probing: .08, converting: .9, tagging: .94, analyzing: .97 }[state] ?? 0;
      r.group.style.setProperty('--p', String(Math.max(0, Math.min(1, fraction))));
      r.main.disabled = active;
      r.main.title = job?.error?.message ? `${job.error.message} — click to retry` : `${r.media?.title || 'Download audio'} · Alt/Shift-click for format`;
      if (state === 'done' && job.analysis) {
        const bpm = job.analysis.bpm;
        const tempo = Number.isFinite(bpm) ? (Math.abs(bpm - Math.round(bpm)) < .05 ? Math.round(bpm) : bpm.toFixed(1)) : '';
        r.main.title = ['Saved', tempo && `${tempo} BPM`, job.analysis.keyShort].filter(Boolean).join(' · ');
        r.analysis = job.analysis; r.analysisError = ''; renderAnalysis(r);
      }
      r.main.setAttribute('aria-label', `${status} — ${r.media?.title || 'Download audio'}`);
    }
    async function download(r, format) {
      closeMenu(); closeAnalysis();
      if (r.main.disabled) return;
      const media = r.descriptor.resolve();
      if (!media?.url) { render(r, { state: 'error', error: { message: 'No media is available yet' } }); return; }
      r.media = media; r.jobId = null; render(r, { state: 'queued' });
      try {
        const options = { ...(r.descriptor.playlist ? { playlist: 'all' } : {}), ...(format ? { format } : {}) };
        const result = await chrome.runtime.sendMessage({ cmd: 'download', url: media.url, title: media.title || document.title, source: adapter.site, ...(Object.keys(options).length ? { options } : {}) });
        if (result?.error || !result?.jobId) throw new Error(result?.error?.message || 'Siphon could not start the download');
        r.jobId = result.jobId;
        render(r, jobs.get(r.jobId) || { state: 'queued' });
      } catch (error) { render(r, { state: 'error', error: { message: error.message } }); }
    }
    function showMenu(r) {
      closeAnalysis();
      if (openMenu === r) { closeMenu(); return; }
      closeMenu(); openMenu = r; r.menu.hidden = false; r.host.style.zIndex = '2147483647'; r.more.setAttribute('aria-expanded', 'true');
      const rect = r.main.getBoundingClientRect(), height = r.menu.offsetHeight;
      r.menu.style.left = `${Math.max(8, Math.min(rect.left, innerWidth - r.menu.offsetWidth - 8))}px`;
      r.menu.style.top = `${Math.max(8, rect.bottom + height + 8 > innerHeight ? rect.top - height - 6 : rect.bottom + 6)}px`;
      r.menu.querySelector('button').focus();
    }
    function create(descriptor, media) {
      const host = element('span'); host.dataset.siphon = descriptor.key; host.dataset.site = adapter.site; host.dataset.compact = String(Boolean(descriptor.compact));
      if (descriptor.slot) host.dataset.slot = descriptor.slot;
      const shadow = host.attachShadow({ mode: 'open' }), style = element('style', CSS), group = element('span'); group.className = 'group';
      const main = element('button'), more = element('button'), analyzer = element('button'), label = element('span'), progress = element('span');
      analyzer.type = 'button'; analyzer.className = 'analyze'; analyzer.append(svgIcon(ANALYZE));
      analyzer.setAttribute('aria-label', 'Analyze BPM and key'); analyzer.setAttribute('aria-expanded', 'false'); analyzer.setAttribute('aria-controls', 'analysis-panel');
      progress.className = 'progress'; more.append(svgIcon(CHEVRON));
      main.type = more.type = 'button'; main.className = 'main'; more.className = 'more'; label.className = 'label';
      const svg = svgIcon(GLYPH); main.append(svg, label);
      more.setAttribute('aria-label', 'Choose audio format'); more.setAttribute('aria-haspopup', 'menu'); more.setAttribute('aria-expanded', 'false');
      const menu = element('div'); menu.className = 'menu'; menu.hidden = true; menu.setAttribute('role', 'menu'); menu.setAttribute('aria-label', 'Download audio as');
      const analysisPanel = element('div'), analysisHeading = element('div', 'BPM & KEY'), analysisResult = element('div'), analysisNote = element('div');
      analysisPanel.className = 'menu analysis-panel'; analysisPanel.id = 'analysis-panel'; analysisPanel.hidden = true;
      analysisPanel.setAttribute('role', 'status'); analysisPanel.setAttribute('aria-atomic', 'true');
      analysisHeading.className = 'analysis-heading'; analysisNote.className = 'analysis-note';
      analysisPanel.append(analysisHeading, analysisResult, analysisNote);
      const analysisSummary = element('span'), summaryTempo = element('span'), summaryKey = element('span');
      analysisSummary.className = 'analysis-summary'; analysisSummary.hidden = true; analysisSummary.setAttribute('aria-hidden', 'true');
      summaryKey.className = 'summary-key'; analysisSummary.append(summaryTempo, summaryKey);
      const r = { host, group, main, more, analyze: analyzer, analysisPanel, analysisResult, analysisNote, analysisSummary, summaryTempo, summaryKey, label, menu, descriptor, media, jobId: null, glyph: svg.firstChild };
      for (const [name, value] of [['MP3', 'mp3'], ['M4A', 'm4a'], ['FLAC', 'flac'], ['Opus', 'opus'], ['WAV', 'wav'], ['Original', 'best']]) {
        const choice = element('button', name); choice.type = 'button'; choice.setAttribute('role', 'menuitem');
        const hint = element('span', { mp3: 'Small', m4a: 'AAC', flac: 'Lossless', opus: 'Smallest', wav: 'For your DAW', best: 'As uploaded' }[value]); hint.className = 'ext'; choice.append(hint);
        choice.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); download(r, value); }); menu.append(choice);
      }
      menu.addEventListener('keydown', e => {
        const choices = [...menu.children], i = choices.indexOf(shadow.activeElement);
        if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) {
          e.preventDefault(); choices[e.key === 'Home' ? 0 : e.key === 'End' ? choices.length - 1 : (i + (e.key === 'ArrowDown' ? 1 : -1) + choices.length) % choices.length].focus();
        }
        if (e.key === 'Escape') { e.preventDefault(); closeMenu(true); }
        if (e.key === 'Tab') closeMenu();
      });
      main.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); if (e.altKey || e.shiftKey) showMenu(r); else download(r); });
      more.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); showMenu(r); });
      analyzer.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); void analyze(r); });
      shadow.addEventListener('keydown', e => { if (e.key === 'Escape' && openAnalysis === r) { e.preventDefault(); e.stopPropagation(); closeAnalysis(true); } });
      group.append(main, more, analyzer, analysisSummary, progress); shadow.append(style, group, menu, analysisPanel); place(r); render(r); renderAnalysis(r); return r;
    }
    // Optional `before` keeps a native anchor (e.g. the Shorts sound pivot) at the end of its column.
    function place(r) {
      const { mount, before } = r.descriptor;
      if (before?.parentNode === mount) { if (r.host.nextSibling !== before) mount.insertBefore(r.host, before); }
      else if (r.host.parentNode !== mount) mount.append(r.host);
    }
    // Borrow a neutral native button's fill so the control reads as part of the host toolbar.
    function tone(r) {
      const sample = r.descriptor.sample;
      if (!sample?.isConnected || sample.matches(':hover, :active')) return;
      const fill = getComputedStyle(sample).backgroundColor, alpha = fill.match(/[\d.]+/g)?.[3];
      if (alpha === undefined || Number(alpha) > 0) r.host.style.setProperty('--native', fill); else r.host.style.removeProperty('--native');
    }
    function remove(r) { if (openMenu === r) closeMenu(); if (openAnalysis === r) closeAnalysis(); r.analysisToken = (r.analysisToken || 0) + 1; r.host.remove(); records.delete(r.host); }
    function scan() {
      timer = null;
      if (lastURL !== location.href) { lastURL = location.href; for (const r of [...records.values()]) remove(r); }
      for (const r of [...records.values()]) if (!r.host.isConnected || !enabled) remove(r);
      if (!enabled) return;
      const descriptors = adapter.collect();
      const desired = new Set();
      for (const d of descriptors) {
        if (!d.mount) continue;
        const media = d.resolve(); if (!media?.url) continue;
        let r = [...records.values()].find(r => r.host.parentElement === d.mount && r.descriptor.key === d.key);
        if (!r) { r = create(d, media); records.set(r.host, r); }
        else { r.descriptor = d; place(r); if (r.media?.url !== media.url) { r.media = media; r.jobId = null; render(r); resetAnalysis(r); } else r.media = media; renderAnalysis(r); }
        r.host.dataset.light = String(!(adapter.dark?.() ?? pageIsDark())); tone(r); desired.add(r.host);
      }
      for (const r of [...records.values()]) if (!desired.has(r.host)) remove(r);
    }
    function schedule() { if (timer === null) timer = setTimeout(scan, 180); }
    const observer = new MutationObserver(mutations => {
      // A site's renderer can remove our control without replacing its toolbar.
      // Internal removals have already left records by the time this callback runs.
      const removedControl = mutations.some(m => [...m.removedNodes].some(n => records.has(n)));
      if (removedControl || mutations.some(m => !m.target.closest?.('[data-siphon]') && (m.type === 'attributes' || [...m.addedNodes, ...m.removedNodes].some(n => n.nodeType === 1 && !n.matches?.('[data-siphon]'))))) schedule();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['href', 'src', 'video-id', 'hidden', 'is-active', 'dark'] });
    for (const event of ['yt-navigate-finish', 'yt-page-data-updated', 'ytmusic-player-page-changed', 'popstate', 'hashchange']) window.addEventListener(event, schedule);
    document.addEventListener('pointerdown', e => { if (openMenu && !e.composedPath().includes(openMenu.host)) closeMenu(); if (openAnalysis && !e.composedPath().includes(openAnalysis.host)) closeAnalysis(); }, true);
    // Responsive hosts re-lay out on resize without DOM mutations; rescan so slots can adapt.
    window.addEventListener('resize', () => { closeMenu(); closeAnalysis(); schedule(); }); window.addEventListener('scroll', () => { closeMenu(); closeAnalysis(); }, { passive: true });
    chrome.runtime.onMessage.addListener((message, sender, respond) => {
      if (message?.q === 'siphon:getMedia') { respond(adapter.current() || null); return false; }
      if (message?.evt === 'jobs' && Array.isArray(message.jobs)) {
        for (const job of message.jobs) if (job?.jobId) { jobs.delete(job.jobId); jobs.set(job.jobId, job); }
        while (jobs.size > 100) jobs.delete(jobs.keys().next().value);
        for (const r of records.values()) if (r.jobId && jobs.has(r.jobId)) render(r, jobs.get(r.jobId));
      }
      return false;
    });
    const applySettings = settings => { enabled = settings?.siteButtons?.[adapter.site] !== false; schedule(); };
    chrome.storage.onChanged.addListener((changes, area) => { if (area === 'sync' && changes.settings) applySettings(changes.settings.newValue); });
    // Wait for settings before first insertion, so disabled integrations never flash.
    chrome.storage.sync.get('settings').then(({ settings }) => { applySettings(settings); }, () => { enabled = true; schedule(); });
  }
  globalThis.SiphonContent = Object.freeze({ start, text, href });
})();
