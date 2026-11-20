// kitedrop desktop UI. Data from other devices (names, file names) is only ever set with
// textContent, never as HTML; the only markup inserted as HTML is the constant icon set below.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const view = (location.hash || "#main").slice(1);
document.getElementById(`view-${view}`).hidden = false;

const ICONS = {
  macos: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M2 20h20"/></svg>',
  windows: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/></svg>',
  android: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="6" y="2" width="12" height="20" rx="2.5"/><path d="M11 18h2"/></svg>',
  ios: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="6" y="2" width="12" height="20" rx="3"/><path d="M10 5h4"/></svg>',
  file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>',
  text: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M4 6h16M4 12h16M4 18h10"/></svg>',
  clip: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="3" width="8" height="4" rx="1"/><path d="M16 5h2a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h2"/></svg>',
  attach: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12l-8.5 8.5a6 6 0 0 1-8.5-8.5L12.5 3.5a4 4 0 0 1 5.7 5.7L9.6 17.8a2 2 0 0 1-2.8-2.8L15 6.8"/></svg>',
  edit: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>',
  close: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  link: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/></svg>',
};

function icon(name) {
  const span = document.createElement("span");
  span.innerHTML = ICONS[name] ?? ICONS.file; // constant markup only
  return span.firstChild;
}

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v);
  }
  for (const c of children) if (c != null) e.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return e;
}

function size(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

// Progress of large sends (core events, ~5 per second): only the row's bar and text change.
const progressRows = new Map(); // transfer id → { wrap, bar, text, badge, size }
const progressSpeed = new Map(); // transfer id → { t, done, phase, rate }

function showProgress(p) {
  const row = progressRows.get(p.id);
  if (!row) return;
  const pct = p.total ? Math.min(100, Math.floor((p.done * 100) / p.total)) : 0;
  const now = Date.now();
  const prev = progressSpeed.get(p.id);
  let rate = null;
  if (prev && prev.phase === p.phase && now > prev.t) {
    const current = ((p.done - prev.done) * 1000) / (now - prev.t);
    rate = prev.rate == null ? current : prev.rate * 0.7 + current * 0.3;
  }
  progressSpeed.set(p.id, { t: now, done: p.done, phase: p.phase, rate });
  row.wrap.hidden = false;
  row.text.hidden = false;
  row.bar.style.width = `${pct}%`;
  row.bar.classList.toggle("sealing", p.phase === "sealing");
  row.badge.textContent = `${p.phase === "sealing" ? "Encrypting" : "Sending"} ${pct}%`;
  const left = rate > 0 && p.phase === "uploading" ? Math.ceil((p.total - p.done) / rate) : null;
  // Show the file's own size: the upload counts the envelope, which is slightly larger.
  const done = row.size && p.total ? Math.round((p.done / p.total) * row.size) : p.done;
  row.text.textContent = [
    `${size(done)} of ${size(row.size || p.total)}`,
    rate > 0 && p.phase === "uploading" ? `${size(Math.round(rate))}/s` : null,
    left != null && left > 0 ? `${left < 60 ? `${left} s` : `${Math.ceil(left / 60)} min`} left` : null,
  ].filter(Boolean).join(" · ");
}

function ago(unix) {
  if (!unix) return "";
  const s = Math.max(0, Date.now() / 1000 - unix);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return new Date(unix * 1000).toLocaleDateString();
}

const PLATFORM = { ios: "iPhone", android: "Android", macos: "Mac", windows: "Windows" };
const STATE = { queued: "Queued", sending: "Sending", delivered: "Delivered", waiting: "Waiting", failed: "Failed" };

function debounce(fn, ms) {
  let t;
  return () => { clearTimeout(t); t = setTimeout(fn, ms); };
}

function showError(target, e) {
  target.textContent = String(e);
  target.hidden = false;
}

function avatar(d) {
  const a = el("div", { class: "avatar" }, icon(d.platform));
  if (!d.self) a.append(el("span", { class: `status-dot ${d.online ? "on" : ""}` }));
  return a;
}

// ---- main window ----

let editing = null; // device id being renamed
let showAllSent = false;
let confirmRemove = null; // device id waiting for a second click

async function renderMain() {
  const s = await invoke("state");
  const mac = navigator.platform.startsWith("Mac");
  document.getElementById("shortcut").textContent = s.shortcut.replace("CommandOrControl", mac ? "⌘" : "Ctrl").replaceAll("+", mac ? "" : "+").replace("Shift", mac ? "⇧" : "Shift");
  const me = s.devices.find((d) => d.self);
  document.getElementById("me").textContent = me ? `${me.name} · ${PLATFORM[me.platform]}` : "";

  const issues = document.getElementById("issues");
  issues.replaceChildren(...[...new Set(s.issues)].map((t) => el("p", {}, t)));
  issues.hidden = s.issues.length === 0;

  const others = s.devices.filter((d) => !d.self);
  const devices = document.getElementById("devices");
  devices.replaceChildren(...others.map(deviceCard));
  if (others.length === 0) {
    devices.append(el("li", { class: "card self" },
      el("div", { class: "grow" },
        el("div", { class: "title" }, "No other devices yet"),
        el("div", { class: "sub" }, "Add your phone or another computer to start sending.")),
      el("button", { class: "btn primary", onclick: () => invoke("open_window", { label: "pair" }) }, "Add device")));
  }

  const transfers = document.getElementById("transfers");
  progressRows.clear();
  const shown = showAllSent ? s.transfers : s.transfers.slice(0, 6);
  transfers.replaceChildren(...shown.map((t) => {
    const what = t.name || (t.kind === "link" ? "Link" : "Text");
    const sub = [`to ${t.target}`, t.kind === "file" ? size(t.size) : null, ago(t.created_at), t.reason].filter(Boolean).join(" · ");
    const buttons = [];
    if (t.state === "waiting" && !t.via_outbox) buttons.push(el("button", { class: "btn", onclick: () => invoke("retry", { id: t.id }).then(renderMain) }, "Retry"));
    if (!["delivered", "failed"].includes(t.state)) buttons.push(el("button", { class: "btn icon subtle", title: "Cancel", onclick: () => invoke("cancel", { id: t.id }).then(renderMain) }, icon("close")));
    const label = t.state === "waiting" && t.via_outbox ? "Waiting for iPhone" : STATE[t.state];
    const badge = el("span", { class: `badge ${t.state}` }, label);
    const grow = el("div", { class: "grow" }, el("div", { class: "title", title: what }, what), el("div", { class: "sub", title: sub }, sub));
    if (t.kind === "file" && ["queued", "sending"].includes(t.state)) {
      const bar = el("div", { class: "bar" });
      const wrap = el("div", { class: "progress", hidden: "" }, bar);
      const text = el("div", { class: "sub ptext", hidden: "" });
      grow.append(wrap, text);
      progressRows.set(t.id, { wrap, bar, text, badge, size: t.size });
      if (t.progress) showProgress(t.progress);
    }
    return el("li", {},
      el("div", { class: "kind" }, icon(t.kind)),
      grow, badge, ...buttons);
  }));
  if (s.transfers.length > shown.length) {
    transfers.append(el("li", { class: "clickable more", onclick: () => { showAllSent = true; renderMain(); } }, `Show all (${s.transfers.length})`));
  }
  document.getElementById("no-transfers").hidden = s.transfers.length > 0;

  const received = document.getElementById("received");
  received.replaceChildren(...s.received.map((r) => {
    const isFile = r.kind === "file" && r.location;
    const row = el("li", isFile ? { class: "clickable", title: "Open", onclick: () => invoke("open_received", { path: r.location, reveal: false }).catch(alert) } : {},
      el("div", { class: "kind" }, icon(r.kind)),
      el("div", { class: "grow" },
        el("div", { class: "title" }, r.name || (r.kind === "link" ? "Link — copied" : "Text — copied")),
        el("div", { class: "sub" }, [`from ${r.from}`, r.kind === "file" ? size(r.size) : null, ago(r.at)].filter(Boolean).join(" · "))));
    if (isFile) {
      row.append(el("button", {
        class: "btn link", onclick: (e) => { e.stopPropagation(); invoke("open_received", { path: r.location, reveal: true }).catch(alert); },
      }, mac ? "Show in Finder" : "Show in folder"));
    }
    return row;
  }));
  document.getElementById("no-received").hidden = s.received.length > 0;
}

function deviceCard(d) {
  const status = d.platform === "ios" ? "Gets items when its app opens" : d.online ? "Online" : d.last_seen ? `Last seen ${ago(d.last_seen)}` : "Offline";
  let body;
  if (editing === d.id) {
    const input = el("input", { value: d.name, class: "rename" });
    const save = async () => {
      const name = input.value.trim();
      editing = null;
      if (name && name !== d.name) await invoke("rename_device", { id: d.id, name }).catch(alert);
      renderMain();
    };
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") save(); if (e.key === "Escape") { editing = null; renderMain(); } });
    setTimeout(() => input.focus(), 0);
    body = el("div", { class: "grow row" }, input, el("button", { class: "btn primary", onclick: save }, "Save"));
  } else {
    body = el("div", { class: "grow" }, el("div", { class: "title" }, d.name), el("div", { class: "sub" }, `${PLATFORM[d.platform]} · ${status}`));
  }
  const actions = editing === d.id ? null : el("div", { class: "actions" },
    el("button", { class: "btn icon", title: "Send files…", onclick: () => invoke("send_files", { targets: [d.id] }).then(renderMain) }, icon("attach")),
    el("button", { class: "btn icon", title: "Send what you copied", onclick: () => invoke("send_clipboard_to", { targets: [d.id] }).then(renderMain).catch(alert) }, icon("clip")),
    el("button", { class: "btn icon subtle", title: "Rename", onclick: () => { editing = d.id; confirmRemove = null; renderMain(); } }, icon("edit")),
    confirmRemove === d.id
      ? el("button", { class: "btn danger", onclick: async () => { confirmRemove = null; await invoke("remove_device", { id: d.id }).catch(alert); renderMain(); } }, "Remove?")
      : el("button", { class: "btn icon subtle", title: "Remove from my devices", onclick: () => { confirmRemove = d.id; renderMain(); setTimeout(() => { if (confirmRemove === d.id) { confirmRemove = null; renderMain(); } }, 4000); } }, icon("close")));
  return el("li", { class: "card" }, avatar(d), body, actions);
}

// ---- send window ----

async function renderSend() {
  const p = await invoke("pending");
  const label = document.getElementById("pending-label");
  const list = document.getElementById("targets");
  if (!p) {
    label.textContent = "Nothing to send.";
    list.replaceChildren();
    return;
  }
  label.textContent = p.kind === "files" ? p.label : `${p.kind === "link" ? "Link" : "Text"}: ${p.label}`;
  const s = await invoke("state");
  const others = s.devices.filter((d) => !d.self);
  const original = label.textContent;
  let busy = false;
  // After a click: the chosen device is marked, the others are disabled, and folders show their
  // zipping progress until the send is queued (then the window closes).
  const send = async (ids, tile) => {
    if (busy) return;
    busy = true;
    list.classList.add("busy");
    tile.classList.add("chosen");
    const sub = tile.querySelector(".sub");
    if (sub) sub.textContent = p.folders ? "Zipping, then sending here…" : "Sending here…";
    if (p.folders) {
      label.textContent = `Zipping ${p.folders === 1 ? "the folder" : "folders"}…`;
      document.getElementById("send-progress").hidden = false;
    }
    try {
      await invoke("send_pending", { targets: ids });
      window.__TAURI__.window.getCurrentWindow().close();
    } catch (e) {
      busy = false;
      list.classList.remove("busy");
      tile.classList.remove("chosen");
      label.textContent = original;
      document.getElementById("send-progress").hidden = true;
      showError(document.getElementById("send-error"), e);
      renderSend();
    }
  };
  const tile = (title, subText, ids, avatarEl) => {
    const li = el("li", { class: "tile" },
      avatarEl,
      el("div", { class: "grow" }, el("div", { class: "title" }, title), subText == null ? null : el("div", { class: "sub" }, subText)));
    li.onclick = () => send(ids, li);
    return li;
  };
  list.replaceChildren(...others.map((d) => tile(d.name,
    d.platform === "ios" ? "iPhone · gets it when its app opens" : `${PLATFORM[d.platform]} · ${d.online ? "online" : "offline — sent when it is back"}`,
    [d.id], avatar(d))));
  if (others.length > 1) {
    list.append(tile("All devices", null, others.map((d) => d.id), null));
  }
  if (others.length === 0) list.append(el("li", { class: "empty" }, "No other devices yet."));
}

// ---- pairing window ----

let reverse = false;
let timer;
let knownCount = 0;
let currentUri = "";

async function renderPair() {
  document.getElementById("tab-normal").classList.toggle("active", !reverse);
  document.getElementById("tab-reverse").classList.toggle("active", reverse);
  document.getElementById("pair-help").textContent = reverse
    ? "Add this computer to your existing devices: scan this code with kitedrop on your phone, or paste its link on a computer that already uses kitedrop."
    : "Scan this code with kitedrop on your phone, or copy the link to another computer.";
  document.getElementById("pair-error").hidden = true;
  try {
    const s = await invoke("state");
    knownCount = s.devices.length;
    const r = await invoke("invite", { reverse });
    currentUri = r.uri;
    document.getElementById("qr").src = "data:image/svg+xml;utf8," + encodeURIComponent(r.svg);
    let left = 60;
    clearInterval(timer);
    document.getElementById("countdown").textContent = left;
    timer = setInterval(() => {
      left -= 1;
      document.getElementById("countdown").textContent = Math.max(left, 0);
      if (left <= 0) {
        clearInterval(timer);
        currentUri = "";
        document.getElementById("qr").removeAttribute("src");
      }
    }, 1000);
  } catch (e) {
    showError(document.getElementById("pair-error"), e);
  }
}

async function checkPaired() {
  const s = await invoke("state");
  if (s.devices.length > knownCount) {
    const status = document.getElementById("pair-status");
    const names = s.devices.filter((d) => !d.self).map((d) => d.name).join(", ");
    status.textContent = `Paired. Your devices: ${names}`;
    status.hidden = false;
    clearInterval(timer);
    document.getElementById("qr").removeAttribute("src");
    knownCount = s.devices.length;
  }
}

// ---- wiring ----

if (view === "main") {
  document.getElementById("add-device").onclick = () => invoke("open_window", { label: "pair" });
  document.getElementById("open-downloads").onclick = () => invoke("open_downloads");
  renderMain();
  listen("kitedrop://changed", debounce(renderMain, 200));
  listen("kitedrop://progress", (e) => showProgress(e.payload));
  window.addEventListener("focus", () => invoke("refresh"));
  setInterval(renderMain, 30_000); // keep "x min ago" fresh
  // Files dropped on this window are handled natively; show a hint while dragging.
  const overlay = document.getElementById("dropping");
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((e) => {
    overlay.hidden = !(e.payload.type === "enter" || e.payload.type === "over");
  });
} else if (view === "send") {
  renderSend();
  listen("kitedrop://pending", renderSend);
  listen("kitedrop://zipping", ({ payload: z }) => {
    const pct = z.total ? Math.min(100, Math.floor((z.done * 100) / z.total)) : 0;
    document.getElementById("send-bar").style.width = `${pct}%`;
    document.getElementById("send-progress-text").textContent = `${pct}% · ${size(z.done)} of ${size(z.total)}`;
  });
} else if (view === "pair") {
  document.getElementById("tab-normal").onclick = () => { reverse = false; renderPair(); };
  document.getElementById("tab-reverse").onclick = () => { reverse = true; renderPair(); };
  document.getElementById("new-code").onclick = renderPair;
  document.getElementById("copy-link").onclick = async () => {
    if (!currentUri) await renderPair();
    await invoke("copy_text", { text: currentUri });
    const status = document.getElementById("pair-status");
    status.textContent = "Link copied. Paste it on the other computer within the minute.";
    status.hidden = false;
  };
  document.getElementById("use-link").onclick = async () => {
    const err = document.getElementById("pair-error");
    const status = document.getElementById("pair-status");
    err.hidden = true;
    status.textContent = "Pairing…";
    status.hidden = false;
    try {
      const name = await invoke("pair_link", { uri: document.getElementById("link").value });
      status.textContent = `Paired with ${name}.`;
      document.getElementById("link").value = "";
    } catch (e) {
      status.hidden = true;
      showError(err, e);
    }
  };
  renderPair();
  listen("kitedrop://changed", debounce(checkPaired, 200));
}
