/* Development-only Chrome adapter. Loaded exclusively by preview.html. */
(() => {
  const params = new URLSearchParams(parent.location.search);
  const scenario = params.get('state') || 'media';
  const listeners = new Set();
  const settings = { theme: params.get('theme') || 'dark', format: 'mp3', quality: '320' };
  const host = { state: scenario === 'missing' ? 'missing' : scenario === 'connecting' ? 'connecting' : 'ready', hostVersion: '1.0.0', ytdlpVersion: '2026.10.14', ffmpeg: { found: true, version: '8.1' } };
  const info = { url: 'https://www.youtube.com/watch?v=preview', webpageUrl: 'https://www.youtube.com/watch?v=preview', title: 'A walk through the quiet city', uploader: 'Nora & the Satellites', duration: 264, extractor: 'YouTube', thumbnail: new URL('dev/cover.svg', document.baseURI).href, isPlaylist: scenario === 'playlist', entryCount: 24, isLive: false };
  const analysis = {bpm:128,bpmConfidence:.91,bpmCandidates:[{bpm:128,score:.91},{bpm:64,score:.76}],key:'A minor',keyShort:'Am',tonic:'A',mode:'minor',camelot:'8A',openKey:'1m',keyConfidence:scenario === 'uncertain' ? .4 : .86,durationAnalyzed:150,engine:'siphon-analysis/1'};
  const analyses = ['analysis','uncertain','analyzing'].includes(scenario) ? {[info.url]:{analysis,title:info.title}} : {};
  let jobs = scenario === 'active' || scenario === 'playlist' ? [{ jobId: 'demo-1', title: 'A walk through the quiet city', state: 'downloading', progress: .42, speed: 3100000, eta: 12, item: scenario === 'playlist' ? { index: 3, count: 12, title: 'A walk through the quiet city' } : null }, ...(scenario === 'active' ? [{ jobId: 'demo-2', title: 'Blue hour (live session)', state: 'converting', progress: null }] : [])] : [];
  if (scenario === 'analyzing') jobs = [{jobId:'demo-analysis',title:info.title,state:'analyzing',stage:'analyzing',progress:null}];
  let history = scenario === 'missing' || scenario === 'empty' || scenario === 'probing' ? [] : [{ id: 'h1', title: 'Weightless, at last', format: 'flac', state: 'done', file: 'C:\\Music\\Weightless.flac' }, { id: 'h2', title: 'The long way home', format: 'mp3', state: 'done', file: 'C:\\Music\\Home.mp3' }, { id: 'h3', title: 'Late night radio — Vol. 02', format: 'opus', state: 'done', file: 'C:\\Music\\Radio.opus' }];
  history = history.map(item => ({...item,analysis}));
  if (scenario === 'errors') history = [{ id: 'failed', title: 'The sound of somewhere else', format: 'mp3', state: 'error', error: { code: 'NETWORK' } }, ...history];
  const emit = event => listeners.forEach(listener => listener(event));
  const calls = [];
  const storage = Object.fromEntries(['sync', 'local', 'session'].map(area => [area, { async get(key) { const data = { settings, jobs, history, analyses }; return typeof key === 'string' ? { [key]: data[key] } : data; }, async set() {}, async remove() {} }]));
  Object.assign(window.chrome || (window.chrome = {}), { storage, tabs: { async query() { return [{id:1,url:info.url}]; }, async sendMessage() { return {url:info.url}; }, async create(args) { calls.push({ cmd: 'tabs.create', ...args }); } }, runtime: {
    getURL: path => new URL(`../../../${path.replace(/^src\//, 'src/')}`, document.baseURI).href,
    async openOptionsPage() { calls.push({ cmd: 'openOptionsPage' }); },
    onMessage: { addListener: listener => listeners.add(listener), removeListener: listener => listeners.delete(listener) },
    async sendMessage(request) {
      calls.push(request);
      const { cmd } = request;
      if (params.has('fail') && ['download', 'downloadTab', 'setSettings', 'open', 'reveal', 'retry'].includes(cmd)) return { error: { code: 'NETWORK' } };
      if (cmd === 'getState') return { host, settings, jobs, history };
      if (cmd === 'probeTab') {
        if (scenario === 'probing' || scenario === 'connecting') return new Promise(() => {});
        await new Promise(resolve => setTimeout(resolve, params.has('freeze') ? 0 : 650));
        return scenario === 'missing' ? { info: null, error: { code: 'HOST_NOT_FOUND' } } : scenario === 'empty' ? { info: null } : { url:info.url, info, analysis:analyses[info.url]?.analysis || null };
      }
      if (cmd === 'analyzeTab' || cmd === 'analyze') {
        if (scenario === 'listening') return new Promise(() => {});
        if (params.has('fail')) return {error:{code:'NETWORK'}};
        await new Promise(resolve => setTimeout(resolve,params.has('freeze') ? 0 : 1200));
        return {analysis,title:info.title};
      }
      if (cmd === 'setSettings') { Object.assign(settings, request.patch); emit({ evt: 'settings', settings }); return settings; }
      if (cmd === 'getSettings') return settings;
      if (cmd === 'download' || cmd === 'downloadTab' || cmd === 'retry') {
        const jobId = `demo-${Date.now()}`; jobs = [...jobs, { jobId, title: cmd === 'download' ? 'Audio from pasted link' : info.title, state: 'downloading', progress: .02, speed: 3100000, eta: 30 }]; emit({ evt: 'jobs', jobs }); return { jobId };
      }
      if (cmd === 'cancel') { jobs = jobs.filter(job => job.jobId !== request.jobId); emit({ evt: 'jobs', jobs }); return {}; }
      return {};
    },
  } });
  window.__preview = { calls, emit, settings, get jobs() { return jobs; }, history, info };
  window.addEventListener('error', event => parent.postMessage({ siphonError: event.message }, '*'));
  window.addEventListener('unhandledrejection', event => parent.postMessage({ siphonError: String(event.reason) }, '*'));
  if (!params.has('freeze')) setInterval(() => {
    jobs = jobs.flatMap(job => {
      if (job.state !== 'downloading') return [job];
      const progress = Math.min(1, job.progress + .015);
      if (progress < 1) return [{ ...job, progress, eta: Math.max(0, Math.ceil((1 - progress) * 25)) }];
      history = [{ id: job.jobId, title: job.title, state: 'done', format: settings.format, analysis, file: 'C:\\Music\\Preview.mp3' }, ...history]; emit({ evt: 'history', history }); return [];
    }); emit({ evt: 'jobs', jobs });
  }, 700);
})();
