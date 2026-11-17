// pingtable web UI. No build step: plain modules, plain DOM.

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

const ROW_H = 32;
const SPARK_SLOTS = 60;
const STATUS_LABEL = {
  ok: 'OK', timeout: 'Timed out', net_unreachable: 'Network unreachable', host_unreachable: 'Host unreachable',
  protocol_unreachable: 'Protocol unreachable', port_unreachable: 'Port unreachable', ttl_expired: 'TTL expired',
  packet_too_big: 'Packet too big', refused: 'Connection refused', resolve_failed: 'Name not resolved',
  bad_destination: 'Bad destination', general_failure: 'General failure', unsupported: 'Not supported here', error: 'Error',
};
// Short forms that fit the narrow "Last" column; the full label is in the tooltip.
const STATUS_SHORT = {
  timeout: 'Timed out', net_unreachable: 'No network', host_unreachable: 'Unreachable', protocol_unreachable: 'No protocol',
  port_unreachable: 'Port closed', ttl_expired: 'TTL expired', packet_too_big: 'Too big', refused: 'Refused',
  resolve_failed: 'No name', bad_destination: 'Bad address', general_failure: 'Failure', unsupported: 'Unsupported', error: 'Error',
};
const STATE_TITLE = {
  up: 'Up', down: 'Down', failing: 'Failing, not yet counted as down', recovering: 'Answering again, not yet counted as up',
  pending: 'No result yet', muted: 'Muted: not pinged, nothing collected',
};

// ------------------------------------------------------------------ token

const TOKEN_KEY = 'pingtable.token';
const url = new URL(location.href);
if (url.searchParams.get('t')) {
  try { localStorage.setItem(TOKEN_KEY, url.searchParams.get('t')); } catch {}
  var bootToken = url.searchParams.get('t');
  url.searchParams.delete('t');
  history.replaceState(null, '', url.pathname + url.search);
}
let token = bootToken || (() => { try { return localStorage.getItem(TOKEN_KEY); } catch { return null; } })();

async function api(path, { method = 'GET', body, raw = false } = {}) {
  const res = await fetch('/api/v1/' + path, {
    method,
    headers: { 'X-Pingtable-Token': token || '', ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (res.status === 401) { $('gate').hidden = false; throw new Error('unauthorized'); }
  if (!res.ok) {
    let message = res.statusText;
    try { message = (await res.json()).error || message; } catch {}
    throw new Error(message);
  }
  return raw ? res : res.json();
}

// ------------------------------------------------------------ preferences

const PREFS_KEY = 'pingtable.ui';
// Raise this when stored preferences change shape or the default columns change.
const PREFS_VERSION = 5;
const prefs = Object.assign(
  { v: 0, cols: null, grouped: true, sound: false, theme: 'light', sort: null, dir: 1, detailH: 320 },
  (() => { try { return JSON.parse(localStorage.getItem(PREFS_KEY)) || {}; } catch { return {}; } })(),
);
if (prefs.v !== PREFS_VERSION) {
  if (prefs.v < 5) { prefs.cols = null; prefs.order = null; } // new default columns and order
  if (prefs.v === 3) {
    // Version 3 had a list of themes; now there is light and dark.
    prefs.theme = prefs.system ? 'system' : prefs.themeId && prefs.themeId !== 'light' ? 'dark' : 'light';
  } else if (prefs.v < 3 && prefs.theme !== 'dark' && prefs.theme !== 'system') {
    prefs.theme = 'light';
  }
  for (const key of ['themeId', 'lightId', 'darkId', 'system', 'palette']) delete prefs[key];
  prefs.v = PREFS_VERSION;
}
const savePrefs = () => { try { localStorage.setItem(PREFS_KEY, JSON.stringify(prefs)); } catch {} };

// ------------------------------------------------------------------ look
//
// Two looks, both defined in app.css: light (the default) and dark
// ("Graphite"), switched with the button in the bar or the T key.
// `prefs.theme` is 'light' or 'dark'; an older 'system' value follows the OS.

function currentTheme() {
  if (prefs.theme === 'dark') return 'dark';
  if (prefs.theme === 'system') return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  return 'light';
}

function applyTheme() {
  const root = document.documentElement;
  const theme = currentTheme();
  root.dataset.theme = theme;
  const bg = getComputedStyle(root).getPropertyValue('--bg').trim();
  document.querySelector('meta[name="theme-color"]').content = bg;
  try { localStorage.setItem('pingtable.theme', prefs.theme); } catch {}
  // In the Windows window, ask the frame to match (title bar, and the colour shown while loading).
  try { if (window.ipc) window.ipc.postMessage(JSON.stringify({ theme, bg: bg.length === 4 ? '#' + [...bg.slice(1)].map((ch) => ch + ch).join('') : bg })); } catch {}
  chartDirty = true;
}

function setTheme(theme) {
  prefs.theme = theme;
  savePrefs();
  applyTheme();
  drawChart();
}

const toggleTheme = () => setTheme(currentTheme() === 'light' ? 'dark' : 'light');

// ------------------------------------------------------------- formatting

const none = '<span class="none">–</span>';

function fmtMs(us) {
  if (us == null) return null;
  const ms = us / 1000;
  return ms < 10 ? ms.toFixed(2) : ms < 100 ? ms.toFixed(1) : Math.round(ms).toLocaleString('en-US');
}
const msCell = (us) => fmtMs(us) ?? none;

function fmtAgo(tMs) {
  if (tMs == null) return null;
  const s = Math.max(0, Math.round((serverNow() - tMs) / 1000));
  if (s < 2) return 'now';
  if (s < 60) return s + 's ago';
  if (s < 3600) return Math.floor(s / 60) + 'm ago';
  if (s < 86400) return Math.floor(s / 3600) + 'h ago';
  return Math.floor(s / 86400) + 'd ago';
}

function fmtDur(ms) {
  const s = Math.round(ms / 1000);
  if (s < 60) return s + 's';
  if (s < 3600) return Math.floor(s / 60) + 'm ' + (s % 60) + 's';
  if (s < 86400) return Math.floor(s / 3600) + 'h ' + Math.floor((s % 3600) / 60) + 'm';
  return Math.floor(s / 86400) + 'd ' + Math.floor((s % 86400) / 3600) + 'h';
}

function fmtClock(tMs, withDate = false) {
  const d = new Date(tMs);
  const time = d.toLocaleTimeString('en-GB', { hour12: false });
  const sameDay = d.toDateString() === new Date().toDateString();
  return withDate || !sameDay ? d.toLocaleDateString('en-CA') + ' ' + time : time;
}

/** A length of time as 05:09 or 1:05:09. */
function fmtSpan(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  const two = (n) => String(n).padStart(2, '0');
  const h = Math.floor(s / 3600);
  return (h ? h + ':' : '') + two(Math.floor((s % 3600) / 60)) + ':' + two(s % 60);
}

const fmtPct = (p) => (p === 0 ? '0' : p < 10 ? p.toFixed(1) : Math.round(p)) + '%';
const fmtEvery = (s) => (s % 3600 === 0 ? s / 3600 + 'h' : s % 60 === 0 ? s / 60 + 'm' : s + 's');
const checkLabel = (p) => (p.kind === 'tcp' ? 'TCP ' + p.port : 'ICMP');
/** A row's host as it is typed in the list: with its port when the row is a port check. */
const hostWithPort = (r) => (r.probe.kind !== 'tcp' ? r.host : (r.host.includes(':') ? `[${r.host}]` : r.host) + ':' + r.probe.port);
const hostCell = (r) => (r.probe.kind !== 'tcp' ? esc(r.host) : `${esc(r.host.includes(':') ? `[${r.host}]` : r.host)}<span class="port">:${r.probe.port}</span>`);

function ipKey(ip) {
  if (!ip) return null;
  if (ip.includes(':')) return 'z' + ip;
  return ip.split('.').map((n) => n.padStart(3, '0')).join('.');
}

// ----------------------------------------------------------------- store

const store = { hosts: new Map(), seq: 0, running: false, runningSince: null, measureEndsAt: null, measureHosts: 0, active: 0, counts: { total: 0, up: 0, down: 0, pending: 0, muted: 0 }, offset: 0 };
const serverNow = () => Date.now() + store.offset;

// `selected` is the host the detail panel shows; `sel` is every highlighted host.
const view = { filter: 'all', query: '', closed: new Set(), selected: null, sel: new Set(), items: [], hostOrder: [] };

// --------------------------------------------------------------- columns

function lastCell(r) {
  if (r.last_status == null) return none;
  if (r.last_status !== 'ok') return esc(STATUS_SHORT[r.last_status] || r.last_status);
  const ms = r.last_rtt_us / 1000;
  const pct = Math.min(100, Math.max(3, ((Math.log10(Math.max(ms, 0.01)) + 1) / 4) * 100));
  return `${fmtMs(r.last_rtt_us)}<span class="bar-fill" style="width:${pct.toFixed(1)}%"></span>`;
}

function sparkHtml(r) {
  const values = r.spark;
  if (!values.length) return '';
  const W = 180, H = 20, PAD = 3;
  const step = W / SPARK_SLOTS;
  const x0 = (SPARK_SLOTS - values.length) * step + step / 2;
  let lo = Infinity, hi = -Infinity;
  for (const v of values) if (v != null) { if (v < lo) lo = v; if (v > hi) hi = v; }
  const span = hi - lo;
  const y = (v) => (span <= 0 ? H / 2 : H - PAD - ((v - lo) / span) * (H - 2 * PAD));
  let path = '', fails = '', pen = false, lastX = 0, lastY = 0, hasLast = false;
  values.forEach((v, i) => {
    const x = x0 + i * step;
    if (v == null) {
      fails += `<line class="fail" x1="${x.toFixed(1)}" x2="${x.toFixed(1)}" y1="2" y2="${H - 2}"/>`;
      pen = false; hasLast = false;
    } else {
      const yy = y(v);
      path += (pen ? 'L' : 'M') + x.toFixed(1) + ' ' + yy.toFixed(1);
      pen = true; lastX = x; lastY = yy; hasLast = true;
    }
  });
  const dot = hasLast ? `<rect class="dot" x="${(lastX - 1.5).toFixed(1)}" y="${(lastY - 1.5).toFixed(1)}" width="3" height="3"/>` : '';
  return `<svg class="spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true">${fails}<path class="line" d="${path}"/>${dot}</svg>`;
}

function lossCell(p) {
  if (p === 0) return '<span class="none">0%</span>';
  return `<span class="${p >= 20 ? 'loss-hot' : 'loss-warm'}">${fmtPct(p)}</span>`;
}

const STATE_RANK = { down: 0, failing: 1, recovering: 2, pending: 3, up: 4, muted: 5 };

const COLS = [
  { id: 'state', label: '', w: 32, cls: 'c-state', fixed: true, sort: (r) => STATE_RANK[r.state], html: (r) => `<span class="sq ${r.state}" title="${STATE_TITLE[r.state]}"></span>` },
  { id: 'host', label: 'Host', w: 180, cls: 'c-host', fixed: true, sort: (r) => r.host.toLowerCase() + (r.probe.kind === 'tcp' ? ':' + String(r.probe.port).padStart(5, '0') : ''), html: hostCell, title: hostWithPort },
  { id: 'ip', label: 'IP address', w: 138, cls: 'dim', sort: (r) => ipKey(r.ip), html: (r) => (r.ip && r.ip !== r.host ? esc(r.ip) : r.ip ? `<span class="none">${esc(r.ip)}</span>` : none) },
  { id: 'description', label: 'Description', w: 150, flex: true, sort: (r) => r.description.toLowerCase() || null, html: (r) => esc(r.description), title: (r) => r.description },
  { id: 'last', label: 'Last ms', w: 100, num: true, cls: 'c-last', sort: (r) => (r.last_status == null ? null : r.last_rtt_us ?? Infinity), html: lastCell, extra: (r) => (r.last_status && r.last_status !== 'ok' ? ' is-fail' : ''), title: (r) => (r.last_status && r.last_status !== 'ok' ? STATUS_LABEL[r.last_status] : r.last_at_ms ? 'at ' + fmtClock(r.last_at_ms) : '') },
  { id: 'trend', label: 'Trend', w: 200, sort: (r) => r.recent_loss_pct, html: sparkHtml },
  { id: 'avg', label: 'Avg ms', w: 72, num: true, sort: (r) => r.avg_rtt_us, html: (r) => msCell(r.avg_rtt_us) },
  { id: 'min', label: 'Min ms', w: 80, num: true, cls: 'dim', off: true, sort: (r) => r.min_rtt_us, html: (r) => msCell(r.min_rtt_us) },
  { id: 'max', label: 'Max ms', w: 80, num: true, cls: 'dim', off: true, sort: (r) => r.max_rtt_us, html: (r) => msCell(r.max_rtt_us) },
  { id: 'jitter', label: 'Jitter ms', w: 88, num: true, cls: 'dim', off: true, sort: (r) => r.jitter_us, html: (r) => msCell(r.jitter_us) },
  { id: 'loss', label: 'Loss', w: 64, num: true, sort: (r) => (r.sent ? r.failed_pct : null), html: (r) => (r.sent ? lossCell(r.failed_pct) : none) },
  { id: 'name', label: 'Name', w: 150, cls: 'dim', hint: 'Name found by reverse lookup', sort: (r) => r.hostname, html: (r) => esc(r.hostname ?? ''), title: (r) => r.hostname ?? '' },
  { id: 'sent', label: 'Sent', w: 66, num: true, cls: 'dim', off: true, sort: (r) => r.sent, html: (r) => r.sent.toLocaleString('en-US') },
  { id: 'streak', label: 'Fails', w: 60, num: true, hint: 'Failed pings in a row right now', sort: (r) => r.consec_failed, html: (r) => r.consec_failed || '<span class="none">0</span>' },
  { id: 'worst', label: 'Worst', w: 68, num: true, cls: 'dim', off: true, hint: 'Longest run of failed pings', sort: (r) => r.max_consec_failed, html: (r) => r.max_consec_failed || '<span class="none">0</span>' },
  { id: 'status', label: 'Status', w: 160, cls: 'dim', off: true, sort: (r) => r.last_status_label, html: (r) => esc(r.last_status_label ?? '') },
  { id: 'ttl', label: 'TTL', w: 52, num: true, cls: 'dim', off: true, sort: (r) => r.last_ttl, html: (r) => r.last_ttl ?? none },
  { id: 'check', label: 'Check', w: 88, cls: 'dim', sort: (r) => checkLabel(r.probe), html: (r) => checkLabel(r.probe) },
  { id: 'every', label: 'Every', w: 62, num: true, cls: 'dim', off: true, sort: (r) => r.interval_secs, html: (r) => fmtEvery(r.interval_secs) },
  { id: 'lastfail', label: 'Last failure', w: 100, cls: 'dim', live: true, sort: (r) => r.last_failed_at_ms, html: (r) => fmtAgo(r.last_failed_at_ms) ?? none, title: (r) => (r.last_failed_at_ms ? fmtClock(r.last_failed_at_ms, true) : '') },
  { id: 'lastok', label: 'Last answer', w: 112, cls: 'dim', off: true, live: true, sort: (r) => r.last_ok_at_ms, html: (r) => fmtAgo(r.last_ok_at_ms) ?? none, title: (r) => (r.last_ok_at_ms ? fmtClock(r.last_ok_at_ms, true) : '') },
  { id: 'tags', label: 'Tags', w: 130, off: true, sort: (r) => r.tags.join(' ') || null, html: (r) => r.tags.map((t) => `<span class="tag">${esc(t)}</span>`).join('') },
  { id: 'mac', label: 'MAC address', w: 150, cls: 'dim', off: true, sort: (r) => r.mac, html: (r) => r.mac ?? none },
  { id: 'reply', label: 'Reply from', w: 138, cls: 'dim', off: true, sort: (r) => ipKey(r.reply_ip), html: (r) => esc(r.reply_ip ?? '') },
];
const COL = Object.fromEntries(COLS.map((c) => [c.id, c]));
if (!Array.isArray(prefs.cols)) prefs.cols = COLS.filter((c) => !c.off).map((c) => c.id);
// Which columns give way first when the window is too narrow for all of them.
const KEEP_ORDER = ['state', 'host', 'last', 'trend', 'ip', 'description', 'avg', 'loss', 'name', 'streak', 'check', 'lastfail'];
let shown = [];
let fitting = false; // true while some chosen columns are left out to fit a narrow window
let colsVersion = 0;
let resizing = false;

/** Width in pixels: the one dragged by hand, or the default. */
const colWidth = (c) => Math.round((prefs.widths && prefs.widths[c.id]) || c.w);

/** Every column in the order chosen by dragging, or the default order. */
function orderedCols() {
  if (!Array.isArray(prefs.order)) return COLS;
  const place = (c) => { const i = prefs.order.indexOf(c.id); return i < 0 ? 1000 + COLS.indexOf(c) : i; };
  return [...COLS].sort((x, y) => (x.fixed ? -1 : y.fixed ? 1 : place(x) - place(y)));
}

/** The chosen columns that fit the window, dropping the least important first. */
function fittedCols() {
  const chosen = orderedCols().filter((c) => c.fixed || prefs.cols.includes(c.id));
  const room = $('grid').clientWidth;
  // When space is short the host and trend columns may shrink, so count them at their small size.
  const least = (c) => (c.id === 'host' || c.id === 'trend' ? Math.min(110, colWidth(c)) : colWidth(c));
  const fits = (cols) => cols.reduce((sum, c) => sum + least(c), 0) <= room;
  const total = (cols) => cols.reduce((sum, c) => sum + colWidth(c), 0);
  if (!room || total(chosen) <= room) { fitting = false; return chosen; }
  const rank = (c) => { const i = KEEP_ORDER.indexOf(c.id); return i < 0 ? 100 : i; };
  const kept = new Set(chosen);
  for (const c of [...chosen].sort((x, y) => rank(y) - rank(x))) {
    if (fits([...kept]) || kept.size <= 3) break;
    kept.delete(c);
  }
  fitting = true;
  return chosen.filter((c) => kept.has(c));
}

function applyColumnWidths() {
  const lastId = shown[shown.length - 1].id;
  const template = shown
    .map((c) => {
      const w = colWidth(c);
      // In a narrow window the host column takes the slack and may shrink.
      if (fitting) return c.id === 'host' ? 'minmax(90px, 1fr)' : c.id === 'trend' ? `minmax(60px, ${w}px)` : w + 'px';
      if (c.id === lastId) return `minmax(${w}px, 1fr)`; // the last column takes what is left
      // The description may grow to a comfortable reading width unless it was sized by hand.
      if (c.flex && !(prefs.widths && prefs.widths[c.id])) return `minmax(${w}px, ${w + 230}px)`;
      return w + 'px';
    })
    .join(' ');
  const grid = $('grid');
  grid.style.setProperty('--cols', template);
  grid.style.setProperty('--min-w', fitting ? '0px' : shown.reduce((sum, c) => sum + colWidth(c), 0) + 'px');
}

function layoutColumns() {
  shown = fittedCols();
  colsVersion++;
  applyColumnWidths();
  $('thead').innerHTML = shown
    .map((c) => {
      const sorted = prefs.sort === c.id;
      const grip = c.id === 'state' ? '' : `<span class="grip" data-grip="${c.id}" title="Drag to resize. Double-click to reset."></span>`;
      const hint = c.hint || 'Sort by ' + (c.label || 'state').toLowerCase();
      return `<button class="th${c.num ? ' is-num' : ''}${sorted ? ' is-sorted ' + (prefs.dir > 0 ? 'asc' : 'desc') : ''}" data-col="${c.id}" role="columnheader"${c.fixed ? '' : ' draggable="true"'} title="${esc(hint)}${c.fixed ? '' : '. Drag to move the column.'}">${esc(c.label)}<span class="arrow"></span>${grip}</button>`;
    })
    .join('');
  rowCache.forEach((entry) => entry.el.remove());
  rowCache.clear();
}

/** Re-fit the columns after the window changed size; redraw only if the set changed. */
function refitColumns() {
  const before = shown.map((c) => c.id).join();
  const wasFitting = fitting;
  const next = fittedCols();
  if (next.map((c) => c.id).join() !== before || wasFitting !== fitting) { layoutColumns(); schedule(); }
}

// ------------------------------------------------------------------ view

function matches(r) {
  if (view.filter === 'up' && !(r.state === 'up' || r.state === 'recovering')) return false;
  if (view.filter === 'down' && !(r.state === 'down' || r.state === 'failing')) return false;
  if (view.filter === 'muted' && r.state !== 'muted') return false;
  if (!view.query) return true;
  const hay = [r.host, r.ip, r.description, r.group, r.tags.join(' '), r.hostname, r.mac, r.last_status_label, checkLabel(r.probe), r.state]
    .filter(Boolean).join(' ').toLowerCase();
  return view.query.split(/\s+/).every((term) => hay.includes(term));
}

function compareRows(a, b) {
  const col = COL[prefs.sort];
  if (col) {
    const x = col.sort(a), y = col.sort(b);
    const xn = x == null || x === '', yn = y == null || y === '';
    if (xn !== yn) return xn ? 1 : -1; // blanks always last
    if (!xn && x !== y) return (x < y ? -1 : 1) * prefs.dir;
  }
  return a.order - b.order;
}

function buildView() {
  const all = [...store.hosts.values()];
  const rows = all.filter(matches).sort(compareRows);
  const anyGroup = all.some((r) => r.group);
  const items = [];
  if (prefs.grouped && anyGroup) {
    const groups = new Map();
    for (const r of [...all].sort((a, b) => a.order - b.order)) {
      const name = r.group || '';
      if (!groups.has(name)) groups.set(name, { name, rows: [], total: 0, up: 0, down: 0 });
      const g = groups.get(name);
      g.total++;
      if (r.state === 'up' || r.state === 'recovering') g.up++;
      if (r.state === 'down' || r.state === 'failing') g.down++;
    }
    for (const r of rows) groups.get(r.group || '').rows.push(r);
    for (const g of groups.values()) {
      if (!g.rows.length && (view.query || view.filter !== 'all')) continue;
      const closed = view.closed.has(g.name);
      items.push({ type: 'group', key: 'g:' + g.name, group: g, closed });
      if (!closed) for (const r of g.rows) items.push({ type: 'host', key: 'h:' + r.id, row: r });
    }
  } else {
    for (const r of rows) items.push({ type: 'host', key: 'h:' + r.id, row: r });
  }
  view.items = items;
  view.hostOrder = items.filter((i) => i.type === 'host').map((i) => i.row.id);
  $('tbody').style.height = items.length * ROW_H + 'px';
  $('empty').hidden = store.hosts.size > 0 || !loadedOnce;
}

const rowCache = new Map(); // key -> { el, stamp, sent }

function rowStamp(item, liveBucket) {
  if (item.type === 'group') {
    const g = item.group;
    return `${g.total}|${g.up}|${g.down}|${item.closed}`;
  }
  const r = item.row;
  return `${r.sent}|${r.state}|${r.last_at_ms}|${r.mac}|${r.hostname}|${r.ip}|${r.description}|${r.interval_secs}|${view.sel.has(r.id)}|${colsVersion}|${liveBucket}`;
}

function hostRowHtml(r) {
  let html = '';
  for (const c of shown) {
    const title = c.title ? ` title="${esc(c.title(r))}"` : '';
    html += `<div class="td ${c.cls || ''}${c.num ? ' is-num' : ''}${c.extra ? c.extra(r) : ''}"${title}>${c.html(r)}</div>`;
  }
  return html;
}

function groupRowHtml(item) {
  const g = item.group;
  const down = g.down ? ` · <b class="dn">${g.down} down</b>` : '';
  return `<span class="caret"></span><span class="gr-name">${esc(g.name || 'Ungrouped')}</span><span class="gr-count"><b>${g.total}</b> ${g.total === 1 ? 'host' : 'hosts'} · ${g.up} up${down}</span>`;
}

function renderWindow() {
  const grid = $('grid');
  const tbody = $('tbody');
  const first = Math.max(0, Math.floor(grid.scrollTop / ROW_H) - 8);
  const last = Math.min(view.items.length, Math.ceil((grid.scrollTop + grid.clientHeight) / ROW_H) + 8);
  const liveBucket = shown.some((c) => c.live) ? Math.floor(Date.now() / 1000) : 0;
  const keep = new Set();
  for (let i = first; i < last; i++) {
    const item = view.items[i];
    keep.add(item.key);
    let entry = rowCache.get(item.key);
    if (!entry) {
      const el = document.createElement('div');
      el.dataset.key = item.key;
      entry = { el, stamp: null, bad: null, y: -1 };
      rowCache.set(item.key, entry);
      tbody.appendChild(el);
    }
    const stamp = rowStamp(item, liveBucket);
    if (stamp !== entry.stamp) {
      const el = entry.el;
      if (item.type === 'group') {
        el.className = 'gr' + (item.closed ? ' is-closed' : '');
        el.innerHTML = groupRowHtml(item);
      } else {
        const r = item.row;
        // Glow for a few seconds when a host goes down or comes back, not on every ping.
        const bad = r.state === 'down' || r.state === 'failing';
        const changed = entry.bad != null && entry.bad !== bad && r.state !== 'muted';
        if (changed) entry.glow = { cls: bad ? 'went-down' : 'came-up', until: Date.now() + 4000 };
        // Rows are redrawn on every ping; keeping the class lets the glow run its full length.
        const glow = entry.glow && Date.now() < entry.glow.until ? ' ' + entry.glow.cls : '';
        el.className = `tr is-${r.state}${r.enabled ? '' : ' is-off'}${view.sel.has(r.id) ? ' is-selected' : ''}${glow}`;
        el.setAttribute('role', 'row');
        el.innerHTML = hostRowHtml(r);
        if (changed) {
          // Restart the animation in case the other glow was still running.
          el.style.animation = 'none';
          void el.offsetWidth;
          el.style.animation = '';
        }
        entry.bad = r.state === 'pending' || r.state === 'muted' ? null : bad;
      }
      entry.stamp = stamp;
    }
    if (entry.y !== i) {
      entry.el.style.transform = `translateY(${i * ROW_H}px)`;
      entry.y = i;
    }
  }
  for (const [key, entry] of rowCache) {
    if (!keep.has(key)) {
      entry.el.remove();
      rowCache.delete(key);
    }
  }
}

function renderBar() {
  const c = store.counts;
  $('n-total').textContent = c.total;
  $('n-up').textContent = c.up;
  $('n-down').textContent = c.down;
  $('n-muted').textContent = c.muted;
  $('tally-muted').hidden = c.muted === 0 && view.filter !== 'muted';
  document.querySelector('.tally-down').classList.toggle('has-down', c.down > 0);
  document.querySelectorAll('.tally').forEach((el) => el.classList.toggle('is-on', el.dataset.filter === view.filter));
  const measuring = store.measureEndsAt != null;
  const runState = $('run-state');
  runState.classList.toggle('is-running', store.running);
  runState.classList.toggle('is-measuring', measuring);
  runState.title = measuring ? 'A measurement is running. Click to see it.' : store.running ? 'Time since pinging started' : '';
  $('run-text').textContent = measuring ? 'MEASURING' : store.running ? 'PINGING' : 'STOPPED';
  let sub = '';
  if (measuring) {
    sub = `${fmtSpan(store.measureEndsAt - serverNow())} left · ${store.measureHosts} ${store.measureHosts === 1 ? 'host' : 'hosts'}`;
  } else if (store.running && store.runningSince != null) {
    sub = fmtSpan(serverNow() - store.runningSince);
  }
  $('run-sub').textContent = sub;
  const toggle = $('btn-toggle');
  const busy = store.running || measuring;
  toggle.textContent = busy ? 'STOP' : 'START';
  toggle.classList.toggle('is-stop', busy);
  toggle.title = measuring ? 'Stop the measurement and all pinging (S)' : 'Start or stop the regular pinging (S)';
  toggle.disabled = c.total === 0;
  document.title = (c.down ? `(${c.down}) ` : '') + 'pingtable';
}

let appInfo = null;

/** Read the settings again (after anything changed them) for the footer. */
async function refreshConfig() {
  try { loaded = await api('config'); } catch {}
  schedule(false);
}

/** The strip along the bottom: where pings come from, how often, and what is going on. */
function renderFoot() {
  const settings = loaded.settings || {};
  if (source) {
    const chosen = settings.source_v4;
    const addr = chosen || source.default_v4 || source.default_v6;
    const iface = source.interfaces.find((i) => i.addr === addr);
    $('foot-src').innerHTML = `<i>From</i><b>${esc(source.name)}</b>` + (addr ? `<span>${esc(addr)}${iface ? ' · ' + esc(iface.name) : ''}${chosen ? '' : ' · automatic'}</span>` : '');
  }
  if (settings.interval_secs) {
    let perSecond = 0;
    for (const r of store.hosts.values()) if (r.enabled) perSecond += 1 / r.interval_secs;
    const rate = !store.running ? '' : perSecond >= 10 ? ` · about ${Math.round(perSecond)} pings a second` : perSecond >= 1 ? ` · about ${perSecond.toFixed(1)} pings a second` : '';
    $('foot-rate').innerHTML = `<i>Every</i><b>${fmtEvery(settings.interval_secs)}</b><i>Timeout</i><b>${settings.timeout_ms} ms</b><span>${rate}</span>`;
  }
  $('foot-flight').innerHTML = `<b>${store.active}</b> in flight`;
  $('foot-sel').hidden = view.sel.size < 2;
  $('foot-sel').innerHTML = `<b>${view.sel.size}</b> selected`;
  const filtered = view.filter !== 'all' || view.query;
  $('foot-shown').hidden = !filtered;
  $('foot-shown').innerHTML = `<b>${view.hostOrder.length}</b> of ${store.hosts.size} shown`;
  if (appInfo) $('foot-ver').textContent = 'pingtable ' + appInfo.version;
}

let frame = 0;
let viewDirty = true;
function schedule(rebuild = true) {
  if (rebuild) viewDirty = true;
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    if (viewDirty) { buildView(); viewDirty = false; }
    renderWindow();
    renderBar();
    renderFoot();
  });
}

// ------------------------------------------------------------------ poll

let loadedOnce = false;
let failures = 0;

function applySnapshot(snap) {
  if (snap.full) {
    store.hosts.clear();
    view.closed.forEach((name) => { if (!snap.hosts.some((h) => (h.group || '') === name)) view.closed.delete(name); });
  }
  for (const row of snap.hosts) store.hosts.set(row.id, row);
  store.seq = snap.seq;
  store.running = snap.running;
  store.runningSince = snap.running_since_ms;
  store.measureEndsAt = snap.measure_ends_at_ms;
  store.measureHosts = snap.measure_hosts;
  store.active = snap.active;
  store.counts = snap.counts;
  store.offset = snap.now_ms - Date.now();
  if (view.selected != null && !store.hosts.has(view.selected)) { view.selected = null; closeDetail(); }
  for (const id of view.sel) if (!store.hosts.has(id)) view.sel.delete(id);
  for (const event of snap.events) announce(event);
  if (view.selected != null && snap.hosts.some((h) => h.id === view.selected)) refreshDetail();
  loadedOnce = true;
  schedule();
}

async function poll() {
  for (;;) {
    try {
      const wait = store.seq ? '&wait_ms=20000' : '';
      const snap = await api(`state?since=${store.seq}${wait}`);
      if (failures) { $('banner').hidden = true; failures = 0; }
      applySnapshot(snap);
    } catch (err) {
      if (err.message === 'unauthorized') return;
      failures++;
      if (failures >= 2) {
        $('banner').textContent = 'Lost contact with pingtable. Retrying…';
        $('banner').hidden = false;
      }
      store.seq = 0; // ask for the whole table when it is back
      await new Promise((r) => setTimeout(r, Math.min(5000, 500 * failures)));
    }
  }
}

// ---------------------------------------------------------------- toasts

let audio = null;
function beep(kind) {
  if (!prefs.sound) return;
  try {
    audio = audio || new AudioContext();
    const tones = kind === 'down' ? [440, 311] : [523, 784];
    tones.forEach((hz, i) => {
      const osc = audio.createOscillator();
      const gain = audio.createGain();
      osc.type = 'square';
      osc.frequency.value = hz;
      const t = audio.currentTime + i * 0.14;
      gain.gain.setValueAtTime(0.0001, t);
      gain.gain.exponentialRampToValueAtTime(0.08, t + 0.01);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.13);
      osc.connect(gain).connect(audio.destination);
      osc.start(t);
      osc.stop(t + 0.14);
    });
  } catch {}
}

function toast(kind, strong, text) {
  const box = $('toasts');
  const el = document.createElement('div');
  el.className = 'toast ' + kind;
  el.innerHTML = `<b>${esc(strong)}</b><span>${esc(text)}</span>`;
  box.appendChild(el);
  while (box.children.length > 4) box.firstChild.remove();
  setTimeout(() => { el.classList.add('is-leaving'); setTimeout(() => el.remove(), 220); }, 6000);
}

function announce(event) {
  if (event.kind === 'down') toast('down', 'DOWN', `${event.host} · ${STATUS_LABEL[event.status] || event.status}`);
  else toast('up', 'UP', `${event.host} is answering again`);
  beep(event.kind);
}

// ---------------------------------------------------------------- detail

let detail = null;
let detailTimer = 0;
let detailBusy = false;
let chartDirty = true;
let hoverIndex = -1;

function select(id, open = false) {
  if (id === view.selected && view.sel.size === 1 && !open) return;
  view.selected = id;
  view.sel = new Set([id]);
  schedule(false);
  if (open || !$('detail').hidden) openDetail();
  scrollToSelected();
}

function scrollToSelected() {
  const index = view.items.findIndex((i) => i.type === 'host' && i.row.id === view.selected);
  if (index < 0) return;
  const grid = $('grid');
  const top = index * ROW_H;
  const visibleTop = grid.scrollTop;
  const visibleBottom = grid.scrollTop + grid.clientHeight - $('thead').offsetHeight - ROW_H;
  if (top < visibleTop) grid.scrollTop = top;
  else if (top > visibleBottom) grid.scrollTop = top - grid.clientHeight + $('thead').offsetHeight + ROW_H;
}

function openDetail() {
  if (view.selected == null) return;
  const panel = $('detail');
  panel.hidden = false;
  panel.style.setProperty('--detail-h', prefs.detailH + 'px');
  detail = null;
  hoverIndex = -1;
  refreshDetail();
  clearInterval(detailTimer);
  detailTimer = setInterval(() => { if (!document.hidden) refreshDetail(); }, 5000);
  schedule(false);
}

function closeDetail() {
  $('detail').hidden = true;
  clearInterval(detailTimer);
  detail = null;
  schedule(false);
}

async function refreshDetail() {
  if (view.selected == null || $('detail').hidden || detailBusy) return;
  detailBusy = true;
  const id = view.selected;
  try {
    const data = await api('hosts/' + id);
    if (id === view.selected) { detail = data; chartDirty = true; renderDetail(); }
  } catch {} finally { detailBusy = false; }
}

function tile(label, value, unit = '', bad = false, words = false) {
  const empty = value === '–';
  return `<div class="tile"><div class="tile-l">${label}</div><div class="tile-v${bad ? ' bad' : ''}${empty ? ' none' : ''}${words ? ' is-words' : ''}">${value}${unit && !empty ? `<small>${unit}</small>` : ''}</div></div>`;
}

function renderDetail() {
  if (!detail) return;
  const r = detail.row;
  $('d-sq').className = 'sq ' + r.state;
  $('d-title').textContent = r.host;
  const stateEl = $('d-state');
  stateEl.className = 'd-state ' + r.state;
  stateEl.textContent = r.state.toUpperCase();
  const meta = [];
  if (r.description) meta.push(esc(r.description));
  if (r.ip && r.ip !== r.host) meta.push(`<b>${esc(r.ip)}</b>`);
  meta.push(checkLabel(r.probe), 'every ' + fmtEvery(r.interval_secs), 'timeout ' + r.timeout_ms + ' ms');
  if (r.packet) meta.push('packet: ' + esc(r.packet));
  if (r.hostname) meta.push(esc(r.hostname));
  if (r.mac) meta.push(r.mac);
  if (r.tags.length) meta.push(r.tags.map((t) => `<span class="tag">${esc(t)}</span>`).join(''));
  $('d-meta').innerHTML = meta.join(' · ');
  $('d-toggle').textContent = r.enabled ? 'MUTE' : 'UNMUTE';

  const dash = '–';
  const w = Object.fromEntries(detail.windows.map((x) => [x.label, x]));
  const win = (label) => (w[label] && w[label].sent ? fmtPct(w[label].loss_pct) : dash);
  const lastBad = r.last_status && r.last_status !== 'ok';
  $('d-tiles').innerHTML =
    tile('Last', lastBad ? esc(STATUS_SHORT[r.last_status] || r.last_status) : fmtMs(r.last_rtt_us) ?? dash, lastBad ? '' : 'ms', lastBad, lastBad) +
    tile('Average', fmtMs(r.avg_rtt_us) ?? dash, 'ms') +
    tile('Median', fmtMs(detail.p50_rtt_us) ?? dash, 'ms') +
    tile('95th pct', fmtMs(detail.p95_rtt_us) ?? dash, 'ms') +
    tile('Fastest', fmtMs(r.min_rtt_us) ?? dash, 'ms') +
    tile('Slowest', fmtMs(r.max_rtt_us) ?? dash, 'ms') +
    tile('Jitter', fmtMs(r.jitter_us) ?? dash, 'ms') +
    tile('Loss, all', r.sent ? fmtPct(r.failed_pct) : dash, '', r.failed_pct >= 20) +
    tile('Loss, 5 min', win('5m'), '', w['5m'] && w['5m'].loss_pct >= 20) +
    tile('Loss, 1 hour', win('1h'), '', w['1h'] && w['1h'].loss_pct >= 20) +
    tile('Sent', r.sent.toLocaleString('en-US')) +
    tile('Worst run', r.max_consec_failed, r.max_consec_failed === 1 ? 'fail' : 'fails');

  const runs = [...detail.runs].reverse();
  $('d-runs').innerHTML = runs.length
    ? runs.map((run, i) => {
        const ok = run.status === 'ok';
        const ongoing = i === 0;
        const span = ongoing ? Math.max(run.end_ms, serverNow()) - run.start_ms : run.end_ms - run.start_ms;
        return `<div class="run-row${ok ? '' : ' is-bad'}"><span class="sq ${ok ? 'up' : 'down'}"></span>` +
          `<span class="what"><b>${esc(STATUS_LABEL[run.status] || run.status)}</b><span class="when">from ${fmtClock(run.start_ms)}</span></span>` +
          `<span class="dur">${fmtDur(span)}${ongoing ? ' so far' : ''}<small>${run.count} ${run.count === 1 ? 'ping' : 'pings'}</small></span></div>`;
      }).join('')
    : '<div class="run-row"><span></span><span class="what none">No pings yet</span><span></span></div>';

  const h = detail.history;
  $('d-chart-range').textContent = h.length ? `${h.length} pings · ${fmtClock(h[0].t_ms)} to ${fmtClock(h[h.length - 1].t_ms)}` : '';
  drawChart();
}

function niceCeil(v) {
  if (v <= 0) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) if (v <= m * p) return m * p;
  return 10 * p;
}

let chartGeom = null;
function drawChart() {
  const canvas = $('d-chart');
  if (!detail || canvas.clientWidth === 0) return;
  const css = getComputedStyle(document.documentElement);
  const color = (name) => css.getPropertyValue(name).trim();
  const dpr = window.devicePixelRatio || 1;
  const W = canvas.clientWidth, H = canvas.clientHeight;
  if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
  }
  const g = canvas.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, W, H);

  const h = detail.history;
  const padL = 46, padR = 14, padT = 6, padB = 20;
  const plotW = W - padL - padR, plotH = H - padT - padB;
  g.font = `11px ${css.getPropertyValue('--mono')}`;
  g.textBaseline = 'middle';

  let maxUs = 0;
  for (const s of h) if (s.rtt_us != null && s.rtt_us > maxUs) maxUs = s.rtt_us;
  const top = niceCeil((maxUs / 1000) * 1.1 || 1);
  const t0 = h.length ? h[0].t_ms : 0;
  const t1 = h.length ? h[h.length - 1].t_ms : 1;
  const x = (t) => padL + (t1 === t0 ? plotW / 2 : ((t - t0) / (t1 - t0)) * plotW);
  const y = (ms) => padT + plotH - (ms / top) * plotH;

  // grid
  const tickStep = top / 4;
  const tickDecimals = Number.isInteger(tickStep) ? 0 : tickStep >= 1 ? 1 : 2;
  g.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const ms = (top / 4) * i;
    const yy = Math.round(y(ms)) + 0.5;
    g.strokeStyle = i === 0 ? color('--faint') : color('--rule');
    g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
    g.fillStyle = color('--dim');
    g.textAlign = 'right';
    g.fillText(ms.toFixed(tickDecimals), padL - 8, yy);
  }
  g.textAlign = 'left';
  if (h.length > 1) {
    g.textBaseline = 'alphabetic';
    g.fillStyle = color('--dim');
    g.fillText(fmtClock(t0), padL, H - 5);
    g.textAlign = 'right';
    g.fillText(fmtClock(t1), W - padR, H - 5);
  }

  // failures
  g.strokeStyle = color('--down');
  g.globalAlpha = h.length > 400 ? 0.35 : 0.55;
  g.lineWidth = Math.max(1, Math.min(4, plotW / Math.max(h.length, 1) - 1));
  g.beginPath();
  for (const s of h) if (s.rtt_us == null) { const xx = Math.round(x(s.t_ms)) + 0.5; g.moveTo(xx, padT); g.lineTo(xx, padT + plotH); }
  g.stroke();
  g.globalAlpha = 1;

  // line, broken at failures
  g.strokeStyle = color('--accent');
  g.lineWidth = 1.5;
  g.lineJoin = 'round';
  g.beginPath();
  let pen = false;
  for (const s of h) {
    if (s.rtt_us == null) { pen = false; continue; }
    const xx = x(s.t_ms), yy = y(s.rtt_us / 1000);
    if (pen) g.lineTo(xx, yy); else g.moveTo(xx, yy);
    pen = true;
  }
  g.stroke();
  if (h.length <= 120) {
    g.fillStyle = color('--accent');
    for (const s of h) if (s.rtt_us != null) g.fillRect(x(s.t_ms) - 1.5, y(s.rtt_us / 1000) - 1.5, 3, 3);
  }

  chartGeom = { x, y, padL, padR, padT, plotH, W };
  if (hoverIndex >= 0 && hoverIndex < h.length) {
    const s = h[hoverIndex];
    const xx = Math.round(x(s.t_ms)) + 0.5;
    g.strokeStyle = color('--text');
    g.lineWidth = 1;
    g.beginPath(); g.moveTo(xx, padT); g.lineTo(xx, padT + plotH); g.stroke();
    if (s.rtt_us != null) { g.fillStyle = color('--text'); g.fillRect(xx - 3, y(s.rtt_us / 1000) - 3, 6, 6); }
  }
  chartDirty = false;
}

function chartHover(event) {
  if (!detail || !chartGeom || !detail.history.length) return;
  const rect = $('d-chart').getBoundingClientRect();
  const px = event.clientX - rect.left;
  const h = detail.history;
  let best = 0, bestDist = Infinity;
  for (let i = 0; i < h.length; i++) {
    const d = Math.abs(chartGeom.x(h[i].t_ms) - px);
    if (d < bestDist) { bestDist = d; best = i; }
  }
  hoverIndex = best;
  const s = h[best];
  const tip = $('d-tip');
  tip.hidden = false;
  tip.textContent = `${fmtClock(s.t_ms)} · ${s.rtt_us != null ? fmtMs(s.rtt_us) + ' ms' : STATUS_LABEL[s.status] || s.status}${s.ttl != null ? ' · TTL ' + s.ttl : ''}`;
  const xx = chartGeom.x(s.t_ms);
  const tipW = tip.offsetWidth;
  tip.style.left = Math.max(4, Math.min(chartGeom.W - tipW - 4, xx - tipW / 2)) + 'px';
  drawChart();
}

// ---------------------------------------------------------------- dialog

// Fields of the Settings window, and the three that belong to the host list.
const settingFields = [
  ['f-interval', 'interval_secs', 'int'], ['f-timeout', 'timeout_ms', 'int'], ['f-concurrent', 'max_concurrent', 'int'],
  ['f-payload', 'payload_size', 'int'], ['f-ttl', 'ttl', 'optint'], ['f-history', 'history_per_host', 'int'],
  ['f-src4', 'source_v4', 'opttext'], ['f-src6', 'source_v6', 'opttext'],
  ['f-repeat', 'repeat', 'bool'], ['f-resolve', 'resolve_every_ping', 'bool'], ['f-rdns', 'reverse_dns', 'bool'],
  ['f-ipv6', 'prefer_ipv6', 'bool'], ['f-mac', 'read_mac', 'bool'], ['f-df', 'dont_fragment', 'bool'],
  ['f-failn', 'fail_trigger', 'int'], ['f-upn', 'recover_trigger', 'int'],
  ['f-cmddown', 'on_down_command', 'text'], ['f-cmdup', 'on_up_command', 'text'],
];
const listFields = [['f-described', 'description_format', 'bool'], ['f-skipfirst', 'cidr_skip_first', 'bool'], ['f-skiplast', 'cidr_skip_last', 'bool']];
// What the server holds, as last read. Each window changes its own part and sends the rest back unchanged.
let loaded = { settings: {}, hosts_text: '', autostart: true, groups: [] };

function fillFields(fields, settings) {
  for (const [id, key, kind] of fields) {
    const el = $(id), v = settings[key];
    if (kind === 'bool') el.checked = !!v; else el.value = v ?? '';
  }
}

function readFields(fields, into) {
  for (const [id, key, kind] of fields) {
    const el = $(id);
    if (kind === 'bool') into[key] = el.checked;
    else if (kind === 'int') { const n = parseInt(el.value, 10); if (Number.isFinite(n)) into[key] = n; }
    else if (kind === 'optint') { const n = parseInt(el.value, 10); into[key] = Number.isFinite(n) ? n : null; }
    else if (kind === 'opttext') into[key] = el.value.trim() || null;
    else into[key] = el.value;
  }
  return into;
}

/** The whole host list as text, for bulk edits. `appended` is added at the end (an import). */
async function openHosts(appended) {
  setMsg('');
  try {
    loaded = await api('config');
    fillFields(listFields, loaded.settings);
    $('f-hosts').value = loaded.hosts_text;
  } catch (err) { setMsg(err.message, true); }
  if (!$('dlg-hosts').open) $('dlg-hosts').showModal();
  if (appended) appendToEditor(appended.text, appended.note);
  preview();
  $('f-hosts').focus();
}

function appendToEditor(text, note) {
  const box = $('f-hosts');
  box.focus();
  box.setSelectionRange(box.value.length, box.value.length);
  const glue = box.value && !box.value.endsWith('\n') ? '\n\n' : box.value ? '\n' : '';
  if (!document.execCommand('insertText', false, glue + text + '\n')) box.value += glue + text + '\n';
  $('f-described').checked = true;
  preview();
  setMsg(note);
}

async function openSettings(tab = 'pinging') {
  $('settings-msg').textContent = '';
  try {
    loaded = await api('config');
    const src = (await loadSource()) || { interfaces: [] };
    for (const [id, key, v6] of [['f-src4', 'source_v4', false], ['f-src6', 'source_v6', true]]) {
      const options = src.interfaces.filter((i) => i.addr.includes(':') === v6);
      const current = loaded.settings[key];
      const auto = v6 ? src.default_v6 : src.default_v4;
      $(id).innerHTML = `<option value="">Automatic${auto ? ` (now ${esc(auto)})` : ''}</option>` +
        options.map((i) => `<option value="${esc(i.addr)}">${esc(i.addr)} · ${esc(i.name)}</option>`).join('') +
        (current && !options.some((i) => i.addr === current) ? `<option value="${esc(current)}">${esc(current)} · not present now</option>` : '');
    }
    fillFields(settingFields, loaded.settings);
    $('f-autostart').checked = loaded.autostart;
    $('cfg-path').textContent = 'Settings are saved in ' + loaded.config_path;
  } catch (err) { $('settings-msg').textContent = err.message; }
  showTab(tab);
  $('dlg-settings').showModal();
}

function showTab(name) {
  document.querySelectorAll('#dlg-settings .tab').forEach((el) => el.classList.toggle('is-on', el.dataset.tab === name));
  document.querySelectorAll('#dlg-settings .tabpane').forEach((el) => el.classList.toggle('is-on', el.dataset.pane === name));
}

async function applySettings() {
  if (!$('settings-form').reportValidity()) return;
  $('settings-msg').textContent = 'Applying…';
  try {
    const out = await api('config', {
      method: 'PUT',
      body: { settings: readFields(settingFields, { ...loaded.settings }), hosts_text: loaded.hosts_text, keep_stats: true, autostart: $('f-autostart').checked },
    });
    if (out.save_error) { $('settings-msg').textContent = 'Applied, but the settings file could not be written: ' + out.save_error; return; }
    $('dlg-settings').close();
    await loadSource();
    refreshConfig();
  } catch (err) { $('settings-msg').textContent = err.message; }
}

function setMsg(text, bad = false) {
  const el = $('dlg-msg');
  el.textContent = text;
  el.classList.toggle('bad', bad);
}

let previewTimer = 0;
let previewSeq = 0;
function preview() {
  clearTimeout(previewTimer);
  previewTimer = setTimeout(async () => {
    const mine = ++previewSeq;
    try {
      const out = await api('parse', {
        method: 'POST',
        body: { text: $('f-hosts').value, description_format: $('f-described').checked, cidr_skip_first: $('f-skipfirst').checked, cidr_skip_last: $('f-skiplast').checked },
      });
      if (mine !== previewSeq) return;
      const parts = [`<b>${out.count.toLocaleString('en-US')}</b> ${out.count === 1 ? 'host' : 'hosts'}`];
      if (out.groups) parts.push(`${out.groups} ${out.groups === 1 ? 'group' : 'groups'}`);
      if (out.tcp) parts.push(`${out.tcp} TCP ${out.tcp === 1 ? 'check' : 'checks'}`);
      $('parse-line').innerHTML = parts.join(' · ');
      $('parse-warn').innerHTML = out.warnings.map((w) => `<li>${esc(w)}</li>`).join('');
    } catch {}
  }, 220);
}

async function applyHosts(keepStats) {
  setMsg('Applying…');
  try {
    const out = await api('config', {
      method: 'PUT',
      body: { settings: readFields(listFields, { ...loaded.settings }), hosts_text: $('f-hosts').value, keep_stats: keepStats, autostart: loaded.autostart },
    });
    if (out.save_error) { setMsg('Applied, but the settings file could not be written: ' + out.save_error, true); return; }
    $('dlg-hosts').close();
    refreshConfig();
    if (out.hosts > 0 && !store.running && loaded.autostart) await api('control', { method: 'POST', body: { action: 'start' } });
    if (out.warnings.length) toast('up', 'NOTE', `${out.warnings.length} ${out.warnings.length === 1 ? 'entry was' : 'entries were'} skipped`);
  } catch (err) { setMsg(err.message, true); }
}

/** Read a CSV or text file and put its hosts in the host list editor for review. */
async function importFile(file) {
  if (!file) return;
  try {
    const text = await file.text();
    const out = await api('import', { method: 'POST', body: { text } });
    if (!out.hosts) { toast('down', 'IMPORT', `No hosts found in ${file.name}`); return; }
    const note = `${out.hosts} ${out.hosts === 1 ? 'host' : 'hosts'} from ${file.name} (${out.format})` +
      (out.skipped.length ? `, ${out.skipped.length} rows skipped` : '') + '. Apply to use them, or Ctrl+Z to undo.';
    openHosts({ text: out.text, note });
  } catch (err) { toast('down', 'ERROR', err.message); }
}

/** Ask for a CSV or text file of hosts. Dropping one on the window does the same. */
function pickImportFile() {
  $('f-import-any').click();
}

// -------------------------------------------------------------- add hosts

/** The group new hosts most likely belong in: the selected host's. */
function likelyGroup() {
  const r = view.selected != null ? store.hosts.get(view.selected) : null;
  return r ? r.group || '' : null;
}

/** Open the small Add window. `group` preselects where the hosts go; `text` prefills them. */
function openAdd(group, text) {
  const groups = [...new Set([...store.hosts.values()].sort((a, b) => a.order - b.order).map((r) => r.group).filter(Boolean))];
  const wanted = group === undefined ? likelyGroup() : group;
  $('add-group').innerHTML =
    '<option value="">No group</option>' +
    groups.map((g) => `<option value="${esc(g)}">${esc(g)}</option>`).join('') +
    '<option value="\u0000new">A new group…</option>';
  $('add-group').value = wanted === '\u0000new' ? '\u0000new' : groups.includes(wanted) ? wanted : '';
  $('add-new').value = '';
  $('add-new-wrap').hidden = $('add-group').value !== '\u0000new';
  $('add-text').value = text || '';
  $('add-msg').textContent = '';
  $('add-msg').classList.remove('bad');
  if (!$('dlg-add').open) {
    // A fresh window starts at plain ping; while it stays open the choice is kept for the next host.
    renderChips($('add-chips'), { ping: true, count: 6, on: ['ping'] });
    $('add-ports').value = '';
    $('dlg-add').showModal();
  }
  ($('add-group').value === '\u0000new' && !text ? $('add-new') : $('add-text')).focus();
}

async function addHosts() {
  const entries = $('add-text').value.split('\n').map((line) => line.trim()).filter(Boolean);
  const choice = $('add-group').value;
  const group = choice === '\u0000new' ? $('add-new').value.trim() : choice;
  const msg = $('add-msg');
  msg.classList.add('bad');
  if (!entries.length) { msg.textContent = 'Type a host name or an address.'; $('add-text').focus(); return; }
  if (choice === '\u0000new' && !group) { msg.textContent = 'Give the new group a name.'; $('add-new').focus(); return; }
  const ping = chipPing($('add-chips'));
  const ports = chipPorts($('add-chips'));
  const portsText = $('add-ports').value.trim();
  if (!ping && !ports.length && !portsText) { msg.textContent = 'Choose PING or at least one port.'; return; }
  try {
    const out = await api('hosts/add', { method: 'POST', body: { entries, group: group || null, ports, ports_text: portsText, ping } });
    const noun = ports.length || portsText ? ['row', 'rows'] : ['host', 'hosts'];
    const done = `Added ${out.added} ${noun[out.added === 1 ? 0 : 1]}${group ? ' to ' + group : ''}. Add more, or close.`;
    // Stay open for the next one, in the same group. The new group exists only once the table has it.
    await new Promise((resolve) => setTimeout(resolve, 250));
    openAdd(group, '');
    if (group && $('add-group').value !== group) {
      $('add-group').insertAdjacentHTML('afterbegin', `<option value="${esc(group)}">${esc(group)}</option>`);
      $('add-group').value = group;
      $('add-new-wrap').hidden = true;
    }
    $('add-msg').textContent = done;
    if (!store.running && loaded.autostart !== false) control('start');
  } catch (err) { msg.textContent = err.message; }
}

async function removeHosts(rows) {
  const removable = rows.filter((r) => r.removable);
  if (!removable.length) {
    toast('up', 'NOTE', rows.length === 1 ? `${rows[0].host} shares its line with other hosts. Edit the host list to remove it.` : 'These hosts share their lines with others. Edit the host list to remove them.');
    return;
  }
  const what = removable.length === 1 ? removable[0].host : `${removable.length} hosts`;
  if (!(await confirmBox('REMOVE FROM THE LIST', `${what} will be taken out of the host list, with its history.` + (removable.length < rows.length ? ` ${rows.length - removable.length} of the selected hosts share a line with others and stay.` : ''), 'REMOVE'))) return;
  try {
    await api('hosts', { method: 'POST', body: { action: 'remove', ids: removable.map((r) => r.id) } });
  } catch (err) { toast('down', 'ERROR', err.message); }
}

function confirmBox(title, text, yes) {
  return new Promise((resolve) => {
    const dlg = $('dlg-confirm');
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    $('confirm-yes').textContent = yes;
    dlg.returnValue = 'no';
    dlg.addEventListener('close', () => resolve(dlg.returnValue === 'yes'), { once: true });
    dlg.showModal();
  });
}

// --------------------------------------------------------------- actions

const control = (action) => api('control', { method: 'POST', body: { action } }).catch((err) => toast('down', 'ERROR', err.message));

async function toggleEnabled(id) {
  const r = store.hosts.get(id);
  if (!r) return;
  try { await api('hosts/' + id, { method: 'POST', body: { enabled: !r.enabled } }); refreshDetail(); } catch (err) { toast('down', 'ERROR', err.message); }
}

async function download(format) {
  try {
    const res = await api('export?format=' + format, { raw: true });
    const blob = await res.blob();
    const a = document.createElement('a');
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-');
    a.href = URL.createObjectURL(blob);
    a.download = `pingtable-${stamp}.${format}`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 2000);
  } catch (err) { toast('down', 'ERROR', err.message); }
}

/** One action on several hosts: mute, unmute, solo, unmute_all, ping_now or reset. */
async function bulk(action, ids) {
  try { await api('hosts', { method: 'POST', body: { action, ids } }); refreshDetail(); } catch (err) { toast('down', 'ERROR', err.message); }
}

async function hostAction(id, body) {
  try { await api('hosts/' + id, { method: 'POST', body }); refreshDetail(); } catch (err) { toast('down', 'ERROR', err.message); }
}

async function copyText(text, what) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Older or locked-down browsers: fall back to a hidden text box.
    const box = document.createElement('textarea');
    box.value = text;
    box.style.cssText = 'position:fixed;opacity:0';
    document.body.appendChild(box);
    box.select();
    document.execCommand('copy');
    box.remove();
  }
  toast('up', 'COPIED', what);
}

// This computer: its name, its addresses and the one it sends from by default.
let source = null;
async function loadSource() {
  try { source = await api('source'); } catch {}
  return source;
}

/** "pingtable on MY-PC (192.168.1.50), 2026-11-16 22:40:01" */
function stampLine() {
  const where = source ? ` on ${source.name}${source.default_v4 ? ` (${source.default_v4})` : ''}` : '';
  return `pingtable${where}, ${fmtClock(serverNow(), true)}`;
}

/** A few plain lines about one host, made to be pasted into a chat or a ticket. */
function summaryText(r) {
  const ms = (us) => fmtMs(us) ?? '–';
  const ip = r.ip && r.ip !== r.host ? ` (${r.ip})` : '';
  const why = r.last_status && r.last_status !== 'ok' ? `, ${STATUS_LABEL[r.last_status].toLowerCase()}` : '';
  const lines = [`${r.host}${ip}: ${r.state.toUpperCase()}${why}`];
  lines.push([r.description, checkLabel(r.probe), 'every ' + fmtEvery(r.interval_secs), r.group && 'group ' + r.group, r.tags.length && 'tags ' + r.tags.join(', ')].filter(Boolean).join(' · '));
  if (r.ok) {
    lines.push(`Round trip: last ${ms(r.last_rtt_us)} ms, average ${ms(r.avg_rtt_us)} ms, fastest ${ms(r.min_rtt_us)} ms, slowest ${ms(r.max_rtt_us)} ms` + (r.jitter_us != null ? `, jitter ${ms(r.jitter_us)} ms` : ''));
  }
  lines.push(r.sent ? `Loss: ${fmtPct(r.failed_pct)} (${r.failed.toLocaleString('en-US')} of ${r.sent.toLocaleString('en-US')} pings failed)` : 'No pings sent yet');
  if (r.consec_failed) {
    lines.push(`Failing for ${r.consec_failed} ${r.consec_failed === 1 ? 'ping' : 'pings in a row'}; last answer ${r.last_ok_at_ms ? fmtClock(r.last_ok_at_ms, true) : 'never'}`);
  } else if (r.last_failed_at_ms) {
    lines.push(`Last failure ${fmtClock(r.last_failed_at_ms, true)}; longest run of failures ${r.max_consec_failed}`);
  }
  const extra = [r.hostname && 'name ' + r.hostname, r.mac && 'MAC ' + r.mac, r.last_ttl != null && 'TTL ' + r.last_ttl].filter(Boolean);
  if (extra.length) lines.push(extra.join(' · '));
  if (r.packet) lines.push('Packet: ' + r.packet);
  lines.push(stampLine());
  return lines.join('\n');
}

function groupSummaryText(g) {
  const rows = [...store.hosts.values()].filter((r) => (r.group || '') === g.name).sort((a, b) => a.order - b.order);
  const width = Math.max(...rows.map((r) => r.host.length), 4);
  const lines = [`${g.name || 'Ungrouped'}: ${g.total} ${g.total === 1 ? 'host' : 'hosts'}, ${g.up} up, ${g.down} down`];
  for (const r of rows) {
    const result = r.last_status == null ? 'no result yet' : r.last_status === 'ok' ? fmtMs(r.last_rtt_us) + ' ms' : STATUS_LABEL[r.last_status].toLowerCase();
    lines.push(`${r.state.toUpperCase().padEnd(10)} ${r.host.padEnd(width)}  ${result}${r.description ? '  ' + r.description : ''}`);
  }
  lines.push(stampLine());
  return lines.join('\n');
}

function browserUrl(r) {
  const host = r.host.includes(':') ? `[${r.host}]` : r.host;
  if (r.probe.kind !== 'tcp' || r.probe.port === 80) return `http://${host}`;
  if (r.probe.port === 443) return `https://${host}`;
  return `http://${host}:${r.probe.port}`;
}

function hostMenu(r) {
  const { spark, ...plain } = r;
  const address = browserUrl(r);
  return {
    title: r.host,
    items: [
      { label: 'Copy summary', hint: 'Ctrl+C', run: () => copyText(summaryText(r), 'Summary of ' + r.host) },
      { label: 'Copy host', run: () => copyText(r.host, r.host) },
      r.ip && r.ip !== r.host && { label: 'Copy IP address', run: () => copyText(r.ip, r.ip) },
      { label: 'Copy as JSON', run: () => copyText(JSON.stringify(plain, null, 2), 'JSON for ' + r.host) },
      'sep',
      { label: 'Ping now', disabled: !r.enabled, run: () => hostAction(r.id, { action: 'ping_now' }) },
      { label: r.enabled ? 'Mute' : 'Unmute', hint: 'M', run: () => toggleEnabled(r.id) },
      { label: 'Ping only this host', run: () => bulk('solo', [r.id]) },
      { label: 'Trace route', disabled: r.probe.kind !== 'icmp' && !r.ip, run: () => openTrace(r) },
      { label: 'Check ports…', hint: 'O', run: () => openPorts(r) },
      { label: 'Measure…', disabled: !r.enabled, run: () => openMeasure([r.id]) },
      store.counts.muted > 0 && { label: 'Unmute every host', run: () => bulk('unmute_all', []) },
      { label: 'Show details', hint: 'Enter', run: () => select(r.id, true) },
      { label: r.group ? `Add a host to ${r.group}…` : 'Add a host…', hint: 'A', run: () => openAdd(r.group || '') },
      { label: 'Open ' + address, run: () => window.open(address, '_blank', 'noopener') },
      'sep',
      { label: 'Remove from the list', hint: 'Del', danger: true, run: () => removeHosts([r]) },
      {
        label: 'Reset its counters', danger: true,
        run: async () => {
          if (await confirmBox('RESET ONE HOST', `Counts, times and history for ${r.host} are cleared. This cannot be undone.`, 'RESET')) hostAction(r.id, { action: 'reset' });
        },
      },
    ].filter(Boolean),
  };
}

function groupMenu(g) {
  const closed = view.closed.has(g.name);
  const members = () => [...store.hosts.values()].filter((r) => (r.group || '') === g.name);
  return {
    title: g.name || 'Ungrouped',
    items: [
      { label: 'Add a host here…', hint: 'A', run: () => openAdd(g.name || '') },
      'sep',
      { label: 'Copy summary', run: () => copyText(groupSummaryText(g), 'Summary of ' + (g.name || 'the group')) },
      { label: 'Copy hosts', run: () => copyText(members().map((r) => r.host).join('\n'), `${g.total} hosts`) },
      'sep',
      { label: 'Ping these now', run: () => bulk('ping_now', members().map((r) => r.id)) },
      { label: 'Mute the group', disabled: !members().some((r) => r.enabled), run: () => bulk('mute', members().map((r) => r.id)) },
      { label: 'Unmute the group', disabled: !members().some((r) => !r.enabled), run: () => bulk('unmute', members().map((r) => r.id)) },
      { label: 'Ping only this group', run: () => bulk('solo', members().map((r) => r.id)) },
      { label: 'Measure this group…', disabled: !members().some((r) => r.enabled), run: () => openMeasure(members().map((r) => r.id)) },
      { label: closed ? 'Expand' : 'Collapse', run: () => { if (closed) view.closed.delete(g.name); else view.closed.add(g.name); schedule(); } },
    ],
  };
}

function selectedRows() {
  return view.hostOrder.filter((id) => view.sel.has(id)).map((id) => store.hosts.get(id)).filter(Boolean);
}

/** Options for several highlighted hosts at once. */
function multiMenu(rows) {
  const ids = rows.map((r) => r.id);
  return {
    title: `${rows.length} hosts selected`,
    items: [
      { label: 'Copy summaries', hint: 'Ctrl+C', run: () => copyText(rows.map(summaryText).join('\n\n'), `Summaries of ${rows.length} hosts`) },
      { label: 'Copy hosts', run: () => copyText(rows.map((r) => r.host).join('\n'), `${rows.length} hosts`) },
      'sep',
      { label: 'Ping now', disabled: !rows.some((r) => r.enabled), run: () => bulk('ping_now', ids) },
      { label: 'Mute', hint: 'M', disabled: !rows.some((r) => r.enabled), run: () => bulk('mute', ids) },
      { label: 'Unmute', disabled: !rows.some((r) => !r.enabled), run: () => bulk('unmute', ids) },
      { label: 'Ping only these', run: () => bulk('solo', ids) },
      { label: 'Measure…', disabled: !rows.some((r) => r.enabled), run: () => openMeasure(ids) },
      store.counts.muted > 0 && { label: 'Unmute every host', run: () => bulk('unmute_all', []) },
      'sep',
      { label: 'Remove from the list', hint: 'Del', danger: true, run: () => removeHosts(rows) },
      {
        label: 'Reset their counters', danger: true,
        run: async () => {
          if (await confirmBox('RESET HOSTS', `Counts, times and history for ${rows.length} hosts are cleared. This cannot be undone.`, 'RESET')) bulk('reset', ids);
        },
      },
    ].filter(Boolean),
  };
}

/** Options for the table itself, when the click is not on a row. */
function tableMenu() {
  return {
    title: 'Hosts',
    items: [
      { label: 'Add a host…', hint: 'A', run: () => openAdd() },
      { label: 'Add a new group…', run: () => openAdd('\u0000new') },
      { label: 'Import hosts from a file…', run: pickImportFile },
      { label: 'Edit the host list…', hint: 'H', run: () => openHosts() },
      { label: 'Check ports on a host…', hint: 'O', run: () => openPorts() },
      'sep',
      { label: 'Settings…', hint: ',', run: () => openSettings() },
    ],
  };
}

function openContextMenu(menu, x, y) {
  const el = $('ctx');
  el.innerHTML = `<div class="menu-title">${esc(menu.title)}</div>` + menu.items
    .map((item, i) => (item === 'sep'
      ? '<div class="menu-sep"></div>'
      : `<button class="menu-item${item.danger ? ' menu-danger' : ''}" data-i="${i}" role="menuitem"${item.disabled ? ' disabled' : ''}>${esc(item.label)}${item.hint ? `<span class="hint">${item.hint}</span>` : ''}</button>`))
    .join('');
  el.hidden = false;
  el.onclick = (e) => {
    const button = e.target.closest('[data-i]');
    if (!button) return;
    closeContextMenu();
    menu.items[Number(button.dataset.i)].run();
  };
  const box = el.getBoundingClientRect();
  el.style.left = Math.max(4, Math.min(x, innerWidth - box.width - 4)) + 'px';
  el.style.top = Math.max(4, Math.min(y, innerHeight - box.height - 4)) + 'px';
}

function closeContextMenu() {
  $('ctx').hidden = true;
}

function contextMenuForKey(key, x, y) {
  if (key.startsWith('g:')) {
    const item = view.items.find((i) => i.key === key);
    if (item) openContextMenu(groupMenu(item.group), x, y);
    return;
  }
  const r = store.hosts.get(Number(key.slice(2)));
  if (!r) return;
  // Right-clicking inside a multi-selection acts on all of it.
  if (view.sel.has(r.id) && view.sel.size > 1) {
    openContextMenu(multiMenu(selectedRows()), x, y);
    return;
  }
  select(r.id);
  openContextMenu(hostMenu(r), x, y);
}

// ------------------------------------------------------- measure and trace

let measureIds = [];
let measureTimer = 0;
let measureLast = null;

/** Open the measurement window for these hosts (or every unmuted host). */
function openMeasure(ids) {
  const usable = (ids && ids.length ? ids : [...store.hosts.keys()]).filter((id) => store.hosts.get(id)?.enabled);
  measureIds = usable;
  $('m-msg').textContent = '';
  if (store.measureEndsAt != null) {
    showMeasureRun();
  } else {
    const names = usable.map((id) => store.hosts.get(id).host);
    $('measure-who').innerHTML = usable.length
      ? `<b>${usable.length} ${usable.length === 1 ? 'host' : 'hosts'}:</b> ${esc(names.slice(0, 6).join(', '))}${names.length > 6 ? ` and ${names.length - 6} more` : ''}`
      : 'There is no unmuted host to measure.';
    $('measure-setup').hidden = false;
    $('measure-run').hidden = true;
    $('m-start').hidden = false;
    $('m-start').textContent = 'START';
    $('m-start').disabled = !usable.length;
    $('m-stop').hidden = true;
    $('m-copy').hidden = true;
  }
  $('dlg-measure').showModal();
}

async function startMeasure() {
  try {
    await api('measure', { method: 'POST', body: { ids: measureIds, duration_secs: Number($('m-duration').value), per_second: Number($('m-rate').value) } });
    showMeasureRun();
  } catch (err) { $('m-msg').textContent = err.message; }
}

/** Back to the choices, to measure the same hosts again. */
function openMeasureSetup() {
  $('dlg-measure').close();
  openMeasure(measureIds);
}

function showMeasureRun() {
  $('measure-setup').hidden = true;
  $('measure-run').hidden = false;
  $('m-start').hidden = true;
  clearInterval(measureTimer);
  pollMeasure();
  measureTimer = setInterval(pollMeasure, 500);
}

async function pollMeasure() {
  let m;
  try { m = (await api('measure')).measurement; } catch { return; }
  if (!m) return;
  measureLast = m;
  const rate = `${m.per_second} ${m.per_second === 1 ? 'ping' : 'pings'} a second`;
  if (m.running) {
    $('m-clock').textContent = fmtSpan(m.ends_at_ms - serverNow());
    $('m-clock-note').textContent = `left of ${fmtSpan(m.duration_secs * 1000)}, at ${rate}`;
  } else {
    $('m-clock').textContent = 'DONE';
    const ran = (m.ended_at_ms ?? m.ends_at_ms) - m.started_at_ms;
    $('m-clock-note').textContent = `ran ${fmtSpan(ran)} at ${rate}, started ${fmtClock(m.started_at_ms)}` + (ran < m.duration_secs * 1000 - 500 ? ' (stopped early)' : '');
    clearInterval(measureTimer);
    $('m-start').hidden = false;
    $('m-start').textContent = 'MEASURE AGAIN';
    $('m-start').disabled = false;
  }
  $('m-stop').hidden = !m.running;
  $('m-copy').hidden = m.running;
  const cell = (us) => `<span class="n${us == null ? ' quiet' : ''}">${fmtMs(us) ?? '–'}</span>`;
  $('m-table').innerHTML =
    '<div class="hd"><span>Host</span><span class="n">Sent</span><span class="n">Lost</span><span class="n">Loss</span><span class="n">Fastest</span><span class="n">Average</span><span class="n">Median</span><span class="n">95th</span><span class="n">Slowest</span><span class="n">Jitter</span></div>' +
    m.hosts.map((x) => `<div><span title="${esc(x.description)}"><b>${esc(x.host)}</b></span><span class="n">${x.sent}</span><span class="n${x.lost ? ' bad' : ' quiet'}">${x.lost}</span>` +
      `<span class="n${x.lost ? ' bad' : ' quiet'}">${fmtPct(x.loss_pct)}</span>${cell(x.min_rtt_us)}${cell(x.avg_rtt_us)}${cell(x.median_rtt_us)}${cell(x.p95_rtt_us)}${cell(x.max_rtt_us)}${cell(x.jitter_us)}</div>`).join('');
}

/** The measurement as fixed-width text for pasting. Times are in milliseconds. */
function measureReportText(m) {
  const ms = (us) => fmtMs(us) ?? '-';
  const width = Math.max(4, ...m.hosts.map((x) => x.host.length));
  const ran = (m.ended_at_ms ?? m.ends_at_ms) - m.started_at_ms;
  const lines = [
    `Measurement: ${fmtSpan(ran)} at ${m.per_second} ${m.per_second === 1 ? 'ping' : 'pings'} a second, started ${fmtClock(m.started_at_ms, true)}`,
    `${'HOST'.padEnd(width)}  ${['SENT', 'LOST', 'LOSS', 'FASTEST', 'AVERAGE', 'MEDIAN', '95TH', 'SLOWEST', 'JITTER'].map((c) => c.padStart(8)).join('')}`,
  ];
  for (const x of m.hosts) {
    const cells = [x.sent, x.lost, fmtPct(x.loss_pct), ms(x.min_rtt_us), ms(x.avg_rtt_us), ms(x.median_rtt_us), ms(x.p95_rtt_us), ms(x.max_rtt_us), ms(x.jitter_us)];
    lines.push(`${x.host.padEnd(width)}  ${cells.map((c) => String(c).padStart(8)).join('')}`);
  }
  lines.push('Times in milliseconds.', stampLine());
  return lines.join('\n');
}

let traceHost = null;
let traceLast = null;

async function openTrace(r) {
  traceHost = r;
  traceLast = null;
  $('trace-title').textContent = 'ROUTE TO ' + r.host.toUpperCase();
  $('trace-note').textContent = 'Tracing… this takes a second or two.';
  $('trace-table').innerHTML = '';
  $('trace-msg').textContent = '';
  if (!$('dlg-trace').open) $('dlg-trace').showModal();
  try {
    const out = await api(`hosts/${r.id}/trace`, { method: 'POST', body: {} });
    if (traceHost !== r) return;
    traceLast = out;
    $('trace-note').textContent = out.reached
      ? `${out.hops.length} ${out.hops.length === 1 ? 'hop' : 'hops'} from ${out.from}${out.source ? ` (${out.source})` : ''} to ${out.target}.`
      : `The trace did not reach ${out.target}. Routers that stay silent show as a dash; many do not answer.`;
    $('trace-table').innerHTML =
      '<div class="hd"><span class="n">Hop</span><span>Address</span><span>Name</span><span class="n">ms</span></div>' +
      out.hops.map((hop) => `<div><span class="n">${hop.ttl}</span><span${hop.addr ? '' : ' class="quiet"'}>${hop.addr ? `<b>${esc(hop.addr)}</b>` : '–'}</span>` +
        `<span class="quiet">${esc(hop.name ?? (hop.addr ? '' : 'no answer'))}</span><span class="n${hop.rtt_us == null ? ' quiet' : ''}">${fmtMs(hop.rtt_us) ?? '–'}</span></div>`).join('');
  } catch (err) {
    $('trace-note').textContent = err.message;
  }
}

function traceText(out) {
  const lines = [`Route to ${out.host} (${out.target}) from ${out.from}${out.source ? ` (${out.source})` : ''}`];
  for (const hop of out.hops) {
    lines.push(`${String(hop.ttl).padStart(2)}  ${(hop.addr ?? '*').padEnd(39)} ${(fmtMs(hop.rtt_us) ?? '*').padStart(8)} ms  ${hop.name ?? ''}`.trimEnd());
  }
  if (!out.reached) lines.push('The trace did not reach the host.');
  lines.push(stampLine());
  return lines.join('\n');
}

// -------------------------------------------------------------- port check

/** TCP ports offered as one-click choices. Keep in step with COMMON_PORTS in crates/core/src/portcheck.rs. */
const PORT_CHOICES = [[22, 'SSH'], [80, 'HTTP'], [443, 'HTTPS'], [3389, 'RDP'], [445, 'SMB'], [53, 'DNS'], [25, 'SMTP'], [3306, 'MySQL'], [5432, 'Postgres'], [1433, 'MSSQL'], [8080, 'HTTP alt'], [8443, 'HTTPS alt']];
const PORT_STATE = { open: 'OPEN, listening', closed: 'CLOSED, refused', no_answer: 'NO ANSWER' };
const PORTS_HELP = 'One TCP connection is opened to each port and closed again; nothing is sent. OPEN means something is listening. CLOSED means the host answered and refused. NO ANSWER usually means a firewall dropped it, or the host is off.';

/** Fill a row of toggle buttons: an optional PING choice, then the first `count` common ports. */
function renderChips(el, { ping = false, count = PORT_CHOICES.length, on = [] } = {}) {
  const chip = (attr, isOn, title, text) => `<button type="button" class="chip${isOn ? ' is-on' : ''}" ${attr} aria-pressed="${isOn}" title="${title}">${text}</button>`;
  el.innerHTML =
    (ping ? chip('data-ping', on.includes('ping'), 'ICMP echo: is the host there?', 'PING') : '') +
    PORT_CHOICES.slice(0, count).map(([port, name]) => chip(`data-port="${port}"`, on.includes(port), `TCP ${port}: is ${name} listening?`, `${port}<i>${name}</i>`)).join('');
}
const chipPorts = (el) => [...el.querySelectorAll('.chip.is-on[data-port]')].map((b) => Number(b.dataset.port));
const chipPing = (el) => !!el.querySelector('.chip.is-on[data-ping]');
function toggleChip(e) {
  const chip = e.target.closest('.chip');
  if (!chip) return;
  const on = !chip.classList.contains('is-on');
  chip.classList.toggle('is-on', on);
  chip.setAttribute('aria-pressed', String(on));
}

let portsGroup = null; // the group "watch in the table" adds rows to
let portsLast = null;  // the last answer, for copy and watch
let portsSeq = 0;

/** Open the port check window, for row `r` when there is one. Nothing is sent until CHECK. */
function openPorts(r) {
  r = r || (view.selected != null ? store.hosts.get(view.selected) : null) || null;
  portsGroup = r ? r.group || '' : null;
  portsLast = null;
  portsSeq++;
  const remembered = Array.isArray(prefs.ports) && prefs.ports.length ? prefs.ports : [22, 80, 443, 3389];
  const own = r && r.probe.kind === 'tcp' ? r.probe.port : null;
  const isChoice = own != null && PORT_CHOICES.some(([port]) => port === own);
  renderChips($('pc-chips'), { on: isChoice ? [own, ...remembered] : remembered });
  $('pc-host').value = r ? r.host : '';
  $('pc-ports').value = own != null && !isChoice ? String(own) : '';
  $('pc-note').textContent = PORTS_HELP;
  $('pc-table').innerHTML = '';
  $('pc-msg').textContent = '';
  $('pc-msg').classList.remove('bad');
  $('pc-copy').hidden = true;
  $('pc-watch').hidden = true;
  $('pc-go').disabled = false;
  if (!$('dlg-ports').open) $('dlg-ports').showModal();
  (r ? $('pc-go') : $('pc-host')).focus();
}

const portStateText = (p) => PORT_STATE[p.state] ?? STATUS_LABEL[p.status] ?? 'Failed';

async function runPortCheck() {
  const host = $('pc-host').value.trim();
  const ports = chipPorts($('pc-chips'));
  const msg = $('pc-msg');
  msg.classList.add('bad');
  if (!host) { msg.textContent = 'Type a host name or an address.'; $('pc-host').focus(); return; }
  const mine = ++portsSeq;
  msg.classList.remove('bad');
  msg.textContent = 'Checking… a port that does not answer takes a few seconds.';
  $('pc-go').disabled = true;
  try {
    const out = await api('portcheck', { method: 'POST', body: { host, ports, ports_text: $('pc-ports').value.trim() } });
    if (mine !== portsSeq) return;
    portsLast = out;
    prefs.ports = ports;
    savePrefs();
    const n = out.results.length;
    msg.textContent = '';
    $('pc-note').textContent = `${out.open} of ${n} ${n === 1 ? 'port' : 'ports'} open on ${out.host}${out.target !== out.host ? ` (${out.target})` : ''}, checked from ${out.from}${out.source ? ` (${out.source})` : ''} at ${fmtClock(out.checked_at_ms)}, waiting up to ${out.timeout_ms} ms for each. ${PORTS_HELP}`;
    $('pc-table').innerHTML =
      '<div class="hd"><span class="n">Port</span><span>Usually</span><span>Result</span><span class="n">ms</span></div>' +
      out.results.map((p) => `<div><span class="n"><b>${p.port}</b></span><span class="quiet">${esc(p.service ?? '')}</span>` +
        `<span class="${p.state}">${esc(portStateText(p))}</span><span class="n${p.rtt_us == null ? ' quiet' : ''}">${fmtMs(p.rtt_us) ?? '–'}</span></div>`).join('');
    $('pc-copy').hidden = false;
    $('pc-watch').hidden = false;
  } catch (err) {
    if (mine === portsSeq) { msg.classList.add('bad'); msg.textContent = err.message; }
  } finally {
    if (mine === portsSeq) $('pc-go').disabled = false;
  }
}

function portsText(out) {
  const lines = [`Port check of ${out.host}${out.target !== out.host ? ` (${out.target})` : ''} from ${out.from}${out.source ? ` (${out.source})` : ''}`];
  for (const p of out.results) {
    lines.push(`${String(p.port).padStart(5)}  ${(p.service ?? '').padEnd(16)} ${portStateText(p).padEnd(18)}${p.rtt_us == null ? '' : (fmtMs(p.rtt_us) + ' ms').padStart(10)}`.trimEnd());
  }
  lines.push(stampLine());
  return lines.join('\n');
}

/** Turn the ports just checked into rows, so they are checked all the time. */
async function watchPorts() {
  const out = portsLast;
  if (!out) return;
  const msg = $('pc-msg');
  const watched = new Set([...store.hosts.values()].filter((r) => r.host === out.host && r.probe.kind === 'tcp').map((r) => r.probe.port));
  const ports = out.results.map((p) => p.port).filter((port) => !watched.has(port));
  msg.classList.remove('bad');
  if (!ports.length) { msg.textContent = 'These ports are already rows in the table.'; return; }
  try {
    const group = portsGroup ?? likelyGroup();
    const added = await api('hosts/add', { method: 'POST', body: { entries: [out.host], group: group || null, ports, ping: false } });
    msg.textContent = `Added ${added.added} ${added.added === 1 ? 'row' : 'rows'}${group ? ' to ' + group : ''}. They are checked with the rest from now on.`;
    if (!store.running && loaded.autostart !== false) control('start');
  } catch (err) { msg.classList.add('bad'); msg.textContent = err.message; }
}

async function restoreDefaultHosts() {
  try {
    const defaults = await api('defaults');
    const box = $('f-hosts');
    box.focus();
    box.select();
    // insertText keeps the change on the undo stack, so Ctrl+Z brings the old list back.
    if (!document.execCommand('insertText', false, defaults.hosts_text)) box.value = defaults.hosts_text;
    $('f-described').checked = defaults.settings.description_format;
    preview();
    setMsg('Default hosts are in the editor. Apply to use them, or Ctrl+Z to undo.');
  } catch (err) { setMsg(err.message, true); }
}

async function useExampleHosts() {
  try {
    const [defaults, cfg] = await Promise.all([api('defaults'), api('config')]);
    await api('config', {
      method: 'PUT',
      body: {
        settings: { ...cfg.settings, description_format: defaults.settings.description_format },
        hosts_text: defaults.hosts_text,
        keep_stats: false,
        autostart: cfg.autostart,
      },
    });
    await control('start');
  } catch (err) { toast('down', 'ERROR', err.message); }
}

async function openAbout() {
  try {
    const info = await api('info');
    $('about-version').textContent = 'Version ' + info.version;
    const facts = [
      ['Website', info.website.replace(/^https?:\/\//, '')],
      ['Source code', info.source.replace(/^https?:\/\//, '')],
      ['License', info.license + '. Copyright 2026 Lorincz LLC.'],
      ['Credits', info.credits],
      ['Built with', `Rust ${info.rustc}, compiled for ${info.target}`],
      ['This computer', info.machine + (source && source.default_v4 ? `, sending from ${source.default_v4}` : '')],
      ['Pings sent with', info.icmp],
      ['Port checks', 'a TCP connection to the port, closed at once; nothing is sent'],
      ['Command line', 'pingtable-cli, beside the app: once, ports, or --help'],
      ['AI agents', 'pingtable-cli mcp serves these checks as tools; pingtable-cli api shows the local API'],
      ['Shown in', window.__PINGTABLE_SHELL__ ? 'its own window (Microsoft Edge WebView2)' : 'your browser'],
      ['Settings file', info.config_path],
    ];
    $('about-facts').innerHTML = facts.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('');
    $('about-libs').textContent = 'Uses the open-source libraries ' + info.libraries.join(', ') + '.';
  } catch (err) { $('about-version').textContent = err.message; }
  $('dlg-about').showModal();
}

function setFilter(name) {
  view.filter = view.filter === name && name !== 'all' ? 'all' : name;
  schedule();
}

function buildMenu() {
  $('menu-cols').innerHTML = COLS.filter((c) => !c.fixed)
    .map((c) => `<button class="menu-item${prefs.cols.includes(c.id) ? ' is-checked' : ''}" data-col="${c.id}"><span class="check"></span>${esc(c.label)}</button>`)
    .join('');
  $('mi-group').classList.toggle('is-checked', prefs.grouped);
  $('mi-sound').classList.toggle('is-checked', prefs.sound);
  $('mi-unmute').disabled = store.counts.muted === 0;
}

function toggleMenu(open) {
  const menu = $('menu');
  const show = open ?? menu.hidden;
  if (show) buildMenu();
  menu.hidden = !show;
  $('btn-menu').setAttribute('aria-expanded', String(show));
}

function moveSelection(delta) {
  const order = view.hostOrder;
  if (!order.length) return;
  const at = order.indexOf(view.selected);
  const next = at < 0 ? (delta > 0 ? 0 : order.length - 1) : Math.min(order.length - 1, Math.max(0, at + delta));
  select(order[next]);
}

// ---------------------------------------------------------------- wiring

function wire() {
  $('grid').addEventListener('scroll', () => { schedule(false); closeContextMenu(); }, { passive: true });
  new ResizeObserver(() => { refitColumns(); schedule(false); drawChart(); }).observe($('grid'));
  new ResizeObserver(() => drawChart()).observe($('d-chart'));

  $('thead').addEventListener('click', (e) => {
    const th = e.target.closest('.th');
    if (!th || resizing || e.target.closest('.grip')) return;
    const id = th.dataset.col;
    if (prefs.sort === id) { if (prefs.dir === 1) prefs.dir = -1; else { prefs.sort = null; prefs.dir = 1; } }
    else { prefs.sort = id; prefs.dir = COL[id].num || id === 'state' ? (id === 'state' ? 1 : -1) : 1; }
    savePrefs();
    layoutColumns();
    schedule();
  });

  // Drag the edge of a column heading to resize it; double-click the edge to reset.
  $('thead').addEventListener('pointerdown', (e) => {
    const grip = e.target.closest('.grip');
    if (!grip) return;
    e.preventDefault();
    const col = COL[grip.dataset.grip];
    const startX = e.clientX;
    const startW = colWidth(col);
    resizing = true;
    grip.classList.add('is-dragging');
    grip.setPointerCapture(e.pointerId);
    const move = (ev) => {
      prefs.widths = prefs.widths || {};
      prefs.widths[col.id] = Math.max(44, Math.min(900, startW + ev.clientX - startX));
      applyColumnWidths();
    };
    const up = () => {
      grip.classList.remove('is-dragging');
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', up);
      grip.removeEventListener('pointercancel', up);
      savePrefs();
      setTimeout(() => { resizing = false; }, 0); // let the click that ends the drag pass first
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
    grip.addEventListener('pointercancel', up);
  });
  $('thead').addEventListener('dblclick', (e) => {
    const grip = e.target.closest('.grip');
    if (!grip || !prefs.widths) return;
    delete prefs.widths[grip.dataset.grip];
    savePrefs();
    applyColumnWidths();
  });

  // Shift-click and Ctrl-click choose rows; they should not also highlight text.
  $('tbody').addEventListener('mousedown', (e) => { if (e.shiftKey || e.ctrlKey || e.metaKey) e.preventDefault(); });

  $('tbody').addEventListener('click', (e) => {
    const row = e.target.closest('[data-key]');
    if (!row) return;
    const key = row.dataset.key;
    if (key.startsWith('g:')) {
      const name = key.slice(2);
      if (view.closed.has(name)) view.closed.delete(name); else view.closed.add(name);
      schedule();
      return;
    }
    const id = Number(key.slice(2));
    if (e.shiftKey && view.selected != null) {
      // Everything between the anchor and the clicked row.
      const order = view.hostOrder;
      const [from, to] = [order.indexOf(view.selected), order.indexOf(id)].sort((x, y) => x - y);
      if (from >= 0) view.sel = new Set(order.slice(from, to + 1));
      schedule(false);
    } else if (e.ctrlKey || e.metaKey) {
      if (view.sel.has(id) && view.sel.size > 1) {
        view.sel.delete(id);
        if (view.selected === id) view.selected = [...view.sel][0];
      } else {
        view.sel.add(id);
        if (view.selected == null) view.selected = id;
      }
      schedule(false);
    } else {
      select(id, true);
    }
  });

  $('tallies').addEventListener('click', (e) => { const b = e.target.closest('.tally'); if (b) setFilter(b.dataset.filter); });
  $('search').addEventListener('input', (e) => { view.query = e.target.value.trim().toLowerCase(); schedule(); });
  $('btn-toggle').addEventListener('click', () => control(store.running || store.measureEndsAt != null ? 'stop' : 'start'));
  $('btn-add').addEventListener('click', () => openAdd());
  $('btn-ports').addEventListener('click', () => openPorts());
  $('mi-ports').addEventListener('click', () => { toggleMenu(false); openPorts(); });
  $('mi-import').addEventListener('click', () => { toggleMenu(false); pickImportFile(); });
  $('foot-src').addEventListener('click', () => openSettings('network'));
  $('foot-rate').addEventListener('click', () => openSettings('pinging'));
  $('btn-settings').addEventListener('click', () => openSettings());
  $('mi-add').addEventListener('click', () => { toggleMenu(false); openAdd(); });
  $('mi-list').addEventListener('click', () => { toggleMenu(false); openHosts(); });
  $('mi-settings').addEventListener('click', () => { toggleMenu(false); openSettings(); });

  // Add window
  $('add-go').addEventListener('click', addHosts);
  $('add-chips').addEventListener('click', toggleChip);
  $('add-import').addEventListener('click', pickImportFile);
  $('add-ports').addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); addHosts(); } });

  // Port check window
  $('pc-chips').addEventListener('click', toggleChip);
  $('pc-go').addEventListener('click', runPortCheck);
  $('pc-close').addEventListener('click', () => $('dlg-ports').close());
  $('pc-copy').addEventListener('click', () => { if (portsLast) copyText(portsText(portsLast), 'Port check of ' + portsLast.host); });
  $('pc-watch').addEventListener('click', watchPorts);
  for (const id of ['pc-host', 'pc-ports']) $(id).addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); runPortCheck(); } });
  $('add-close').addEventListener('click', () => $('dlg-add').close());
  $('add-group').addEventListener('change', () => {
    const fresh = $('add-group').value === '\u0000new';
    $('add-new-wrap').hidden = !fresh;
    if (fresh) $('add-new').focus();
  });
  $('add-text').addEventListener('keydown', (e) => {
    // Enter adds; Shift+Enter (or Enter on a list of several) starts a new line.
    if (e.key === 'Enter' && !e.shiftKey && (e.ctrlKey || !$('add-text').value.includes('\n'))) { e.preventDefault(); addHosts(); }
  });
  $('add-new').addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); $('add-text').focus(); } });

  // Paste hosts straight onto the table.
  document.addEventListener('paste', (e) => {
    if (document.querySelector('dialog[open]') || e.target.closest('input, textarea')) return;
    const text = (e.clipboardData && e.clipboardData.getData('text/plain') || '').trim();
    if (!text) return;
    e.preventDefault();
    openAdd(undefined, text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).join('\n'));
  });

  // Drop a host list or CSV file anywhere on the window.
  let dragDepth = 0;
  const hasFiles = (e) => e.dataTransfer && [...e.dataTransfer.types].includes('Files');
  addEventListener('dragenter', (e) => { if (hasFiles(e)) { dragDepth++; document.body.classList.add('is-dropping'); } });
  addEventListener('dragleave', (e) => { if (hasFiles(e) && --dragDepth <= 0) { dragDepth = 0; document.body.classList.remove('is-dropping'); } });
  addEventListener('dragover', (e) => { if (hasFiles(e)) e.preventDefault(); });
  addEventListener('drop', (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth = 0;
    document.body.classList.remove('is-dropping');
    importFile(e.dataTransfer.files[0]);
  });
  $('btn-import').addEventListener('click', () => $('f-import').click());
  // The same import from anywhere else: the file's hosts open in the list editor for review.
  $('f-import-any').addEventListener('change', (e) => {
    const file = e.target.files[0];
    e.target.value = '';
    if (!file) return;
    if ($('dlg-add').open) $('dlg-add').close();
    importFile(file);
  });
  $('f-import').addEventListener('change', async (e) => {
    const file = e.target.files[0];
    e.target.value = '';
    if (!file) return;
    try {
      const out = await api('import', { method: 'POST', body: { text: await file.text() } });
      if (!out.hosts) { setMsg(`No hosts found in ${file.name}.`, true); return; }
      appendToEditor(out.text, `${out.hosts} ${out.hosts === 1 ? 'host' : 'hosts'} from ${file.name} (${out.format})` + (out.skipped.length ? `, ${out.skipped.length} rows skipped` : '') + '. Apply to use them, or Ctrl+Z to undo.');
    } catch (err) { setMsg(err.message, true); }
  });

  // Drag a column heading sideways to move the column.
  let draggedCol = null;
  const clearDropMarks = () => document.querySelectorAll('.th.drop-before, .th.drop-after, .th.is-dragged').forEach((el) => el.classList.remove('drop-before', 'drop-after', 'is-dragged'));
  $('thead').addEventListener('dragstart', (e) => {
    const th = e.target.closest('.th[draggable="true"]');
    if (!th || resizing) { e.preventDefault(); return; }
    draggedCol = th.dataset.col;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', draggedCol);
    th.classList.add('is-dragged');
  });
  $('thead').addEventListener('dragover', (e) => {
    const th = e.target.closest('.th[draggable="true"]');
    if (!draggedCol || !th || th.dataset.col === draggedCol) return;
    e.preventDefault();
    const box = th.getBoundingClientRect();
    const after = e.clientX > box.left + box.width / 2;
    document.querySelectorAll('.th.drop-before, .th.drop-after').forEach((el) => el.classList.remove('drop-before', 'drop-after'));
    th.classList.add(after ? 'drop-after' : 'drop-before');
  });
  $('thead').addEventListener('drop', (e) => {
    const th = e.target.closest('.th[draggable="true"]');
    if (!draggedCol || !th || th.dataset.col === draggedCol) return;
    e.preventDefault();
    const after = th.classList.contains('drop-after');
    const order = orderedCols().map((c) => c.id).filter((id) => id !== draggedCol);
    order.splice(order.indexOf(th.dataset.col) + (after ? 1 : 0), 0, draggedCol);
    prefs.order = order;
    savePrefs();
    layoutColumns();
    schedule();
  });
  $('thead').addEventListener('dragend', () => { draggedCol = null; clearDropMarks(); });
  $('btn-empty-add').addEventListener('click', () => openAdd());
  $('btn-empty-import').addEventListener('click', pickImportFile);
  $('btn-empty-example').addEventListener('click', useExampleHosts);
  $('btn-defaults').addEventListener('click', restoreDefaultHosts);

  // Right-click a row for its options. Elsewhere the browser's own menu stays,
  // except in the Windows window, where it would only offer Back and Reload.
  document.addEventListener('contextmenu', (e) => {
    const row = e.target.closest('#tbody [data-key]');
    if (row) {
      e.preventDefault();
      contextMenuForKey(row.dataset.key, e.clientX, e.clientY);
    } else if (e.target.closest('#grid') && !e.target.closest('.thead')) {
      e.preventDefault();
      openContextMenu(tableMenu(), e.clientX, e.clientY);
    } else if (window.__PINGTABLE_SHELL__ && !e.target.closest('input, textarea')) {
      e.preventDefault();
    }
  });
  document.addEventListener('pointerdown', (e) => { if (!$('ctx').hidden && !e.target.closest('#ctx')) closeContextMenu(); });
  addEventListener('blur', closeContextMenu);
  addEventListener('resize', closeContextMenu);

  $('btn-menu').addEventListener('click', (e) => { e.stopPropagation(); toggleMenu(); });
  document.addEventListener('click', (e) => { if (!$('menu').hidden && !e.target.closest('.menu-wrap')) toggleMenu(false); });
  $('menu-cols').addEventListener('click', (e) => {
    const item = e.target.closest('[data-col]');
    if (!item) return;
    const id = item.dataset.col;
    prefs.cols = prefs.cols.includes(id) ? prefs.cols.filter((c) => c !== id) : [...prefs.cols, id];
    savePrefs(); buildMenu(); layoutColumns(); schedule();
  });
  $('mi-group').addEventListener('click', () => { prefs.grouped = !prefs.grouped; savePrefs(); buildMenu(); schedule(); });
  $('mi-sound').addEventListener('click', () => { prefs.sound = !prefs.sound; savePrefs(); buildMenu(); if (prefs.sound) beep('up'); });
  $('btn-theme').addEventListener('click', toggleTheme);
  $('mi-unmute').addEventListener('click', () => { toggleMenu(false); bulk('unmute_all', []); });
  $('mi-now').addEventListener('click', () => { toggleMenu(false); control('ping_now'); });
  $('mi-measure').addEventListener('click', () => { toggleMenu(false); openMeasure(view.sel.size > 1 ? [...view.sel] : null); });
  $('run-state').addEventListener('click', () => { if (store.measureEndsAt != null) openMeasure(null); });
  $('m-start').addEventListener('click', () => { if ($('measure-run').hidden) startMeasure(); else openMeasureSetup(); });
  $('m-stop').addEventListener('click', () => api('measure/stop', { method: 'POST', body: {} }).then(pollMeasure).catch(() => {}));
  $('m-copy').addEventListener('click', () => { if (measureLast) copyText(measureReportText(measureLast), 'Measurement report'); });
  $('m-close').addEventListener('click', () => $('dlg-measure').close());
  $('dlg-measure').addEventListener('close', () => clearInterval(measureTimer));
  $('trace-again').addEventListener('click', () => { if (traceHost) openTrace(traceHost); });
  $('trace-copy').addEventListener('click', () => { if (traceLast) copyText(traceText(traceLast), 'Route to ' + traceLast.host); });
  $('mi-csv').addEventListener('click', (e) => { e.preventDefault(); toggleMenu(false); download('csv'); });
  $('mi-json').addEventListener('click', (e) => { e.preventDefault(); toggleMenu(false); download('json'); });
  $('mi-keys').addEventListener('click', () => { toggleMenu(false); $('dlg-keys').showModal(); });
  $('mi-about').addEventListener('click', () => { toggleMenu(false); openAbout(); });
  $('mi-widths').addEventListener('click', () => {
    // Back to the default set of columns, their order and their widths.
    prefs.widths = {};
    prefs.order = null;
    prefs.cols = COLS.filter((c) => !c.off).map((c) => c.id);
    savePrefs(); layoutColumns(); schedule(); toggleMenu(false);
  });
  $('mi-reset').addEventListener('click', async () => {
    toggleMenu(false);
    if (await confirmBox('RESET COUNTERS', 'Every count, time and history line is cleared. The host list stays. This cannot be undone.', 'RESET')) control('reset');
  });

  // detail
  $('d-close').addEventListener('click', closeDetail);
  $('d-toggle').addEventListener('click', () => toggleEnabled(view.selected));
  const chart = $('d-chart');
  chart.addEventListener('mousemove', chartHover);
  chart.addEventListener('mouseleave', () => { hoverIndex = -1; $('d-tip').hidden = true; drawChart(); });
  $('detail-grip').addEventListener('pointerdown', (e) => {
    e.preventDefault();
    const panel = $('detail');
    const startY = e.clientY, startH = panel.offsetHeight;
    const move = (ev) => {
      prefs.detailH = Math.max(180, Math.min(window.innerHeight * 0.75, startH + (startY - ev.clientY)));
      panel.style.setProperty('--detail-h', prefs.detailH + 'px');
      schedule(false);
    };
    const up = () => { removeEventListener('pointermove', move); removeEventListener('pointerup', up); savePrefs(); drawChart(); };
    addEventListener('pointermove', move);
    addEventListener('pointerup', up);
  });

  // hosts dialog
  document.querySelectorAll('#dlg-settings .tab').forEach((el) => el.addEventListener('click', () => showTab(el.dataset.tab)));
  $('settings-cancel').addEventListener('click', () => $('dlg-settings').close());
  $('settings-apply').addEventListener('click', applySettings);
  $('f-hosts').addEventListener('input', preview);
  ['f-described', 'f-skipfirst', 'f-skiplast'].forEach((id) => $(id).addEventListener('change', preview));
  $('dlg-cancel').addEventListener('click', () => $('dlg-hosts').close());
  $('dlg-apply').addEventListener('click', () => applyHosts(true));
  $('dlg-reset').addEventListener('click', () => applyHosts(false));
  $('f-hosts').addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); applyHosts(true); } });

  // keyboard
  document.addEventListener('keydown', (e) => {
    if (document.querySelector('dialog[open]')) return;
    const typing = e.target.matches('input, textarea, select');
    if (e.key === 'Escape') {
      if (!$('ctx').hidden) closeContextMenu();
      else if (!$('menu').hidden) toggleMenu(false);
      else if (typing) { e.target.value = ''; e.target.dispatchEvent(new Event('input')); e.target.blur(); }
      else if (view.sel.size > 1) { view.sel = new Set(view.selected != null ? [view.selected] : []); schedule(false); }
      else if (!$('detail').hidden) closeDetail();
      else if (view.filter !== 'all') setFilter('all');
      return;
    }
    if (typing) return;
    const selected = view.selected != null ? store.hosts.get(view.selected) : null;
    const rows = selectedRows();
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && rows.length && !String(getSelection())) {
      e.preventDefault();
      copyText(rows.map(summaryText).join('\n\n'), rows.length === 1 ? 'Summary of ' + rows[0].host : `Summaries of ${rows.length} hosts`);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a' && view.hostOrder.length) {
      e.preventDefault();
      view.sel = new Set(view.hostOrder);
      if (view.selected == null) view.selected = view.hostOrder[0];
      schedule(false);
      return;
    }
    if ((e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) && selected) {
      e.preventDefault();
      const rowEl = document.querySelector(`#tbody [data-key="h:${selected.id}"]`);
      const box = rowEl ? rowEl.getBoundingClientRect() : { left: 40, bottom: 120 };
      openContextMenu(rows.length > 1 ? multiMenu(rows) : hostMenu(selected), box.left + 40, box.bottom - 4);
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const actions = {
      '/': () => $('search').focus(),
      ArrowDown: () => moveSelection(1), j: () => moveSelection(1),
      ArrowUp: () => moveSelection(-1), k: () => moveSelection(-1),
      PageDown: () => moveSelection(15), PageUp: () => moveSelection(-15),
      Home: () => moveSelection(-1e9), End: () => moveSelection(1e9),
      Enter: () => ($('detail').hidden ? openDetail() : closeDetail()),
      ' ': () => view.selected != null && toggleEnabled(view.selected),
      m: () => {
        const rows = selectedRows();
        if (rows.length > 1) bulk(rows.some((r) => r.enabled) ? 'mute' : 'unmute', rows.map((r) => r.id));
        else if (view.selected != null) toggleEnabled(view.selected);
      },
      s: () => store.counts.total && control(store.running || store.measureEndsAt != null ? 'stop' : 'start'),
      p: () => store.counts.total && control('ping_now'),
      h: () => openHosts(),
      a: () => openAdd(),
      Insert: () => openAdd(),
      o: () => openPorts(),
      ',': () => openSettings(),
      Delete: () => { const rows = selectedRows(); if (rows.length) removeHosts(rows); },
      g: () => { prefs.grouped = !prefs.grouped; savePrefs(); schedule(); },
      t: toggleTheme,
      '?': () => $('dlg-keys').showModal(),
      F1: () => $('dlg-keys').showModal(),
    };
    if (actions[key]) { e.preventDefault(); actions[key](); }
  });

  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => { if (prefs.theme === 'system') { applyTheme(); drawChart(); } });
  setInterval(() => { if (!document.hidden) schedule(false); }, 1000);
}

// ------------------------------------------------------------------ boot

if (!token) {
  $('gate').hidden = false;
} else {
  layoutColumns();
  wire();
  applyTheme();
  loadSource().then(() => schedule(false));
  refreshConfig();
  api('info').then((info) => { appInfo = info; schedule(false); }).catch(() => {});
  poll();
}
