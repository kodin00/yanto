// Keep status badges on the Deployments page current from the /events stream.
(() => {
  if (!document.querySelector('[data-deployment-status]')) return;
  const source = new EventSource('/events');
  const update = (e) => {
    const ev = JSON.parse(e.data);
    const badge = document.querySelector('[data-deployment-status="' + CSS.escape(ev.deployment_id) + '"]');
    if (badge) {
      badge.textContent = ev.status;
      badge.className = 'badge ' + ev.status;
    }
  };
  source.addEventListener('status', update);
  source.addEventListener('done', update);
})();
