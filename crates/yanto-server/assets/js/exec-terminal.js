// xterm.js terminal over the exec WebSocket. Typed input goes out as binary frames, shell
// output comes back as binary frames, resize and notices use JSON/text frames.
(() => {
  const host = document.getElementById("terminal");
  const status = document.getElementById("terminal-status");
  if (!host || typeof Terminal === "undefined") return;

  const term = new Terminal({
    cursorBlink: true,
    fontFamily: "ui-monospace, Menlo, Consolas, monospace",
    fontSize: 14,
    scrollback: 5000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(host);
  fit.fit();

  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const url = `${scheme}://${location.host}${host.dataset.wsPath}?cols=${term.cols}&rows=${term.rows}`;
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  const enc = new TextEncoder();

  ws.onopen = () => {
    status.textContent = "Connected";
    term.focus();
  };
  ws.onmessage = (ev) => {
    if (typeof ev.data === "string") term.write(ev.data);
    else term.write(new Uint8Array(ev.data));
  };
  ws.onclose = (ev) => {
    status.textContent = ev.reason ? `Closed: ${ev.reason}` : "Closed";
    term.write("\r\n\x1b[2m[session closed]\x1b[0m\r\n");
    term.options.disableStdin = true;
  };
  ws.onerror = () => {
    status.textContent = "Connection failed";
  };

  term.onData((data) => {
    if (ws.readyState === WebSocket.OPEN) ws.send(enc.encode(data));
  });
  const sendSize = () => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
    }
  };
  term.onResize(sendSize);
  window.addEventListener("resize", () => fit.fit());
})();
