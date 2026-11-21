// Live follow of the application log over SSE. Text only (textContent), never HTML.
(() => {
  const pre = document.getElementById("app-log");
  const box = document.getElementById("follow");
  const state = document.getElementById("follow-state");
  if (!pre || !box) return;
  const MAX_LINES = 500;
  let source = null;
  let lastSeq = Number(pre.dataset.lastSeq) || 0;

  const append = (line) => {
    lastSeq = Math.max(lastSeq, line.seq);
    const span = document.createElement("span");
    span.className = String(line.level).toLowerCase();
    span.textContent = `${line.target} ${line.text}\n`;
    pre.appendChild(span);
    while (pre.childElementCount > MAX_LINES) pre.removeChild(pre.firstElementChild);
    pre.scrollTop = pre.scrollHeight;
  };

  const open = () => {
    source = new EventSource(`${pre.dataset.streamUrl}?after=${lastSeq}`);
    state.textContent = "following";
    source.onmessage = (ev) => append(JSON.parse(ev.data));
    source.onerror = () => {
      source.close();
      state.textContent = "reconnecting";
      setTimeout(() => box.checked && open(), 2000);
    };
  };

  box.addEventListener("change", () => {
    if (box.checked) open();
    else {
      source && source.close();
      state.textContent = "";
    }
  });
  pre.scrollTop = pre.scrollHeight;
})();
