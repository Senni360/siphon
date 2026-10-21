import test from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as wait } from 'node:timers/promises';
import { createChromeMock } from '../helpers/chrome-mock.js';
import { NativeManager } from '../../src/background/native.js';
import { AnalysisCache } from '../../src/background/analysis.js';
import { Jobs } from '../../src/background/jobs.js';
import { Notifications } from '../../src/background/notify.js';
import { createRouter } from '../../src/background/router.js';
import { DEFAULT_SETTINGS } from '../../src/shared/settings.js';
import { SAMPLE_ANALYSIS as analysis } from '../../src/shared/music.js';
import { CMD, JOB_STATE, TERMINAL_STATES } from '../../src/shared/protocol.js';

async function setup(extra = {}) {
  const chrome = createChromeMock({cookies:[{name:'auth',value:'secret',domain:'.youtube.com',path:'/',secure:true}], ...extra.chrome});
  const cache = new AnalysisCache(chrome); await cache.init();
  const getSettings = async () => ({...DEFAULT_SETTINGS,...extra.settings});
  const native = new NativeManager(chrome,{getSettings,...extra.native});
  const counts = [], terminals = [];
  const jobs = new Jobs(chrome,{native,getSettings,analysisCache:cache,onCount:n => counts.push(n),onTerminal:j => terminals.push(j)});
  const ready = jobs.init();
  const {route} = createRouter({chrome,native,jobs,ready,getSettings,analysisCache:cache,setSettings:async () => {}});
  chrome.nativeHandler = (request,port) => port.reply(request,request.type === 'hello' ? {hostVersion:'test'} : request.type === 'listJobs' ? {jobs:[]} : request.type === 'pickFile' ? {path:'C:\\Music\\track.wav'} : {analysis,source:request.path ? 'file' : 'url',title:'Track'});
  await ready;
  return {chrome,native,jobs,route,cache,counts,terminals};
}
test('analyze routes normalized URL with cookies, caches result and survives popup/worker reopen', async () => {
  const x = await setup();
  try {
    const result = await x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/abc#t=3'});
    assert.deepEqual(result.analysis,analysis);
    const request = x.chrome.calls.native.find(r => r.type === 'analyze');
    assert.equal(request.url,'https://www.youtube.com/watch?v=abc'); assert.match(request.cookies,/secret/);
    assert.deepEqual((await x.route({cmd:CMD.ANALYZE,url:'https://www.youtube.com/watch?v=abc&t=4'})).analysis,analysis);
    assert.equal(x.chrome.calls.native.filter(r => r.type === 'analyze').length,1);
    const reopened = new AnalysisCache(x.chrome); await reopened.init(); assert.deepEqual(reopened.get('https://youtu.be/abc').analysis,analysis);
    assert.doesNotMatch(JSON.stringify(x.chrome.storage.session.data.analyses),/secret|cookies/);
  } finally { x.native.disconnect(); }
});
test('analyze tab resolves the playing media and honors disabled cookie export', async () => {
  const x = await setup({settings:{sendSiteCookies:false}});
  x.chrome.tabMessageHandler = () => ({url:'https://youtu.be/playing',title:'Playing'});
  try {
    await x.route({cmd:CMD.ANALYZE_TAB,tabId:1});
    const request = x.chrome.calls.native.find(r => r.type === 'analyze');
    assert.equal(request.url,'https://www.youtube.com/watch?v=playing'); assert.equal('cookies' in request,false);
    await assert.rejects(x.route({cmd:CMD.ANALYZE,url:'file:///secret'}),{code:'UNSUPPORTED_URL'});
  } finally { x.native.disconnect(); }
});
test('file analysis picks a file, direct paths bypass picker, cancel returns null', async () => {
  const x = await setup();
  try {
    assert.equal((await x.route({cmd:CMD.ANALYZE_FILE})).path,'C:\\Music\\track.wav');
    assert.equal(x.chrome.calls.native.at(-1).path,'C:\\Music\\track.wav');
    assert.equal((await x.route({cmd:CMD.ANALYZE_FILE,path:'D:\\music.flac'})).analysis.bpm,128);
    assert.equal(x.chrome.calls.native.filter(r => r.type === 'pickFile').length,1);
    x.chrome.nativeHandler = (r,p) => p.reply(r,{path:null});
    assert.equal(await x.route({cmd:CMD.ANALYZE_FILE}),null);
    assert.equal(x.chrome.calls.native.at(-1).type,'pickFile');
    assert.deepEqual(x.chrome.storage.session.data.analyses || {},{});
  } finally { x.native.disconnect(); }
});
test('analysis gets a dedicated 150 second timeout and releases pending requests', async () => {
  const x = await setup({native:{analyzeTimeoutMs:35,timeoutMs:10}});
  assert.equal(new NativeManager(x.chrome).analyzeTimeoutMs,150000);
  await x.native.connect(); x.chrome.nativeHandler = null;
  const result = assert.rejects(x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/slow'}),{code:'NETWORK'});
  await wait(20); assert.equal(x.native.pending.size,1);
  await wait(30); await result;
  assert.equal(x.native.pending.size,0); assert.equal(x.cache.get('https://youtu.be/slow'),null); x.native.disconnect();
});
test('concurrent URL requests share analysis; failures remain retryable', async () => {
  const x = await setup();
  try {
    await Promise.all([x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/same'}),x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/same'})]);
    assert.equal(x.chrome.calls.native.filter(r => r.type === 'analyze').length,1);
    x.chrome.nativeHandler = (r,p) => p.reply(r,{}, {code:'NETWORK',message:'offline'});
    await assert.rejects(x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/retry'}),{code:'NETWORK'});
    x.chrome.nativeHandler = (r,p) => p.reply(r,{analysis});
    assert.deepEqual((await x.route({cmd:CMD.ANALYZE,url:'https://youtu.be/retry'})).analysis,analysis);
  } finally { x.native.disconnect(); }
});
test('analyzing stays active for queue/badge; done stores analysis and seeds cache', async () => {
  const x = await setup();
  try {
    assert.equal(TERMINAL_STATES.has(JOB_STATE.ANALYZING),false);
    const {jobId} = await x.jobs.download({url:'https://youtu.be/song'});
    await x.jobs.event({type:'job',jobId,state:'analyzing',stage:'analyzing'});
    assert.equal(x.jobs.list()[0].state,'analyzing'); assert.equal(x.counts.at(-1),1); assert.equal(x.jobs.history.length,0); assert.equal(x.terminals.length,0);
    const recovered = new Jobs(x.chrome,{native:x.native,getSettings:async () => DEFAULT_SETTINGS}); await recovered.init(); assert.equal(recovered.list()[0].state,'analyzing');
    await x.jobs.event({type:'job',jobId,state:'done',analysis,analyses:[{file:'a.mp3',analysis}],file:'a.mp3'});
    assert.equal(x.counts.at(-1),0); assert.deepEqual(x.jobs.history[0].analysis,analysis); assert.equal(x.jobs.history[0].analyses.length,1);
    assert.deepEqual(x.chrome.storage.local.data.history[0].analysis,analysis); assert.deepEqual(x.cache.get('https://youtu.be/song').analysis,analysis);
    const snapshot = await x.route({cmd:CMD.GET_STATE}); assert.deepEqual(snapshot.history[0].analysis,analysis);
    await x.jobs.event({jobId:'no-analysis',state:'done',analysis:null,analysisError:{code:'INTERNAL',message:'No pulse'}});
    assert.equal(x.jobs.history[0].state,'done'); assert.equal(x.jobs.history[0].analysis,null);
  } finally { x.native.disconnect(); }
});
test('completion notifications include readable BPM and long key', async () => {
  const x = await setup();
  const notifications = new Notifications(x.chrome,{native:x.native,getSettings:async () => DEFAULT_SETTINGS,badge:{flash:async () => {}},findJob:() => {}});
  await notifications.terminal({id:'done',state:'done',file:'song.mp3',analysis});
  assert.match(x.chrome.calls.notifications[0].message,/128 BPM · A minor/);
});
test('a finished download opens the drag tray instead of a toast when the host supports it', async () => {
  const requests = [];
  const native = { status: { state: 'ready', dropzone: true }, request: async (type, payload) => { requests.push([type, payload]); return { shown: 1 }; } };
  const calls = [];
  const chrome = { runtime: { getURL: p => p }, notifications: { onClicked: { addListener() {} }, onButtonClicked: { addListener() {} }, create: async (...a) => calls.push(a) } };
  const notifications = new Notifications(chrome,{native,getSettings:async () => DEFAULT_SETTINGS,badge:{flash:async () => {}},findJob:() => {}});
  await notifications.terminal({id:'done',state:'done',title:'Song',format:'wav',file:'C:\Music\Song.wav',analysis});
  assert.deepEqual(requests, [['showDrop', { items: [{ files: ['C:\Music\Song.wav'], title: 'Song', subtitle: '128 BPM · Am · WAV', service: 'other' }] }]]);
  assert.equal(calls.length, 0);
  native.status.dropzone = false;
  await notifications.terminal({id:'done',state:'done',title:'Song',format:'wav',file:'C:\Music\Song.wav',analysis});
  assert.equal(calls.length, 1);
});
