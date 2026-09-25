(function () {
  "use strict";

  var root = document.documentElement;
  var live = document.querySelector("[data-live]");
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  function announce(msg) {
    if (!live) return;
    live.textContent = "";
    window.setTimeout(function () { live.textContent = msg; }, 30);
  }

  /* ---------- theme: dark by default, remembered per visitor ---------- */
  var toggle = document.querySelector("[data-theme-toggle]");
  function syncToggle() {
    var dark = root.getAttribute("data-theme") !== "light";
    toggle.setAttribute("aria-label", dark ? "Switch to light mode" : "Switch to dark mode");
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", dark ? "#16181b" : "#eceee9");
  }
  if (toggle) {
    syncToggle();
    toggle.addEventListener("click", function () {
      var next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem("cxcap-theme", next); } catch (e) {}
      syncToggle();
      announce(next === "light" ? "Light mode on" : "Dark mode on");
    });
  }

  /* ---------- copy install command ---------- */
  document.querySelectorAll("[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var block = btn.closest("[data-copy-block]");
      var src = block.querySelector("[data-copy-source]");
      var text = (src.getAttribute("data-copy-text") || src.textContent).trim();
      var done = function () {
        btn.textContent = "Copied";
        btn.setAttribute("data-state", "done");
        announce("Install command copied");
        window.setTimeout(function () {
          btn.textContent = "Copy";
          btn.removeAttribute("data-state");
        }, 2000);
      };
      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(done, fallback);
      } else {
        fallback();
      }
      function fallback() {
        var ta = document.createElement("textarea");
        ta.value = text;
        ta.setAttribute("readonly", "");
        ta.style.position = "absolute";
        ta.style.left = "-9999px";
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand("copy"); done(); } catch (e) { announce("Select the command and copy it manually"); }
        document.body.removeChild(ta);
      }
    });
  });

  /* ---------- tabs (WAI-ARIA pattern, arrow keys) ---------- */
  document.querySelectorAll("[data-tabs]").forEach(function (tabs) {
    var list = Array.prototype.slice.call(tabs.querySelectorAll('[role="tab"]'));
    function select(tab, focus) {
      list.forEach(function (t) {
        var on = t === tab;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
      });
      if (focus) tab.focus();
    }
    list.forEach(function (tab, i) {
      tab.addEventListener("click", function () { select(tab, false); });
      tab.addEventListener("keydown", function (e) {
        var next = null;
        if (e.key === "ArrowRight") next = list[(i + 1) % list.length];
        if (e.key === "ArrowLeft") next = list[(i - 1 + list.length) % list.length];
        if (e.key === "Home") next = list[0];
        if (e.key === "End") next = list[list.length - 1];
        if (next) { e.preventDefault(); select(next, true); }
      });
    });
  });

  /* ---------- GitHub stars (shown only if the API answers) ---------- */
  var stars = document.querySelector("[data-stars]");
  if (stars && window.fetch) {
    fetch("https://api.github.com/repos/moonlettai/cxcap")
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (d) {
        if (d && typeof d.stargazers_count === "number" && d.stargazers_count > 0) {
          stars.textContent = d.stargazers_count.toLocaleString("en");
          stars.setAttribute("aria-label", d.stargazers_count + " stars");
          stars.hidden = false;
        }
      })
      .catch(function () {});
  }

  /* ---------- the locate map ----------
     Real data: cxcap 1.0.4, excalidraw@1118751,
     --focus packages/element/src/textMeasurements.ts
     11 direct importers (5 named in the output, "+6 more"), 323 reached. */
  var svg = document.querySelector("[data-map]");
  if (!svg) return;
  var NS = "http://www.w3.org/2000/svg";
  var DIRECT = [
    "packages/element/src/index.ts",
    "packages/element/src/textElement.ts",
    "packages/element/src/newElement.ts",
    "packages/element/src/stickyNote.ts",
    "packages/element/src/renderElement.ts",
    null, null, null, null, null, null
  ];
  var REACH = 323;
  var countDirect = document.querySelector("[data-count-direct]");
  var countReach = document.querySelector("[data-count-reach]");

  function el(name, attrs, parent) {
    var n = document.createElementNS(NS, name);
    for (var k in attrs) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }

  var cy = -8;
  var gReach = el("g", {}, svg);
  var gEdges = el("g", {}, svg);
  var gNodes = el("g", {}, svg);
  var gDig = el("g", {}, svg);

  // 323 reach marks in an elliptical band, golden-angle spread (deterministic)
  var reachDots = [];
  var golden = Math.PI * (3 - Math.sqrt(5));
  for (var i = 0; i < REACH; i++) {
    var t = (i + 0.5) / REACH;
    var band = 0.62 + 0.38 * Math.sqrt(t);
    var a = i * golden;
    var x = Math.cos(a) * 285 * band;
    var y = cy + Math.sin(a) * 222 * band;
    reachDots.push(el("circle", { cx: x.toFixed(1), cy: y.toFixed(1), r: 2.4, class: "m-reach" }, gReach));
  }

  // 11 direct importers on an inner ellipse
  var nodes = [];
  DIRECT.forEach(function (path, k) {
    var ang = -Math.PI / 2 + (k / DIRECT.length) * Math.PI * 2 + 0.12;
    var nx = Math.cos(ang) * 172;
    var ny = cy + Math.sin(ang) * 118;
    var edge = el("line", { x1: 0, y1: cy, x2: 0, y2: cy, class: "m-edge" }, gEdges);
    var g = el("g", { class: "m-node" }, gNodes);
    el("title", {}, g).textContent = path ? path + " imports it" : "Another direct importer (the output lists 5 by name)";
    el("circle", { cx: nx, cy: ny, r: 13, class: "m-direct-ring" }, g);
    el("circle", { cx: nx, cy: ny, r: 7, class: "m-direct" }, g);
    if (path) {
      var name = path.split("/").pop();
      var right = nx >= 0;
      var lbl = el("text", {
        x: nx + (right ? 20 : -20), y: ny + 4, class: "m-node-label",
        "text-anchor": right ? "start" : "end"
      }, g);
      lbl.textContent = name;
    }
    nodes.push({ g: g, edge: edge, x: nx, y: ny });
  });

  // the spot you want to change: white dashed outline, like a proposed dig
  var dig = el("rect", { x: -104, y: cy - 30, width: 208, height: 60, rx: 4, class: "m-dig-fill" }, gDig);
  var digLine = el("rect", { x: -104, y: cy - 30, width: 208, height: 60, rx: 4, class: "m-dig" }, gDig);
  var t1 = el("text", { x: 0, y: cy - 2, class: "m-label", "text-anchor": "middle" }, gDig);
  t1.textContent = "textMeasurements.ts";
  var t2 = el("text", { x: 0, y: cy + 16, class: "m-sublabel", "text-anchor": "middle" }, gDig);
  t2.textContent = "the file you'll change";

  function finalState() {
    digLine.style.strokeDashoffset = "0";
    [dig, digLine, t1, t2].forEach(function (n) { n.style.opacity = 1; });
    nodes.forEach(function (n) {
      n.g.style.opacity = 1;
      n.edge.setAttribute("x2", n.x);
      n.edge.setAttribute("y2", n.y);
    });
    reachDots.forEach(function (d) { d.style.opacity = 0.75; });
    if (countDirect) countDirect.textContent = "11";
    if (countReach) countReach.textContent = String(REACH);
  }

  var running = 0;
  function play() {
    var run = ++running;
    [dig, digLine, t1, t2].forEach(function (n) { n.style.opacity = 0; });
    nodes.forEach(function (n) {
      n.g.style.opacity = 0;
      n.edge.setAttribute("x2", 0);
      n.edge.setAttribute("y2", cy);
    });
    reachDots.forEach(function (d) { d.style.opacity = 0; });
    if (countDirect) countDirect.textContent = "0";
    if (countReach) countReach.textContent = "0";

    var start = null;
    var T_DIG = 500, T_NODES = 1300, T_REACH = 2900;
    function frame(ts) {
      if (run !== running) return;
      if (start === null) start = ts;
      var e = ts - start;

      var pd = Math.min(1, e / T_DIG);
      [dig, digLine, t1, t2].forEach(function (n) { n.style.opacity = pd; });

      var shown = 0;
      nodes.forEach(function (n, k) {
        var local = (e - T_DIG - k * 70) / 260;
        var p = Math.max(0, Math.min(1, local));
        var ease = 1 - Math.pow(1 - p, 3);
        n.edge.setAttribute("x2", (n.x * ease).toFixed(1));
        n.edge.setAttribute("y2", (cy + (n.y - cy) * ease).toFixed(1));
        n.g.style.opacity = p;
        if (p >= 1) shown++;
      });
      if (countDirect) countDirect.textContent = String(shown);

      var pr = Math.max(0, Math.min(1, (e - T_NODES) / (T_REACH - T_NODES)));
      var lit = Math.round(REACH * (1 - Math.pow(1 - pr, 2)));
      for (var j = 0; j < REACH; j++) reachDots[j].style.opacity = j < lit ? 0.75 : 0;
      if (countReach) countReach.textContent = String(lit);

      if (e < T_REACH + 50) window.requestAnimationFrame(frame);
      else finalState();
    }
    window.requestAnimationFrame(frame);
  }

  if (reduceMotion) {
    finalState();
  } else if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) { io.disconnect(); play(); }
    }, { threshold: 0.35 });
    finalState();
    io.observe(svg);
  } else {
    finalState();
  }

  var replay = document.querySelector("[data-replay]");
  if (replay) {
    if (reduceMotion) replay.hidden = true;
    replay.addEventListener("click", function () { play(); });
  }
})();
