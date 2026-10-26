// Local-only harness. ?theme=light|dark&host=ready|missing|error&empty=1
import { SAMPLE_ANALYSIS } from '../../shared/music.js';
import { mergeSettings } from '../../shared/settings.js';
const params = new URLSearchParams(location.search);
const listeners = new Set(), messages = [];
const now = Date.UTC(2026,9,16,20);
const titles = ['Says — live at the studio', 'An evening in the city', 'Tape sketches, volume II', 'A room with a view', 'Low tide / full session'];
const urls = ['https://www.youtube.com/watch?v=sample', 'https://soundcloud.com/demo/evening', 'https://artist.bandcamp.com/album/tape-sketches', 'https://www.youtube.com/watch?v=sample2', 'https://soundcloud.com/demo/low-tide'];
const state = {
  settings: mergeSettings({theme:params.get('theme') || 'dark'}), jobs: params.get('state') === 'analyzing' ? [{jobId:'analyzing',state:'analyzing',stage:'analyzing',title:titles[0]}] : [],
  host: {state:params.get('host') || 'ready',dropzone:true,hostVersion:'1.0.0',ytdlpVersion:'2026.10.14',ffmpeg:{found:true,version:'8.1',path:'C:\\Tools\\ffmpeg\\bin\\ffmpeg.exe'},jsRuntime:{name:'node',version:'24.0.0'},defaultOutputDir:'C:\\Users\\senni\\Music\\Siphon',platform:'win32'},
  history: params.has('empty') ? [] : titles.map((title,i) => ({analysis:i < 3 ? {...SAMPLE_ANALYSIS,bpm:[128,96,174][i]} : null,id:`sample-${i}`,title,url:urls[i],thumbnail:i < 3 ? new URL('../../../icons/icon.svg',import.meta.url).href : null,format:['mp3','opus','flac','m4a','best'][i],state:i === 3 ? 'error' : i === 4 ? 'cancelled' : 'done',file:i < 3 ? `C:\\Users\\senni\\Music\\Siphon\\${title}.mp3` : null,createdAt:now-i*86400000,finishedAt:now-i*86400000+60000,totalBytes:i < 3 ? (i+1)*12340000 : null,error:i === 3 ? {message:'The source requires you to sign in. Enable site cookies and retry.'} : null})),
};
if (state.host.state !== 'ready') state.host = {state:state.host.state,error:{message:state.host.state === 'missing' ? 'Native messaging host not found.' : 'The native host connection was interrupted.'}};
const emit = event => listeners.forEach(listener => listener(structuredClone(event)));
window.__preview = {state,messages,emit,failNext:null};
window.chrome = {
  runtime: {
    id:'bieofegakabfdjplcnpiaegdifmempmf', getManifest:() => ({version:'1.0.0'}),
    onMessage:{addListener:listener => listeners.add(listener),removeListener:listener => listeners.delete(listener)},
    async sendMessage(message) {
      messages.push(structuredClone(message));
      await new Promise(resolve => setTimeout(resolve,80));
      if (window.__preview.failNext) { const error = window.__preview.failNext; window.__preview.failNext = null; return {error:{message:error}}; }
      switch (message.cmd) {
        case 'getState': return structuredClone(state);
        case 'setSettings': state.settings = mergeSettings({...state.settings,...message.patch}); emit({evt:'settings',settings:state.settings}); return structuredClone(state.settings);
        case 'analyze': case 'analyzeFile':
          if (params.get('state') === 'analyzing') return new Promise(() => {});
          if (params.has('cancel')) return null;
          return {analysis:SAMPLE_ANALYSIS,title:'Studio session',...(message.cmd === 'analyzeFile' ? {path:'C:\\Music\\Studio session.wav'} : {})};
        case 'pickFolder': return {path:'D:\\Music\\My collection'};
        case 'updateHost': { const before = state.host.ytdlpVersion; state.host.ytdlpVersion = '2026.10.17'; return {before,after:state.host.ytdlpVersion}; }
        case 'reconnectHost': emit({evt:'host',status:state.host}); return structuredClone(state.host);
        case 'clearHistory': state.history = message.ids ? state.history.filter(i => !message.ids.includes(i.id)) : []; emit({evt:'history',history:state.history}); return {};
        case 'retry': return {jobId:'preview-retry'};
        case 'open': case 'reveal': return {};
        default: return {};
      }
    },
  },
  commands:{getAll:async () => [{name:'download-current',shortcut:'Alt+Shift+D'}]},
  tabs:{create:async options => {messages.push({preview:'tabs.create',...options}); return {id:1,...options};}},
};
const html = await (await fetch(new URL('../options.html',import.meta.url))).text();
document.body.innerHTML = new DOMParser().parseFromString(html,'text/html').body.innerHTML;
// The preview's asset base must not send hash navigation to the options directory.
document.querySelectorAll('a[href^="#"]').forEach(link => { link.href = new URL(link.getAttribute('href'), location.href).href; });
await import('../options.js');
