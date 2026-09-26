/* Loaded as a module: scoped and strict by default. */

var root = document.documentElement;
var live = document.querySelector("[data-live]");
var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function announce(msg) {
  if (!live) return;
  live.textContent = "";
  window.setTimeout(function () { live.textContent = msg; }, 30);
}

/* ---------- theme: dark by default, remembered per visitor ---------- */
function syncToggle(toggle) {
  var dark = root.getAttribute("data-theme") !== "light";
  toggle.setAttribute("aria-label", dark ? "Switch to light mode" : "Switch to dark mode");
  var meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", dark ? "#16181b" : "#eceee9");
}

function initTheme() {
  var toggle = document.querySelector("[data-theme-toggle]");
  if (!toggle) return;
  syncToggle(toggle);
  toggle.addEventListener("click", function () {
    var next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
    root.setAttribute("data-theme", next);
    try { localStorage.setItem("cxcap-theme", next); } catch (e) {}
    syncToggle(toggle);
    announce(next === "light" ? "Light mode on" : "Dark mode on");
  });
}

/* ---------- copy install command ---------- */
function copyDone(btn) {
  btn.textContent = "Copied";
  btn.setAttribute("data-state", "done");
  announce("Install command copied");
  window.setTimeout(function () {
    btn.textContent = "Copy";
    btn.removeAttribute("data-state");
  }, 2000);
}

function copyFallback(text, btn) {
  var ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "absolute";
  ta.style.left = "-9999px";
  document.body.appendChild(ta);
  ta.select();
  try { document.execCommand("copy"); copyDone(btn); } catch (e) { announce("Select the command and copy it manually"); }
  document.body.removeChild(ta);
}

function copyFrom(btn) {
  var block = btn.closest("[data-copy-block]");
  var src = block.querySelector("[data-copy-source]");
  var text = (src.getAttribute("data-copy-text") || src.textContent).trim();
  if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(text).then(
      function () { copyDone(btn); },
      function () { copyFallback(text, btn); }
    );
  } else {
    copyFallback(text, btn);
  }
}

function initCopy() {
  document.querySelectorAll("[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () { copyFrom(btn); });
  });
}

/* ---------- tabs (WAI-ARIA pattern, arrow keys) ---------- */
var TAB_KEYS = {
  ArrowRight: function (i, n) { return (i + 1) % n; },
  ArrowLeft: function (i, n) { return (i - 1 + n) % n; },
  Home: function () { return 0; },
  End: function (i, n) { return n - 1; }
};

function selectTab(list, tab, focus) {
  list.forEach(function (t) {
    var on = t === tab;
    t.setAttribute("aria-selected", on ? "true" : "false");
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
  });
  if (focus) tab.focus();
}

function initTabGroup(tabs) {
  var list = Array.prototype.slice.call(tabs.querySelectorAll('[role="tab"]'));
  list.forEach(function (tab, i) {
    tab.addEventListener("click", function () { selectTab(list, tab, false); });
    tab.addEventListener("keydown", function (e) {
      var move = TAB_KEYS[e.key];
      if (!move) return;
      e.preventDefault();
      selectTab(list, list[move(i, list.length)], true);
    });
  });
}

function initTabs() {
  document.querySelectorAll("[data-tabs]").forEach(initTabGroup);
}

/* ---------- GitHub stars (shown only if the API answers) ---------- */
function showStars(stars, d) {
  if (!d || typeof d.stargazers_count !== "number" || d.stargazers_count <= 0) return;
  stars.textContent = d.stargazers_count.toLocaleString("en");
  stars.setAttribute("aria-label", d.stargazers_count + " stars");
  stars.hidden = false;
}

function initStars() {
  var stars = document.querySelector("[data-stars]");
  if (!stars || !window.fetch) return;
  fetch("https://api.github.com/repos/moonlettai/cxcap")
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (d) { showStars(stars, d); })
    .catch(function () {});
}

/* ---------- the locate map ----------
   Real data: cxcap 1.0.4, excalidraw@1118751,
   --focus packages/element/src/textMeasurements.ts
   11 direct importers (5 named in the output, "+6 more"), 323 reached. */
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
var CY = -8;
var T_DIG = 500, T_NODES = 1300, T_REACH = 2900;

function el(name, attrs, parent) {
  var n = document.createElementNS(NS, name);
  for (var k in attrs) n.setAttribute(k, attrs[k]);
  if (parent) parent.appendChild(n);
  return n;
}

function setCount(node, value) {
  if (node) node.textContent = String(value);
}

// 323 reach marks in an elliptical band, golden-angle spread (deterministic)
function drawReach(g) {
  var dots = [];
  var golden = Math.PI * (3 - Math.sqrt(5));
  for (var i = 0; i < REACH; i++) {
    var band = 0.62 + 0.38 * Math.sqrt((i + 0.5) / REACH);
    var a = i * golden;
    var x = Math.cos(a) * 285 * band;
    var y = CY + Math.sin(a) * 222 * band;
    dots.push(el("circle", { cx: x.toFixed(1), cy: y.toFixed(1), r: 2.4, class: "m-reach" }, g));
  }
  return dots;
}

function drawNodeLabel(g, path, nx, ny) {
  var right = nx >= 0;
  var lbl = el("text", {
    x: nx + (right ? 20 : -20), y: ny + 4, class: "m-node-label",
    "text-anchor": right ? "start" : "end"
  }, g);
  lbl.textContent = path.split("/").pop();
}

// 11 direct importers on an inner ellipse
function drawNode(path, k, gEdges, gNodes) {
  var ang = -Math.PI / 2 + (k / DIRECT.length) * Math.PI * 2 + 0.12;
  var nx = Math.cos(ang) * 172;
  var ny = CY + Math.sin(ang) * 118;
  var edge = el("line", { x1: 0, y1: CY, x2: 0, y2: CY, class: "m-edge" }, gEdges);
  var g = el("g", { class: "m-node" }, gNodes);
  el("title", {}, g).textContent = path ? path + " imports it" : "Another direct importer (the output lists 5 by name)";
  el("circle", { cx: nx, cy: ny, r: 13, class: "m-direct-ring" }, g);
  el("circle", { cx: nx, cy: ny, r: 7, class: "m-direct" }, g);
  if (path) drawNodeLabel(g, path, nx, ny);
  return { g: g, edge: edge, x: nx, y: ny };
}

// the spot you want to change: white dashed outline, like a proposed dig
function drawDig(g) {
  var fill = el("rect", { x: -104, y: CY - 30, width: 208, height: 60, rx: 4, class: "m-dig-fill" }, g);
  var line = el("rect", { x: -104, y: CY - 30, width: 208, height: 60, rx: 4, class: "m-dig" }, g);
  var t1 = el("text", { x: 0, y: CY - 2, class: "m-label", "text-anchor": "middle" }, g);
  t1.textContent = "textMeasurements.ts";
  var t2 = el("text", { x: 0, y: CY + 16, class: "m-sublabel", "text-anchor": "middle" }, g);
  t2.textContent = "the file you'll change";
  return { line: line, parts: [fill, line, t1, t2] };
}

function buildMap(svg) {
  var gReach = el("g", {}, svg);
  var gEdges = el("g", {}, svg);
  var gNodes = el("g", {}, svg);
  var gDig = el("g", {}, svg);
  return {
    reachDots: drawReach(gReach),
    nodes: DIRECT.map(function (path, k) { return drawNode(path, k, gEdges, gNodes); }),
    dig: drawDig(gDig),
    countDirect: document.querySelector("[data-count-direct]"),
    countReach: document.querySelector("[data-count-reach]")
  };
}

function setDigOpacity(map, value) {
  map.dig.parts.forEach(function (n) { n.style.opacity = value; });
}

function setNode(n, p) {
  var ease = 1 - Math.pow(1 - p, 3);
  n.edge.setAttribute("x2", (n.x * ease).toFixed(1));
  n.edge.setAttribute("y2", (CY + (n.y - CY) * ease).toFixed(1));
  n.g.style.opacity = p;
}

function setReach(map, lit) {
  for (var j = 0; j < REACH; j++) map.reachDots[j].style.opacity = j < lit ? 0.75 : 0;
  setCount(map.countReach, lit);
}

function finalState(map) {
  map.dig.line.style.strokeDashoffset = "0";
  setDigOpacity(map, 1);
  map.nodes.forEach(function (n) { setNode(n, 1); });
  setReach(map, REACH);
  setCount(map.countDirect, map.nodes.length);
}

function drawFrame(map, e) {
  setDigOpacity(map, Math.min(1, e / T_DIG));
  var shown = 0;
  map.nodes.forEach(function (n, k) {
    var p = Math.max(0, Math.min(1, (e - T_DIG - k * 70) / 260));
    setNode(n, p);
    if (p >= 1) shown++;
  });
  setCount(map.countDirect, shown);
  var pr = Math.max(0, Math.min(1, (e - T_NODES) / (T_REACH - T_NODES)));
  setReach(map, Math.round(REACH * (1 - Math.pow(1 - pr, 2))));
}

function makePlayer(map) {
  var running = 0;
  return function play() {
    var run = ++running;
    var start = null;
    drawFrame(map, 0);
    function frame(ts) {
      if (run !== running) return;
      if (start === null) start = ts;
      var e = ts - start;
      if (e < T_REACH + 50) {
        drawFrame(map, e);
        window.requestAnimationFrame(frame);
      } else {
        finalState(map);
      }
    }
    window.requestAnimationFrame(frame);
  };
}

function playWhenVisible(svg, play) {
  if (reduceMotion || !("IntersectionObserver" in window)) return;
  var io = new IntersectionObserver(function (entries) {
    if (entries[0].isIntersecting) { io.disconnect(); play(); }
  }, { threshold: 0.35 });
  io.observe(svg);
}

function initReplay(play) {
  var replay = document.querySelector("[data-replay]");
  if (!replay) return;
  if (reduceMotion) replay.hidden = true;
  replay.addEventListener("click", play);
}

function initMap() {
  var svg = document.querySelector("[data-map]");
  if (!svg) return;
  var map = buildMap(svg);
  var play = makePlayer(map);
  finalState(map);
  playWhenVisible(svg, play);
  initReplay(play);
}

initTheme();
initCopy();
initTabs();
initStars();
initMap();
