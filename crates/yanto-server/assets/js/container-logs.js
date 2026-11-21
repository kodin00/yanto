// Live log follow over SSE. Log text is inserted with textContent, never as HTML.
(() => {
  const pre = document.getElementById("log");
  const box = document.getElementById("follow");
  const state = document.getElementById("follow-state");
  if (!pre || !box) return;
  const MAX_LINES = 5000;
  let source = null;
  let lastSeen = Math.floor(Date.now() / 1000);

  const append = (line) => {
    const span = document.createElement("span");
    span.className = line.stream;
    span.textContent = `${line.timestamp ? line.timestamp + " " : ""}${line.text}\n`;
    pre.appendChild(span);
    while (pre.childElementCount > MAX_LINES) pre.removeChild(pre.firstElementChild);
    pre.scrollTop = pre.scrollHeight;
    if (line.timestamp) {
      const t = Date.parse(line.timestamp);
      if (!Number.isNaN(t)) lastSeen = Math.floor(t / 1000);
    }
  };

  const open = () => {
    source = new EventSource(`${pre.dataset.streamUrl}?since=${lastSeen}`);
    state.textContent = "following";
    source.onmessage = (ev) => append(JSON.parse(ev.data));
    source.addEventListener("error", (ev) => {
      if (ev.data) state.textContent = `error: ${ev.data}`;
    });
    source.onerror = () => {
      // Browser retries by itself; resume from the last line seen instead of the original `since`.
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
