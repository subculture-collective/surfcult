// The surfcult web admin: reads the API in this server and draws the guide,
// the channels and the system's health. Read only; nothing here changes the
// TV. See docs/reference/api.md for the reads and docs/design/cable-grid.md
// for the look.
"use strict";

const SLOT_MS = 30 * 60 * 1000;
const GUIDE_SLOTS = 6;
const REFRESH_MS = 30 * 1000;
const TOKEN_KEY = "surfcult-token";
const VIEWS = ["guide", "channels", "system"];

let timezone;

// ---------- small helpers ----------

/** An element with attributes and children; strings become text. */
function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (name === "class") node.className = value;
    else if (name === "style") Object.assign(node.style, value);
    else node.setAttribute(name, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function storedToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || "";
  } catch {
    return "";
  }
}

function storeToken(token) {
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Without storage the token lasts until the page is closed.
  }
  sessionToken = token;
}

let sessionToken = storedToken();

class NeedsToken extends Error {}

/** Reads `path` from the API as JSON, with the token when there is one. */
async function api(path) {
  const headers = sessionToken ? { Authorization: `Bearer ${sessionToken}` } : {};
  const response = await fetch(path, { headers, cache: "no-store" });
  if (response.status === 401) throw new NeedsToken();
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || `${path} answered ${response.status}`);
  return body;
}

/** `HH:MM` in the TV's time zone. */
function hm(ms) {
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    timeZone: timezone,
  }).format(new Date(ms));
}

function hms(ms) {
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
    timeZone: timezone,
  }).format(new Date(ms));
}

function day(ms) {
  return new Intl.DateTimeFormat("en-GB", {
    weekday: "long",
    day: "numeric",
    month: "long",
    timeZone: timezone,
  }).format(new Date(ms));
}

/** "4 min ago", "3 h ago" and so on. */
function ago(ms, now) {
  const minutes = Math.round((now - ms) / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 120) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} days ago`;
}

function pad(n) {
  return String(n).padStart(2, "0");
}

function onAt(cells, at) {
  return cells.find((c) => c.start_ms <= at && at < c.end_ms);
}

function nextAfter(cells, at) {
  return cells.find((c) => c.start_ms >= at);
}

// ---------- the views ----------

async function loadGuide() {
  const from = Date.now() - (Date.now() % SLOT_MS);
  const guide = await api(`/guide?from=${from}&to=${from + GUIDE_SLOTS * SLOT_MS}`);
  timezone = guide.timezone;
  return { guide };
}

function guideView({ guide }) {
  const { from_ms: from, to_ms: to, now_ms: now } = guide;
  const span = to - from;
  const pct = (ms) => `${((Math.min(Math.max(ms, from), to) - from) / span) * 100}%`;
  const slots = el(
    "div",
    { class: "slots", style: { gridTemplateColumns: `repeat(${GUIDE_SLOTS}, 1fr)` } },
    Array.from({ length: GUIDE_SLOTS }, (_, i) => el("div", { class: "slot" }, hm(from + i * SLOT_MS))),
  );
  const rows = guide.channels.map((channel) =>
    el(
      "div",
      { class: "row" },
      el("div", { class: "ch" }, pad(channel.number), el("br"), channel.name),
      el(
        "div",
        { class: "lane" },
        channel.cells.map((cell) => {
          const on = cell.start_ms <= now && now < cell.end_ms;
          const classes = ["cell", cell.movie && "movie", on && "on", cell.start_ms < from && "before", cell.end_ms > to && "after"];
          return el(
            "div",
            {
              class: classes.filter(Boolean).join(" "),
              style: { left: pct(cell.start_ms), width: `calc(${pct(cell.end_ms)} - ${pct(cell.start_ms)} - 4px)` },
              title: `${cell.title}, ${hm(cell.start_ms)}–${hm(cell.end_ms)}`,
            },
            cell.title,
            el("small", {}, `${hm(cell.start_ms)}–${hm(cell.end_ms)}`),
          );
        }),
        el("div", { class: "now", style: { left: pct(now) }, "aria-hidden": "true" }),
      ),
    ),
  );
  const grid = el(
    "div",
    { class: "grid" },
    el(
      "div",
      { class: "grid-inner" },
      el("div", { class: "row head" }, el("div", { class: "ch clock-cell" }, hm(now)), slots),
      rows,
    ),
  );
  return [
    el("div", { class: "view-head" }, el("h2", {}, "Tonight on surfcult"), el("p", {}, day(now))),
    guide.channels.length ? grid : el("p", { class: "note" }, "channels.toml has no channels."),
    onNowList(guide),
  ];
}

/** Each channel with what is on now and its progress: the phone's guide. */
function onNowList(guide) {
  const now = guide.now_ms;
  return el(
    "div",
    { class: "onnow", "aria-label": "On now" },
    guide.channels.map((channel) => {
      const cell = onAt(channel.cells, now);
      const next = nextAfter(channel.cells, cell ? cell.end_ms : now);
      const progress = cell ? ((now - cell.start_ms) / (cell.end_ms - cell.start_ms)) * 100 : 0;
      return el(
        "div",
        { class: "item" },
        el("div", { class: "n" }, pad(channel.number), el("small", {}, channel.name)),
        el(
          "div",
          {},
          el("div", { class: "t" }, cell ? cell.title : "Off the air"),
          el(
            "div",
            { class: "s" },
            cell ? `Until ${hm(cell.end_ms)}. ` : "",
            next ? `Next at ${hm(next.start_ms)}: ${next.title}` : "",
          ),
          cell && el("div", { class: "bar-track" }, el("i", { style: { width: `${progress}%` } })),
        ),
      );
    }),
  );
}

async function loadChannels() {
  const [{ channels }, guide] = await Promise.all([api("/channels"), api("/guide")]);
  timezone = guide.timezone;
  return { channels, guide };
}

function channelsView({ channels, guide }) {
  const now = guide.now_ms;
  const modes = { sequential: "In order", shuffle: "Shuffle", weighted: "Weighted", blocks: "Blocks" };
  const body = channels.map((channel) => {
    const cells = guide.channels.find((c) => c.number === channel.number)?.cells ?? [];
    const cell = onAt(cells, now);
    const next = nextAfter(cells, cell ? cell.end_ms : now);
    const progress = cell ? ((now - cell.start_ms) / (cell.end_ms - cell.start_ms)) * 100 : 0;
    return el(
      "tr",
      {},
      el("td", { class: "num" }, pad(channel.number)),
      el("td", {}, el("strong", {}, channel.name), el("br"), el("small", {}, modes[channel.mode] ?? channel.mode)),
      el(
        "td",
        {},
        cell ? cell.title : el("small", {}, "Off the air"),
        cell && el("div", { class: "bar-track" }, el("i", { style: { width: `${progress}%` } })),
      ),
      el("td", {}, next ? [`${hm(next.start_ms)} `, el("small", {}, next.title)] : el("small", {}, "Nothing in the next three hours")),
    );
  });
  return [
    el("div", { class: "view-head" }, el("h2", {}, "Channels"), el("p", {}, "From channels.toml. Edit that file to change them.")),
    el(
      "div",
      { class: "panel table-wrap" },
      el(
        "table",
        {},
        el("thead", {}, el("tr", {}, ["CH", "NAME", "ON NOW", "NEXT"].map((h) => el("th", { scope: "col" }, h)))),
        el("tbody", {}, body),
      ),
    ),
  ];
}

async function loadSystem() {
  const [health, { channels }, guide] = await Promise.all([api("/health"), api("/channels"), api("/guide")]);
  timezone = guide.timezone;
  return { health, channels };
}

function systemView({ health, channels }) {
  const now = health.now_ms;
  const names = new Map(channels.map((c) => [c.number, c.name]));
  const rows = health.channels.map((c) => {
    const until = c.scheduled_until_ms;
    let state;
    if (until === null) state = el("span", { class: "pill bad" }, "No schedule");
    else if (until - now < 24 * 60 * 60 * 1000) state = el("span", { class: "pill warn" }, `Runs out in ${Math.max(0, Math.round((until - now) / 3600000))} h`);
    else state = el("span", { class: "pill" }, `${Math.round((until - now) / 86400000)} days ahead`);
    return el(
      "tr",
      {},
      el("td", { class: "num" }, pad(c.number)),
      el("td", {}, names.get(c.number) ?? ""),
      el("td", {}, until === null ? "—" : `${day(until)} ${hm(until)}`),
      el("td", {}, state),
    );
  });
  return [
    el("div", { class: "view-head" }, el("h2", {}, "System")),
    el(
      "div",
      { class: "facts" },
      el("div", { class: "panel fact" }, el("h3", {}, "STATUS"), el("p", {}, health.status === "ok" ? "Running" : health.status)),
      el(
        "div",
        { class: "panel fact" },
        el("h3", {}, "LIBRARY SYNCED"),
        el("p", {}, health.library_synced_at_ms === null ? "Not yet" : ago(health.library_synced_at_ms, now)),
      ),
      el("div", { class: "panel fact" }, el("h3", {}, "DATABASE"), el("p", {}, `Schema ${health.schema_version}`)),
      el("div", { class: "panel fact" }, el("h3", {}, "TIME ZONE"), el("p", {}, timezone ?? "")),
    ),
    el(
      "div",
      { class: "panel table-wrap" },
      el(
        "table",
        {},
        el("thead", {}, el("tr", {}, ["CH", "NAME", "SCHEDULED UNTIL", ""].map((h) => el("th", { scope: "col" }, h)))),
        el("tbody", {}, rows),
      ),
    ),
  ];
}

const ROUTES = {
  guide: [loadGuide, guideView],
  channels: [loadChannels, channelsView],
  system: [loadSystem, systemView],
};

// ---------- the page ----------

function currentView() {
  const name = location.hash.slice(1);
  return VIEWS.includes(name) ? name : "guide";
}

async function render() {
  const name = currentView();
  for (const link of document.querySelectorAll(".bar nav a")) {
    if (link.dataset.view === name) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  const view = document.getElementById("view");
  const [load, draw] = ROUTES[name];
  try {
    const data = await load();
    document.getElementById("token").hidden = true;
    view.replaceChildren(...draw(data));
    document.getElementById("status").textContent = `Updated ${hms(Date.now())}. Refreshes every 30 seconds.`;
  } catch (error) {
    if (error instanceof NeedsToken) {
      const panel = document.getElementById("token");
      document.getElementById("token-error").hidden = !sessionToken;
      panel.hidden = false;
      view.replaceChildren(el("p", { class: "note" }, "surfcult needs a token to show this."));
      document.getElementById("token-input").focus();
      return;
    }
    view.replaceChildren(
      el("div", { class: "panel problem" }, el("h2", {}, "Cannot read surfcult"), el("p", { class: "error" }, error.message), el("p", {}, "It tries again in 30 seconds.")),
    );
  }
}

function tick() {
  const clock = document.getElementById("clock");
  clock.textContent = timezone ? hms(Date.now()) : "";
}

document.getElementById("token-form").addEventListener("submit", (event) => {
  event.preventDefault();
  storeToken(document.getElementById("token-input").value.trim());
  render();
});

window.addEventListener("hashchange", render);
render();
setInterval(render, REFRESH_MS);
setInterval(tick, 1000);
