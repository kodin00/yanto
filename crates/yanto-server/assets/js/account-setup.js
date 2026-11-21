// Invite/reset links carry the token in the URL fragment (#token=...), which browsers never
// send to the server, so it stays out of access logs and Referer headers.
(function () {
  function init() {
    var m = /[#&]token=([^&]+)/.exec(window.location.hash);
    var input = document.getElementById("token");
    if (!m || !input) return;
    input.value = decodeURIComponent(m[1]);
    window.history.replaceState(null, "", window.location.pathname);
    document.body.dispatchEvent(new Event("token-ready"));
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
