const params = new URLSearchParams(location.search);
if (params.has('capture')) document.body.classList.add('capture');
document.querySelector('#theme').value = params.get('theme') || 'dark';
document.querySelector('#scenario').value = params.get('state') || 'media';
for (const id of ['theme', 'scenario']) document.getElementById(id).addEventListener('change', () => {
  const next = new URL(location.href); next.searchParams.set('theme', document.querySelector('#theme').value); next.searchParams.set('state', document.querySelector('#scenario').value); location.href = next.href;
});
window.__previewErrors = [];
window.addEventListener('message', event => { if (event.source === document.querySelector('iframe').contentWindow && event.data?.siphonError) window.__previewErrors.push(event.data.siphonError); });
const response = await fetch('../popup.html');
if (!response.ok) throw new Error(`Popup HTML: ${response.status}`);
const html = await response.text();
const base = new URL('../', location.href).href;
const freeze = params.has('freeze') ? '<style>*,*::before,*::after{animation:none!important;transition:none!important}</style>' : '';
document.querySelector('#popup').srcdoc = html.replace('<head>', `<head><base href="${base}">${freeze}<script src="dev/mock.js"></script>`);
