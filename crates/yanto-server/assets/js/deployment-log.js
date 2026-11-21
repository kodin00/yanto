// Live deployment log over SSE. The server sends HTML that is already escaped (and only
// whitelisted <span class="ansi-N"> markup), so insertAdjacentHTML is safe here.
(() => {
  const log = document.getElementById('log');
  if (!log || !log.dataset.stream) return;
  const badge = document.getElementById('status');
  const setStatus = (s) => {
    if (!badge) return;
    badge.textContent = s;
    badge.className = 'badge ' + s;
  };
  const source = new EventSource(log.dataset.stream);
  source.addEventListener('log', (e) => {
    const atBottom = window.innerHeight + window.scrollY >= document.body.scrollHeight - 40;
    log.insertAdjacentHTML('beforeend', e.data);
    if (atBottom) window.scrollTo(0, document.body.scrollHeight);
  });
  source.addEventListener('status', (e) => setStatus(e.data));
  source.addEventListener('done', (e) => {
    setStatus(e.data);
    source.close();
  });
})();
