// Pitlane's folder picker inside VS Code's side bar: the repository or a lane's worktree. The app
// sets the places (window.__pitlanePlaces); choosing one asks the app, by a pitlane:// navigation
// it intercepts, to show that folder's VS Code.
(function () {
  if (window.__pitlanePlace) return;
  window.__pitlanePlace = true;
  var here = new URLSearchParams(location.search).get("folder") || "";
  var places = window.__pitlanePlaces || [];
  var icons = {
    folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
    branch: '<svg viewBox="0 0 24 24"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 4-6 3-12 7"/></svg>',
    chev: '<svg viewBox="0 0 24 24"><path d="M7 10l5 5 5-5"/></svg>',
  };
  function current() {
    for (var i = 0; i < places.length; i++) if (places[i].root === here) return places[i];
    return { root: here, label: here.split("/").pop() || "folder", kind: "folder" };
  }
  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; });
  }
  function close() {
    var m = document.getElementById("pitlane-places");
    if (m) m.remove();
  }
  function open(btn) {
    if (document.getElementById("pitlane-places")) return close();
    var r = btn.getBoundingClientRect();
    var m = document.createElement("div");
    m.id = "pitlane-places";
    m.setAttribute("role", "menu");
    m.style.left = r.left + "px";
    m.style.top = r.bottom + 4 + "px";
    function item(p) {
      return '<button type="button" role="menuitem" data-root="' + esc(p.root) + '" aria-current="' + (p.root === here) + '">' + (p.kind === "lane" ? icons.branch : icons.folder) + '<span class="pl-label">' + esc(p.label) + '</span><span class="pl-sub">' + esc(p.root.replace(/^\/Users\/[^/]+/, "~")) + "</span></button>";
    }
    var repo = places.filter(function (p) { return p.kind !== "lane"; });
    var lanes = places.filter(function (p) { return p.kind === "lane"; });
    if (!repo.length) repo = [current()];
    m.innerHTML =
      '<div class="pl-head">Repository</div>' + repo.map(item).join("") +
      '<div class="pl-head">Lane worktrees' + (lanes.length ? ' <span class="pl-count">' + lanes.length + "</span>" : "") + "</div>" +
      (lanes.length ? lanes.map(item).join("") : '<p class="pl-empty">No lane has a worktree yet. A lane gets one when its card starts.</p>');
    m.addEventListener("click", function (e) {
      var b = e.target.closest("button[data-root]");
      if (!b) return;
      close();
      if (b.dataset.root !== here) location.href = "pitlane://place?root=" + encodeURIComponent(b.dataset.root);
    });
    // VS Code's workbench cancels wheel events that reach it, so the list never scrolled by mouse:
    // scroll it here and keep the event from going further.
    m.addEventListener("wheel", function (e) {
      e.preventDefault();
      e.stopPropagation();
      var unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? m.clientHeight : 1;
      m.scrollTop += e.deltaY * unit;
    }, { passive: false });
    m.addEventListener("keydown", function (e) {
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      e.preventDefault();
      var all = Array.prototype.slice.call(m.querySelectorAll("button[data-root]"));
      var i = all.indexOf(document.activeElement);
      var next = all[(i + (e.key === "ArrowDown" ? 1 : all.length - 1)) % all.length];
      if (next) next.focus();
    });
    (document.querySelector(".monaco-workbench") || document.body).appendChild(m);
    var first = m.querySelector('[aria-current="true"]') || m.querySelector("button");
    if (first) first.focus();
  }
  function render() {
    var title = document.querySelector(".part.titlebar .titlebar-left");
    if (!title) return;
    var btn = document.getElementById("pitlane-place");
    if (!btn) {
      btn = document.createElement("button");
      btn.id = "pitlane-place";
      btn.type = "button";
      btn.title = "Choose the folder: the repository or a lane's worktree";
      // VS Code's title bar handles presses on itself (dragging, focus): keep ours from it.
      btn.addEventListener("pointerdown", function (e) {
        e.stopPropagation();
      });
      btn.addEventListener("mousedown", function (e) {
        e.stopPropagation();
      });
      btn.addEventListener("click", function (e) {
        e.preventDefault();
        e.stopPropagation();
        open(btn);
      });
      btn.addEventListener("keydown", function (e) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          if (!document.getElementById("pitlane-places")) open(btn);
        }
      });
      title.insertBefore(btn, title.firstChild);
    }
    // Redrawn only when the place changes. (Comparing innerHTML never matched, the browser writes
    // SVG back differently, so the button's insides were replaced on every mutation, forever: the
    // press landed on a node gone by the release, and no click ever came.)
    var c = current();
    var key = c.kind + "\n" + c.root + "\n" + c.label;
    if (btn.dataset.key === key) return;
    btn.dataset.key = key;
    btn.innerHTML = (c.kind === "lane" ? icons.branch : icons.folder) + '<span class="pl-name">' + esc(c.label) + "</span>" + icons.chev;
  }
  window.__pitlaneSetPlaces = function (p) {
    places = p || [];
    render();
  };
  document.addEventListener("pointerdown", function (e) {
    var m = document.getElementById("pitlane-places");
    if (m && !m.contains(e.target) && e.target.id !== "pitlane-place" && !e.target.closest("#pitlane-place")) close();
  }, true);
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") close();
  }, true);
  var pending = false;
  function watch() {
    new MutationObserver(function () {
      if (pending) return;
      pending = true;
      requestAnimationFrame(function () {
        pending = false;
        render();
      });
    }).observe(document.body, { childList: true, subtree: true });
    render();
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", watch);
  else watch();
  // The secondary side bar held the chat, which Pitlane turns off: close it while it shows nothing
  // but that (VS Code's own ⌥⌘B, the one way that works in every layout).
  function hideAux() {
    var close = document.querySelector(".part.auxiliarybar .codicon-auxiliarybar-close");
    if (close) return close.click();
    var ev = new KeyboardEvent("keydown", { key: "b", code: "KeyB", metaKey: true, altKey: true, bubbles: true, cancelable: true });
    try {
      Object.defineProperty(ev, "keyCode", { get: function () { return 66; } });
      Object.defineProperty(ev, "which", { get: function () { return 66; } });
    } catch (e) {}
    (document.activeElement || document.body).dispatchEvent(ev);
  }
  var tries = 0;
  var closer = setInterval(function () {
    tries++;
    var aux = document.querySelector(".part.auxiliarybar");
    var text = aux ? aux.innerText.trim() : "";
    if (aux && aux.offsetWidth > 0 && (text === "" || text === "Chat" || /^Drag a view here/.test(text))) {
      hideAux();
      clearInterval(closer);
    }
    if (tries > 20) clearInterval(closer);
  }, 500);
})();
