// The surfcult web admin: reads the API in this server and draws the guide,
// the channels and the system's health. When the server runs inside
// surfcult run it can also tune the TV, set its CRT filter and edit a
// channel's settings in channels.toml (/tv, /tune, /crt, /channels/N). See docs/reference/api.md for the API and docs/design/cable-grid.md
// for the look.
"use strict";

const SLOT_MS = 30 * 60 * 1000;
const GUIDE_SLOTS = 6;
const REFRESH_MS = 30 * 1000;
const TOKEN_KEY = "surfcult-token";
const VIEWS = ["guide", "channels", "system"];

let timezone;
/** A message for the status line in place of the next "Updated". */
let notice = null;

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

/** Sends `body` as JSON to `path` and returns the answer's JSON, if any. */
async function post(path, body, method = "POST") {
  const headers = { "Content-Type": "application/json" };
  if (sessionToken) headers.Authorization = `Bearer ${sessionToken}`;
  const response = await fetch(path, { method, headers, body: JSON.stringify(body) });
  if (response.status === 401) throw new NeedsToken();
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(answer.error || `${path} answered ${response.status}`);
  return answer;
}

/** What the TV is doing, or `{controllable: false}` for a server without one. */
async function loadTv() {
  return api("/tv");
}

/** A Tune button for `number`, or the On TV marker when it is on already. */
function tuneControl(tv, number) {
  if (!tv.controllable) return null;
  if (tv.channel === number) return el("span", { class: "pill on-tv" }, "On TV");
  return el("button", { class: "btn small", type: "button", "data-tune": number }, "Tune");
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
  const [guide, tv] = await Promise.all([api(`/guide?from=${from}&to=${from + GUIDE_SLOTS * SLOT_MS}`), loadTv()]);
  timezone = guide.timezone;
  return { guide, tv };
}

function guideView({ guide, tv }) {
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
    onNowList(guide, tv),
  ];
}

/** Each channel with what is on now and its progress: the phone's guide. */
function onNowList(guide, tv) {
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
        { class: tv.controllable ? "item with-tune" : "item" },
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
        tuneControl(tv, channel.number),
      );
    }),
  );
}

async function loadChannels() {
  const [{ channels }, guide, tv] = await Promise.all([api("/channels"), api("/guide"), loadTv()]);
  timezone = guide.timezone;
  return { channels, guide, tv };
}

function channelsView({ channels, guide, tv }) {
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
      tv.controllable &&
        el(
          "td",
          { class: "act" },
          el("a", { class: "btn small quiet", href: `#edit-${channel.number}` }, "Edit"),
          " ",
          tuneControl(tv, channel.number),
        ),
    );
  });
  const note = tv.controllable
    ? "From channels.toml. Edit changes the file, and the TV follows."
    : "From channels.toml. Edit that file to change them.";
  return [
    el("div", { class: "view-head" }, el("h2", {}, "Channels"), el("p", {}, note)),
    el(
      "div",
      { class: "panel table-wrap" },
      el(
        "table",
        {},
        el("thead", {}, el("tr", {}, ["CH", "NAME", "ON NOW", "NEXT", tv.controllable ? "TV" : null].filter(Boolean).map((h) => el("th", { scope: "col" }, h)))),
        el("tbody", {}, body),
      ),
    ),
  ];
}

async function loadSystem() {
  const [health, { channels }, guide, tv] = await Promise.all([api("/health"), api("/channels"), api("/guide"), loadTv()]);
  timezone = guide.timezone;
  return { health, channels, tv };
}

/** The CRT filter's three levels as buttons, the current one pressed. */
function crtControl(tv) {
  if (!tv.controllable) {
    return el("p", { class: "note" }, "This server has no TV to control. The web admin inside surfcult run can set the CRT filter.");
  }
  const levels = [["heavy", "Heavy"], ["light", "Light"], ["off", "Off"]];
  return el(
    "div",
    { class: "panel" },
    el("h3", {}, "CRT FILTER"),
    el("p", { class: "note" }, "Scanlines and glow on the TV's guide, banner, menu and watermark. Never on the program."),
    el(
      "div",
      { class: "segmented", role: "group", "aria-label": "CRT filter" },
      levels.map(([value, label]) =>
        el("button", { class: "btn small", type: "button", "data-crt": value, "aria-pressed": tv.crt === value ? "true" : "false" }, label),
      ),
    ),
  );
}

function systemView({ health, channels, tv }) {
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
    crtControl(tv),
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

// ---------- editing a channel ----------

const SOURCE_LABELS = {
  jellyfin_library: "Library",
  jellyfin_collection: "Collection",
  jellyfin_tag: "Tag",
  jellyfin_genre: "Genre",
  jellyfin_series: "Series",
};

/** The channel number of an `#edit-N` address, or null. */
function editedChannel() {
  const match = /^#edit-(\d+)$/.exec(location.hash);
  return match ? Number(match[1]) : null;
}

async function loadEdit() {
  return api(`/channels/${editedChannel()}`);
}

/** A labelled field. `hint` goes under it. */
function field(label, id, control, hint) {
  control.id = id;
  return el("div", { class: "field" }, el("label", { for: id }, label), control, hint && el("small", {}, hint));
}

/** A `<select>` of `options` ([value, label] pairs) with `value` chosen; a
 * value not among them is added, so nothing in the file is lost. */
function select(name, options, value) {
  const all = options.some(([v]) => v === value) ? options : [...options, [value, value]];
  return el("select", { name }, all.map(([v, label]) => el("option", { value: v, selected: v === value }, label)));
}

function input(name, value, attrs = {}) {
  return el("input", { name, value: value ?? "", autocomplete: "off", ...attrs });
}

function editView({ number, settings, pools, source_kinds }) {
  const poolOptions = (none) => [["", none], ...pools.map((p) => [p.name, `${p.name} (${p.kind})`])];
  const breaks = settings.breaks;
  const program = settings.source
    ? [
        field("Source", "f-kind", select("kind", source_kinds.map((k) => [k, SOURCE_LABELS[k] ?? k]), settings.source.kind)),
        field("Source name", "f-source", input("source", settings.source.name, { required: true }), "As Jellyfin names it."),
        field("Order", "f-mode", select("mode", [["sequential", "In order"], ["shuffle", "Shuffle"]], settings.mode)),
      ]
    : [el("p", { class: "note wide" }, "This channel plays blocks or weighted sources. Change those in channels.toml.")];
  const form = el(
    "form",
    { class: "panel edit", id: "edit-form", "data-channel": number },
    field("Name", "f-name", input("name", settings.name, { required: true, maxlength: 60 })),
    program,
    field("Grid", "f-grid", input("grid", settings.grid, { placeholder: "30m" }), "Programs start on this grid, such as 30m. Empty for none."),
    field("Filler", "f-filler", select("filler", poolOptions("None"), settings.filler ?? "")),
    el(
      "fieldset",
      { class: "wide" },
      el("legend", {}, el("label", {}, el("input", { type: "checkbox", name: "breaks", checked: Boolean(breaks) }), " Commercial breaks")),
      el(
        "div",
        { class: "fields" },
        field("Every", "f-every", input("every", breaks?.every ?? "11m"), "Such as 11m."),
        field("Length", "f-length", input("break_length", breaks?.length ?? "2m"), "Such as 2m."),
        field("Pool", "f-pool", select("pool", pools.map((p) => [p.name, `${p.name} (${p.kind})`]), breaks?.pool ?? pools[0]?.name ?? "")),
        field("Pad with", "f-pad", select("pad", poolOptions("Filler pool"), breaks?.pad ?? "")),
      ),
    ),
    el("p", { class: "error wide", id: "edit-error", hidden: true }),
    el("div", { class: "actions wide" }, el("button", { class: "btn", type: "submit" }, "Save to channels.toml"), el("a", { class: "btn quiet", href: "#channels" }, "Cancel")),
  );
  return [
    el(
      "div",
      { class: "view-head" },
      el("h2", {}, `Channel ${pad(number)}`),
      el("p", {}, "Saving rewrites this channel in channels.toml. What is on in the next 30 minutes stays; the TV plays the new settings after that."),
    ),
    form,
  ];
}

/** The form as the settings `PUT /channels/N` takes. */
function editedSettings(form) {
  const value = (name) => form.elements[name]?.value.trim() ?? "";
  const optional = (name) => value(name) || null;
  const settings = {
    name: value("name"),
    source: null,
    mode: null,
    grid: optional("grid"),
    breaks: null,
    filler: optional("filler"),
  };
  if (form.elements.kind) {
    settings.source = { kind: value("kind"), name: value("source") };
    settings.mode = value("mode");
  }
  if (form.elements.breaks.checked) {
    settings.breaks = { every: value("every"), length: value("break_length"), pool: value("pool"), pad: optional("pad") };
  }
  return settings;
}

document.addEventListener("submit", async (event) => {
  const form = event.target.closest("#edit-form");
  if (!form) return;
  event.preventDefault();
  const number = Number(form.dataset.channel);
  const error = document.getElementById("edit-error");
  const button = form.querySelector("button[type=submit]");
  button.disabled = true;
  try {
    await post(`/channels/${number}`, editedSettings(form), "PUT");
    notice = `Saved channel ${pad(number)} to channels.toml.`;
    location.hash = "channels";
  } catch (failure) {
    if (failure instanceof NeedsToken) return render();
    error.textContent = `Not saved: ${failure.message}`;
    error.hidden = false;
    button.disabled = false;
  }
});

document.addEventListener("change", (event) => {
  if (event.target.name !== "breaks") return;
  event.target.closest("fieldset").querySelector(".fields").hidden = !event.target.checked;
});

const ROUTES = {
  guide: [loadGuide, guideView],
  channels: [loadChannels, channelsView],
  system: [loadSystem, systemView],
  edit: [loadEdit, editView],
};

// ---------- the page ----------

function currentView() {
  if (editedChannel() !== null) return "edit";
  const name = location.hash.slice(1);
  return VIEWS.includes(name) ? name : "guide";
}

async function render() {
  const name = currentView();
  const tab = name === "edit" ? "channels" : name;
  for (const link of document.querySelectorAll(".bar nav a")) {
    if (link.dataset.view === tab) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  const view = document.getElementById("view");
  const [load, draw] = ROUTES[name];
  try {
    const data = await load();
    document.getElementById("token").hidden = true;
    view.replaceChildren(...draw(data));
    if (name === "edit") {
      const breaks = view.querySelector("input[name=breaks]");
      breaks.closest("fieldset").querySelector(".fields").hidden = !breaks.checked;
      document.getElementById("f-name").focus();
      document.getElementById("status").textContent = "Editing. The page does not refresh until you save or cancel.";
    } else {
      document.getElementById("status").textContent = notice ?? `Updated ${hms(Date.now())}. Refreshes every 30 seconds.`;
      notice = null;
    }
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

/** Sends a control request, says so, and reads the page again once the TV
 * has had a step to act. */
async function control(path, body, done) {
  const status = document.getElementById("status");
  try {
    await post(path, body);
    status.textContent = done;
    setTimeout(render, 400);
  } catch (error) {
    if (error instanceof NeedsToken) return render();
    status.textContent = `The TV did not take that: ${error.message}`;
  }
}

document.addEventListener("click", (event) => {
  const tune = event.target.closest("[data-tune]");
  if (tune) {
    const channel = Number(tune.dataset.tune);
    control("/tune", { channel }, `Asked the TV to change to channel ${pad(channel)}.`);
    return;
  }
  const crt = event.target.closest("[data-crt]");
  if (crt) control("/crt", { level: crt.dataset.crt }, `Asked the TV to set the CRT filter to ${crt.textContent}.`);
});

document.getElementById("token-form").addEventListener("submit", (event) => {
  event.preventDefault();
  storeToken(document.getElementById("token-input").value.trim());
  render();
});

window.addEventListener("hashchange", render);
render();
// An open edit form is not refreshed under the viewer's typing.
setInterval(() => currentView() === "edit" || render(), REFRESH_MS);
setInterval(tick, 1000);
