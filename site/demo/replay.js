/* pingtable.net demo: a real pingtable run, replayed through the real app.
 *
 * site/demo/index.html is the app's own page (copied from ui/ by
 * scripts/build-site-demo.sh). This file loads first and replaces
 * window.fetch for /api/v1/*, so the unmodified app.js runs against a
 * recording instead of a pingtable on this computer. Nothing is sent
 * anywhere: the recording is recording.js, made by scripts/record-demo.py.
 *
 * Recording format 1 (window.PINGTABLE_RECORDING)
 *
 *   recorded_on   UTC date of the recording, "YYYY-MM-DD".
 *   warmup_ms     How long pingtable had been pinging when the recording
 *                 started. That moment is t = 0.
 *   duration_ms   Length of the recording. The replay then starts again.
 *   time_keys     Field names that hold a time of day (`last_at_ms`,
 *                 `t_ms`, ...). In the recording every such time is stored
 *                 as milliseconds after t = 0 (negative in the warm-up).
 *                 The replay adds a base so that t = 0 is the moment the
 *                 current loop started: "ago" labels, clocks and charts
 *                 read as now, and each loop starts fresh.
 *   statuses      Status names. A sample's status is stored as one letter,
 *                 "oabcdefghijklmnpqrstuvwxyz"[index]; "o" is "ok".
 *   labels        Status name to `last_status_label`.
 *   derived       Row fields worked out from the host's history instead of
 *                 being stored with every update (the counters of HostStats
 *                 in crates/core/src/engine.rs; see derive() below), and the
 *                 spark. The recorder checks that this gives exactly what
 *                 the app sent and stores any value it would not.
 *   history[id]   Every sample of one host, as columns: `t` time steps in
 *                 ms (the first from t = 0), `s` one status letter per
 *                 sample, `r` the round trip in microseconds of each OK
 *                 sample in turn, `ttl` [count, ttl] runs, and `b`
 *                 {index: beats} for a sample that stood for several beats.
 *   initial       The table at t = 0: `top` holds the top-level fields of
 *                 GET /state (running, counts, active, ...); `rows` are
 *                 [id, n, fields]: the row covers the host's first n
 *                 samples (-1: derive nothing), `fields` are all the others.
 *   timeline      What changed, in time order. Every entry starts with the
 *                 milliseconds since the previous one:
 *                   [dt, 0, id, n, fields?]  a row: it now covers n samples;
 *                                            fields changed (or are given
 *                                            because the history cannot)
 *                   [dt, 1, event]           a host went down or came back
 *                   [dt, 2, fields]          top-level fields changed
 *                   [dt, 3, note]            a note for the person watching
 *   static        Replies recorded once: GET info, config, source and
 *                 defaults and POST parse for the recorded host list.
 *
 * URL parameters. ?theme=dark or ?theme=light picks the look (inside the
 * pingtable.net page the demo then follows the page's own switch), and
 * ?tour=0 skips the opening tour. Three test parameters work only on
 * localhost, 127.0.0.1 and file: pages: ?speed=10 plays ten times as fast
 * (0.1 to 50), ?at=300 starts 300 seconds in, ?loop=60 starts again after
 * 60 seconds (10 seconds up to the recording's length). Anything else, and
 * any value that is not exactly one of these, is ignored.
 *
 * Containment. The page frames the demo with
 * sandbox="allow-scripts allow-downloads", and every /demo/ response carries
 * the same CSP sandbox, so the demo runs in an opaque origin: it cannot touch
 * pingtable.net, its storage or its cookies, open windows, submit forms or
 * navigate the page. Its Content-Security-Policy (a header, and a <meta> that
 * scripts/build-site-demo.sh adds) lets nothing leave: connect-src 'none',
 * no frames, workers or plug-ins, scripts from this folder only. The code at
 * the end of this file makes the unmodified app work inside that and keeps
 * whatever it reaches for inside the page: storage lives in memory, dialog
 * forms close here, links, window.open and any fetch outside /api/v1/ go
 * nowhere, and HTML passes a Trusted Types policy that refuses markup that
 * could run code or navigate.
 */

(function () {
  'use strict';

  var LETTERS = 'oabcdefghijklmnpqrstuvwxyz';
  var STRIDE = 1e7; // seq numbers: generation * STRIDE + entries seen + 1
  var EVENT_MAX_AGE = 20000; // events older than this (a tab that slept) are not announced
  var f32 = Math.fround;

  // ------------------------------------------------------------ decoding

  function decodeHistory(enc, statuses) {
    var out = [];
    var ttls = [];
    enc.ttl.forEach(function (run) { for (var j = 0; j < run[0]; j++) ttls.push(run[1]); });
    var t = 0, ri = 0;
    for (var i = 0; i < enc.s.length; i++) {
      t += enc.t[i];
      var status = statuses[LETTERS.indexOf(enc.s[i])];
      var s = { t: t, status: status, rtt: null, ttl: ttls[i] == null ? null : ttls[i], beats: 1 };
      if (status === 'ok') s.rtt = enc.r[ri++];
      if (enc.b && enc.b[i] != null) s.beats = enc.b[i];
      out.push(s);
    }
    return out;
  }

  // A port of HostStats::apply in crates/core/src/engine.rs.
  function newStats() {
    return { n: 0, sent: 0, ok: 0, failed: 0, missed: 0, consecFailed: 0, consecOk: 0, maxConsecFailed: 0,
      maxConsecFailedAt: null, last: null, lastOkAt: null, lastFailedAt: null, min: null, max: null,
      sum: 0, timed: 0, jitter: 0, prev: null };
  }

  function applySample(st, s) {
    st.sent++;
    st.last = s;
    if (s.status === 'ok') {
      st.ok++;
      st.consecOk++;
      st.consecFailed = 0;
      st.lastOkAt = s.t;
      if (s.rtt != null) {
        st.sum += s.rtt;
        st.timed++;
        st.min = st.min == null ? s.rtt : Math.min(st.min, s.rtt);
        st.max = st.max == null ? s.rtt : Math.max(st.max, s.rtt);
        if (st.prev != null) {
          var diff = Math.abs(f32(f32(s.rtt) - f32(st.prev)));
          st.jitter = st.timed === 2 ? diff : f32(st.jitter + f32(f32(diff - st.jitter) / 16));
        }
        st.prev = s.rtt;
      }
    } else {
      st.failed++;
      st.missed += Math.max(0, s.beats - 1);
      st.consecFailed++;
      st.consecOk = 0;
      st.lastFailedAt = s.t;
      if (st.consecFailed >= st.maxConsecFailed) {
        st.maxConsecFailed = st.consecFailed;
        st.maxConsecFailedAt = s.t;
      }
    }
    st.n++;
  }

  function createReplay(rec, opts) {
    opts = opts || {};
    var now = opts.now || Date.now;
    var speed = opts.speed || 1;
    var statuses = rec.statuses;
    var timeKeys = {};
    rec.time_keys.forEach(function (k) { timeKeys[k] = true; });
    var derivedKeys = rec.derived;
    var sparkLen = rec.spark_len || 60;
    var loopMs = Math.max(1000, opts.loopMs ? Math.min(opts.loopMs, rec.duration_ms) : rec.duration_ms);
    var settings = (rec.static.config && rec.static.config.settings) || {};
    var historyCap = settings.history_per_host || 2000;

    var hist = {};
    Object.keys(rec.history).forEach(function (id) { hist[id] = decodeHistory(rec.history[id], statuses); });
    var stats = {}; // id -> running stats, moved forward as rows advance

    function derive(id, n) {
      var samples = hist[id] || [];
      var st = stats[id];
      if (!st || st.n > n) st = stats[id] = newStats();
      while (st.n < n && st.n < samples.length) applySample(st, samples[st.n]);
      var recent = samples.slice(Math.max(0, n - sparkLen), n);
      var beats = 0, lost = 0;
      recent.forEach(function (s) { beats += s.beats; if (s.status !== 'ok') lost += s.beats; });
      var total = st.sent + st.missed;
      var last = st.last;
      return {
        sent: st.sent, ok: st.ok, failed: st.failed, missed: st.missed,
        failed_pct: total === 0 ? 0 : f32(f32(f32(st.failed + st.missed) * 100) / f32(total)),
        recent_loss_pct: beats === 0 ? 0 : f32(f32(f32(lost) * 100) / f32(beats)),
        consec_failed: st.consecFailed, max_consec_failed: st.maxConsecFailed,
        max_consec_failed_at_ms: st.maxConsecFailedAt,
        last_status: last ? last.status : null,
        last_status_label: last ? (rec.labels[last.status] || last.status) : null,
        last_rtt_us: last ? last.rtt : null,
        last_ttl: last ? last.ttl : null,
        last_at_ms: last ? last.t : null,
        last_ok_at_ms: st.lastOkAt, last_failed_at_ms: st.lastFailedAt,
        avg_rtt_us: st.timed ? Math.floor(st.sum / st.timed) : null,
        min_rtt_us: st.min, max_rtt_us: st.max,
        jitter_us: st.timed > 1 ? Math.floor(st.jitter + 0.5) : null,
        spark: recent.map(function (s) { return s.status === 'ok' ? s.rtt : null; }),
      };
    }

    function updateRow(row, n, fields) {
      if (fields) for (var k in fields) row[k] = fields[k];
      row._n = n;
      if (n < 0) return;
      var d = derive(row.id, n);
      derivedKeys.forEach(function (key) { if (!fields || !(key in fields)) row[key] = d[key]; });
    }

    // The table at t = 0, kept to start each loop from.
    var initialRows = rec.initial.rows.map(function (r) {
      var row = { id: r[0] };
      updateRow(row, r[1], r[2]);
      return row;
    });
    var pristine = JSON.stringify({ rows: initialRows, top: rec.initial.top });

    var timeline = [];
    var t = 0;
    rec.timeline.forEach(function (e) {
      t += e[0];
      if (t > loopMs) return;
      if (e[1] === 0) timeline.push({ t: t, kind: 0, id: e[2], n: e[3], fields: e[4] || null });
      else timeline.push({ t: t, kind: e[1], data: e[2] });
    });

    // Where the first loop starts: given, or a few seconds before the first host goes down,
    // so that something happens soon after the demo comes into view.
    var startAt = opts.startAt;
    if (startAt == null) {
      startAt = 0;
      for (var j = 0; j < timeline.length; j++) {
        var first = timeline[j];
        if (first.kind === 1 && first.data.kind === 'down') { startAt = Math.max(0, first.t - (opts.lead == null ? 4000 : opts.lead)); break; }
      }
    }
    startAt = Math.min(startAt, loopMs - 1);

    var pageStart = now();
    var held = !!opts.hold; // time stands still until release(): the demo is not in view yet
    var firstBase = null;
    var generation = 0;
    var waiters = [];
    var recentDown = null;
    var cur = { loop: -1, i: 0, rows: null, order: [], top: null, changedAt: {} };

    function reset(loop) {
      var copy = JSON.parse(pristine);
      generation++;
      cur.loop = loop;
      cur.i = 0;
      cur.rows = {};
      cur.changedAt = {};
      copy.rows.forEach(function (row) { cur.rows[row.id] = row; cur.changedAt[row.id] = -1; });
      cur.order = copy.rows.map(function (row) { return row.id; });
      cur.top = copy.top;
    }

    function clock() {
      var wall = now();
      if (held) return { wall: wall, loop: 0, vt: startAt, base: wall - startAt, held: true };
      var elapsed = (wall - pageStart) * speed + startAt;
      var loop = Math.floor(elapsed / loopMs);
      var vt = elapsed - loop * loopMs;
      var base = wall - vt;
      if (firstBase == null) firstBase = base;
      return { wall: wall, loop: loop, vt: vt, base: base };
    }

    /** Start the clock (the demo came into view). The app's next poll gets a fresh table. */
    function release() {
      if (!held) return;
      held = false;
      pageStart = now();
      cur.loop = -1;
      var w = waiters;
      waiters = [];
      w.forEach(function (wake) { wake(); });
    }

    /** Move the replay to the present; returns the clock. */
    function sync() {
      var c = clock();
      if (c.loop !== cur.loop) reset(c.loop);
      while (cur.i < timeline.length && timeline[cur.i].t <= c.vt) {
        var e = timeline[cur.i];
        if (e.kind === 0) {
          var row = cur.rows[e.id];
          if (row) { updateRow(row, e.n, e.fields); cur.changedAt[e.id] = cur.i; }
        } else if (e.kind === 2) {
          for (var k in e.data) cur.top[k] = e.data[k];
        }
        cur.i++;
      }
      return c;
    }

    function shift(obj, base) {
      var out = {};
      for (var k in obj) {
        if (k.charAt(0) === '_') continue;
        var v = obj[k];
        out[k] = timeKeys[k] && typeof v === 'number' ? Math.round(v + base) : v;
      }
      return out;
    }

    function seqOf() { return generation * STRIDE + cur.i + 1; }

    function snapshot(c, seen) {
      var full = seen == null;
      var top = shift(cur.top, c.base);
      if (cur.top.running_since_ms != null) top.running_since_ms = Math.round(cur.top.running_since_ms + (firstBase != null ? firstBase : c.base));
      var hosts = [];
      cur.order.forEach(function (id) {
        if (full || cur.changedAt[id] >= seen) hosts.push(shift(cur.rows[id], c.base));
      });
      var events = [], notes = [];
      if (!full) {
        for (var i = seen; i < cur.i; i++) {
          var e = timeline[i];
          if (e.kind === 1 && c.vt - e.t <= EVENT_MAX_AGE) {
            var ev = shift(e.data, c.base);
            ev.seq = generation * STRIDE + i + 1;
            events.push(ev);
            recentDown = e.data.kind === 'down' ? e.data.host_id : recentDown === e.data.host_id ? null : recentDown;
          } else if (e.kind === 3) {
            notes.push(shift(e.data, c.base));
          }
        }
      }
      var out = { schema_version: 1, seq: seqOf(), now_ms: Math.round(c.wall) };
      for (var k in top) out[k] = top[k];
      out.full = full;
      out.hosts = hosts;
      out.events = events;
      out.notes = notes;
      if (!out.warnings) out.warnings = [];
      return out;
    }

    /** Wait ms, or until release(). */
    function sleep(ms) {
      return new Promise(function (resolve) {
        var done = false;
        var wake = function () { if (!done) { done = true; resolve(); } };
        setTimeout(wake, ms);
        if (held) waiters.push(wake);
      });
    }

    /** GET /state?since=…&wait_ms=…, as the app's long poll uses it. */
    function state(since, waitMs) {
      var c = sync();
      var gen = generation;
      var seen = since - gen * STRIDE - 1;
      if (!(seen >= 0) || seen > cur.i) return Promise.resolve(snapshot(c, null));
      if (seen < cur.i || !(waitMs > 0)) return Promise.resolve(snapshot(c, seen));
      // Nothing new yet: wait for the next change, as the real server does.
      var next = cur.i < timeline.length ? timeline[cur.i].t : loopMs;
      // While time stands still, a fresh table every second keeps "ago" labels where they were.
      var ms = c.held ? Math.min(waitMs, 1000) : Math.min(waitMs, Math.max(0, (next - c.vt) / speed) + 40);
      return sleep(ms).then(function () {
        var c2 = sync();
        return snapshot(c2, generation !== gen || c2.held ? null : seen);
      });
    }

    /** The samples a row covers: its first n, at most as many as the app keeps. */
    function samplesOf(id) {
      var row = cur.rows[id];
      var samples = hist[id] || [];
      var n = row._n;
      if (n < 0) {
        n = 0;
        while (n < samples.length && row.last_at_ms != null && samples[n].t <= row.last_at_ms) n++;
      }
      return samples.slice(Math.max(0, n - historyCap), n);
    }

    /** GET /hosts/{id}: the row, its history, runs, percentiles and loss windows. */
    function detail(id, c) {
      c = c || sync();
      if (!cur.rows[id]) return null;
      return detailOf(id, samplesOf(id), c.vt, c.base);
    }

    function detailOf(id, h, nowRel, base) {
      var runs = [];
      h.forEach(function (s) {
        var last = runs[runs.length - 1];
        if (last && last.status === s.status) { last.end_ms = s.t; last.count++; }
        else runs.push({ status: s.status, start_ms: s.t, end_ms: s.t, count: 1 });
      });
      if (runs.length > 500) runs = runs.slice(runs.length - 500);
      var times = [];
      h.forEach(function (s) { if (s.rtt != null) times.push(s.rtt); });
      times.sort(function (a, b) { return a - b; });
      function pct(p) {
        if (!times.length) return null;
        var rank = Math.ceil((p / 100) * times.length);
        return times[Math.min(Math.max(rank, 1), times.length) - 1];
      }
      var kept = h.length ? Math.max(0, nowRel - h[0].t) : 0;
      var windows = [['1m', 60000], ['5m', 300000], ['1h', 3600000], ['24h', 86400000]].map(function (w) {
        var sent = 0, failed = 0, sum = 0, timed = 0;
        for (var i = h.length - 1; i >= 0 && nowRel - h[i].t <= w[1]; i--) {
          sent += h[i].beats;
          if (h[i].status !== 'ok') failed += h[i].beats;
          if (h[i].rtt != null) { sum += h[i].rtt; timed++; }
        }
        return { label: w[0], sent: sent, failed: failed, loss_pct: sent ? f32(f32(f32(failed) * 100) / f32(sent)) : 0,
          avg_rtt_us: timed ? Math.floor(sum / timed) : null, covered_ms: Math.round(Math.min(kept, w[1])) };
      });
      return {
        schema_version: 1,
        row: cur.rows[id] ? shift(cur.rows[id], base) : null,
        history: h.map(function (s) {
          var out = { t_ms: Math.round(s.t + base), status: s.status };
          if (s.rtt != null) out.rtt_us = s.rtt;
          if (s.ttl != null) out.ttl = s.ttl;
          if (s.beats !== 1) out.beats = s.beats;
          return out;
        }),
        runs: runs.map(function (r) {
          return { status: r.status, start_ms: Math.round(r.start_ms + base), end_ms: Math.round(r.end_ms + base), count: r.count };
        }),
        p50_rtt_us: pct(50), p95_rtt_us: pct(95), p99_rtt_us: pct(99),
        windows: windows,
      };
    }

    function rows(c) {
      c = c || sync();
      return cur.order.map(function (id) { return shift(cur.rows[id], c.base); });
    }

    return {
      rec: rec, loopMs: loopMs, timeline: timeline, history: hist,
      clock: clock, sync: sync, state: state, detail: detail, rows: rows, release: release,
      startAt: startAt,
      /** The host that went down most recently and has not come back, if any. */
      recentDown: function () { return recentDown; },
      // For tests: a detail from the first n samples, as of nowRel ms after t = 0.
      detailAt: function (id, n, nowRel) { sync(); return detailOf(id, (hist[id] || []).slice(0, n), nowRel, 0); },
      rowAt: function (id) { return cur.rows[id]; },
      set: function (o) { if (o.now) now = o.now; if (o.pageStart != null) pageStart = o.pageStart; cur.loop = -1; firstBase = null; },
    };
  }

  window.PINGTABLE_DEMO = { createReplay: createReplay, decodeHistory: decodeHistory };
  if (typeof document === 'undefined' || typeof location === 'undefined') return; // tests in Node stop here

  // ----------------------------------------------------- the demo page

  // This runs before theme.js and app.js, so everything below is in place before the app starts.

  var SELF = location.protocol + '//' + location.host; // where the demo was served from
  var DEBUG = location.protocol === 'file:' || /^(localhost|127\.0\.0\.1|\[::1\])$/.test(location.hostname);
  var embedded = false;
  try { embedded = window.parent !== window; } catch (e) { embedded = true; }

  // HTML. With the CSP's require-trusted-types-for 'script', every string the app puts into
  // innerHTML (or any other HTML sink) passes this policy first. The app escapes what it shows,
  // so markup that could run code, load a document, submit or navigate can only come from a
  // mistake; such a string is shown as plain text instead. (Browsers without Trusted Types
  // still have the CSP.)
  var UNSAFE_HTML = [
    /<\/?(?:script|style|link|meta|base|i?frame|frameset|object|embed|applet|form|portal|fencedframe|template|noscript|math|foreignobject|animate|animatemotion|animatetransform|set|use)(?![\w-])/i,
    /[\s\/"'](?:on[a-z]+|srcdoc|formaction|action|http-equiv|ping)\s*=/i,
    /[\s\/"'](?:xlink:)?href\s*=\s*(?!["']?(?:#|blob:|https:\/\/github\.com\/lorincz-llc\/pingtable(?:[\/#?"'\s>]|$)))/i,
  ];
  var refusedHTML = 0;
  function asText(s) { return s.replace(/[&<>"']/g, function (c) { return '&#' + c.charCodeAt(0) + ';'; }); }
  if (window.trustedTypes && window.trustedTypes.createPolicy) {
    try {
      window.trustedTypes.createPolicy('default', {
        createHTML: function (html) {
          html = String(html);
          for (var i = 0; i < UNSAFE_HTML.length; i++) {
            if (UNSAFE_HTML[i].test(html)) { refusedHTML++; return asText(html); }
          }
          return html;
        },
      });
    } catch (e) {}
  }

  // Storage. In the sandbox localStorage throws, and the demo should not keep anything anyway:
  // the app gets a store in memory, which is gone when the page closes. The app asks for its
  // access token before it does anything; any will do here, so it is in the store already.
  function memoryStorage(seed) {
    var data = Object.create(null);
    var store = {
      getItem: function (k) { k = String(k); return k in data ? data[k] : null; },
      setItem: function (k, v) { data[String(k)] = String(v); },
      removeItem: function (k) { delete data[String(k)]; },
      clear: function () { data = Object.create(null); },
      key: function (i) { var keys = Object.keys(data); return keys[i] == null ? null : keys[i]; },
    };
    Object.defineProperty(store, 'length', { get: function () { return Object.keys(data).length; } });
    for (var k in seed) store.setItem(k, seed[k]);
    return store;
  }
  var storage = memoryStorage({ 'pingtable.token': 'demo' });
  [['localStorage', storage], ['sessionStorage', memoryStorage({})]].forEach(function (s) {
    try { Object.defineProperty(window, s[0], { configurable: true, enumerable: true, get: function () { return s[1]; } }); } catch (e) {}
  });

  // URL parameters: exactly one value each, and only the values listed; nothing else is read.
  var params = new URLSearchParams(location.search);
  function one(name) { var all = params.getAll(name); return all.length === 1 ? all[0] : null; }
  function number(name, min, max) {
    var v = DEBUG ? one(name) : null;
    if (v == null || !/^\d{1,6}(?:\.\d{1,3})?$/.test(v)) return null;
    return Math.min(max, Math.max(min, Number(v)));
  }
  var theme = one('theme');
  var OPTIONS = Object.freeze({
    debug: DEBUG,
    theme: theme === 'dark' || theme === 'light' ? theme : null,
    tour: one('tour') !== '0',
    speed: number('speed', 0.1, 50) || 1,
    at: number('at', 0, 86400), // seconds; engine() keeps both within the recording
    loop: number('loop', 10, 86400),
  });
  window.PINGTABLE_DEMO.options = OPTIONS;
  window.PINGTABLE_DEMO.refusedHTML = function () { return refusedHTML; };

  // Light or dark: ?theme= when given, then whatever the page around the demo says.
  var wanted = OPTIONS.theme;
  if (wanted) {
    storage.setItem('pingtable.theme', wanted);
    storage.setItem('pingtable.ui', JSON.stringify({ theme: wanted }));
  }
  function follow(look) {
    if (look !== 'dark' && look !== 'light') return;
    if (document.documentElement.dataset.theme === look) return;
    var button = document.getElementById('btn-theme');
    if (button) button.click(); // the app's own switch, so it saves and redraws as usual
  }
  // The page posts {pingtableTheme: 'light' | 'dark'} to '*' (this frame's origin is opaque).
  // Only that page counts: the frame's parent, from the origin that served this file, with
  // exactly that one field.
  var PAGE_ORIGIN = location.protocol === 'file:' ? 'null' : SELF;
  window.addEventListener('message', function (e) {
    if (!embedded || e.source !== window.parent || e.origin !== PAGE_ORIGIN) return;
    var d = e.data;
    if (!d || typeof d !== 'object' || Object.getPrototypeOf(d) !== Object.prototype) return;
    var keys = Object.keys(d);
    if (keys.length !== 1 || keys[0] !== 'pingtableTheme') return;
    if (d.pingtableTheme === 'light' || d.pingtableTheme === 'dark') follow(d.pingtableTheme);
  });

  // A short notice in the app's own toast style.
  var lastNotice = { text: '', at: 0 };
  function notice(text) {
    var full = text;
    if (Array.isArray(text)) { full = text[1]; text = text[0]; }
    var box = document.getElementById('toasts');
    if (!box || (text === lastNotice.text && Date.now() - lastNotice.at < 2000)) return;
    lastNotice = { text: text, at: Date.now() };
    var el = document.createElement('div');
    el.className = 'toast up';
    el.title = full;
    var strong = document.createElement('b');
    strong.textContent = 'DEMO';
    var span = document.createElement('span');
    span.textContent = text;
    el.appendChild(strong);
    el.appendChild(span);
    box.appendChild(el);
    while (box.children.length > 4) box.removeChild(box.firstChild);
    setTimeout(function () {
      el.classList.add('is-leaving');
      setTimeout(function () { if (el.parentNode) el.parentNode.removeChild(el); }, 220);
    }, 6000);
  }

  var replay = null;
  function engine() {
    if (replay) return replay;
    var rec = window.PINGTABLE_RECORDING;
    if (!rec || rec.format !== 1) throw new Error('The recording did not load.');
    replay = createReplay(rec, {
      speed: OPTIONS.speed,
      startAt: OPTIONS.at != null ? Math.min(OPTIONS.at * 1000, rec.duration_ms) : null,
      loopMs: OPTIONS.loop != null ? Math.min(OPTIONS.loop * 1000, rec.duration_ms) : 0,
      hold: embedded, // inside the site, time starts when the demo comes into view
    });
    return replay;
  }
  // For tests: the clock the replay settled on (numbers only).
  window.PINGTABLE_DEMO.timing = function () { return replay ? { loopMs: replay.loopMs, startAt: replay.startAt } : null; };

  function reply(status, body, headers) {
    var text = typeof body === 'string' ? body : JSON.stringify(body);
    var h = { 'Content-Type': 'application/json' };
    for (var k in headers || {}) h[k] = headers[k];
    return new Response(text, { status: status, headers: h });
  }
  var ok = function (body) { return reply(200, body || { ok: true }); };
  var refuse = function (text) { return reply(409, { error: text }); };

  var RECORDING = 'This is a recording, so ';
  // A toast is one short line; the whole sentence is its tooltip.
  var KEEPS = ['A recording: it keeps playing.', RECORDING + 'it keeps playing. In pingtable, this starts and stops the pinging.'];
  var AS_RECORDED = ['A recording: hosts stay as recorded.', RECORDING + 'every host stays as it was recorded. In pingtable this acts on the hosts at once.'];
  var WRITE_NOTICE = {
    start: KEEPS, stop: KEEPS,
    ping_now: ['A recording: nothing is pinged here.', 'This is a recording: nothing is pinged from your browser. In pingtable, this pings at once.'],
    reset: ['A recording: counters stay as recorded.', RECORDING + 'the counters stay as they were recorded.'],
    mute: AS_RECORDED, unmute: AS_RECORDED, unmute_all: AS_RECORDED, solo: AS_RECORDED,
    remove: ['A recording: the list stays as it was.', RECORDING + 'the host list stays as it was.'],
  };
  var NOT_HERE = {
    list: RECORDING + 'the host list and settings cannot be changed here. Get pingtable to watch your own hosts.',
    import: RECORDING + 'a file cannot be imported here. Get pingtable to watch your own hosts.',
    trace: 'Trace route is not part of the demo: it would show the way out of the home network it was recorded on. In pingtable it takes a second or two.',
    measure: RECORDING + 'nothing new can be measured. In pingtable, Measure pings the chosen hosts up to ten times a second for a set time.',
  };

  function readBody(body) {
    if (!body) return {};
    try { return JSON.parse(body); } catch (e) { return {}; }
  }

  function csvCell(text) {
    text = text == null ? '' : String(text);
    if (/^[=+\-@\t\r]/.test(text)) text = "'" + text;
    return /[,"\n\r]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text;
  }

  function exportTable(format) {
    var rows = engine().rows();
    if (format === 'csv') {
      var ms = function (us) { return us == null ? '' : (us / 1000).toFixed(3); };
      var time = function (t) { return t == null ? '' : new Date(t).toISOString(); };
      var lines = ['host,ip,description,group,tags,probe,state,last_status,last_rtt_ms,last_ttl,avg_rtt_ms,min_rtt_ms,max_rtt_ms,jitter_ms,sent,ok,failed,failed_pct,recent_loss_pct,consec_failed,max_consec_failed,last_ok_at,last_failed_at,reply_ip,hostname,mac,interval_secs,timeout_ms'];
      rows.forEach(function (r) {
        lines.push([csvCell(r.host), csvCell(r.ip), csvCell(r.description), csvCell(r.group), csvCell((r.tags || []).join(' ')),
          r.probe.kind === 'tcp' ? 'tcp:' + r.probe.port : 'icmp', r.state, csvCell(r.last_status_label), ms(r.last_rtt_us),
          r.last_ttl == null ? '' : r.last_ttl, ms(r.avg_rtt_us), ms(r.min_rtt_us), ms(r.max_rtt_us), ms(r.jitter_us),
          r.sent, r.ok, r.failed, r.failed_pct.toFixed(2), r.recent_loss_pct.toFixed(2), r.consec_failed, r.max_consec_failed,
          time(r.last_ok_at_ms), time(r.last_failed_at_ms), csvCell(r.reply_ip), csvCell(r.hostname), r.mac || '',
          r.interval_secs, r.timeout_ms].join(','));
      });
      return reply(200, lines.join('\r\n') + '\r\n', { 'Content-Type': 'text/csv; charset=utf-8' });
    }
    return reply(200, JSON.stringify({ schema_version: 1, exported_at: new Date().toISOString(), hosts: rows }, null, 2));
  }

  /** One API call, answered from the recording. */
  function answer(path, query, method, body) {
    var r = engine();
    var s = r.rec.static;
    var parts = path.split('/');
    var b = readBody(body);
    if (method === 'GET') {
      switch (parts[0]) {
        case 'state': return r.state(Number(query.get('since')) || 0, Number(query.get('wait_ms')) || 0).then(ok);
        case 'hosts': {
          var d = r.detail(Number(parts[1]));
          return d ? ok(d) : reply(404, { error: 'No such host.' });
        }
        case 'info': return ok(s.info);
        case 'source': return ok(s.source);
        case 'defaults': return ok(s.defaults);
        case 'config': return ok(s.config);
        case 'measure': return ok({ measurement: null });
        case 'export': return exportTable(query.get('format'));
      }
    } else if (method === 'PUT' && parts[0] === 'config') {
      return refuse(NOT_HERE.list);
    } else if (method === 'POST') {
      switch (parts[0]) {
        case 'control':
          notice(WRITE_NOTICE[b.action] || WRITE_NOTICE.ping_now);
          return ok();
        case 'hosts':
          if (parts[1] === 'add') return refuse(NOT_HERE.list);
          if (parts[2] === 'trace') return refuse(NOT_HERE.trace);
          notice(WRITE_NOTICE[b.action] || (b.enabled === false ? WRITE_NOTICE.mute : WRITE_NOTICE.unmute));
          return ok();
        case 'measure':
          if (parts[1] === 'stop') return ok({ measurement: null });
          return refuse(NOT_HERE.measure);
        case 'parse':
          if (s.parse && b.text === s.parse.body.text) return ok(s.parse.reply);
          return refuse(NOT_HERE.list);
        case 'import': return refuse(NOT_HERE.import);
      }
    }
    return reply(404, { error: 'Not part of the demo.' });
  }

  // The app's API calls are answered here; any other fetch fails at once, without a request.
  var NOTHING_SENT = 'Not part of the demo: it sends nothing anywhere.';
  window.fetch = function (input, init) {
    var href = input && typeof input === 'object' && 'url' in input ? input.url : String(input);
    var url = null;
    try { url = new URL(href, location.href); } catch (e) {}
    var at = url && url.protocol + '//' + url.host === SELF ? url.pathname.indexOf('/api/v1/') : -1;
    if (at < 0) return Promise.reject(new TypeError(NOTHING_SENT));
    var method = String((init && init.method) || (input && input.method) || 'GET').toUpperCase();
    return Promise.resolve()
      .then(function () { return answer(url.pathname.slice(at + 8), url.searchParams, method, init && init.body); })
      .catch(function (err) { return reply(500, { error: String(err && err.message || err) }); });
  };

  // Everything else that could send something, for code that tries anyway. The CSP blocks all
  // of these too (WebRTC excepted, which no CSP covers); this answers at once and says why.
  function refusing(name) {
    return function () { throw new DOMException(name + ': ' + NOTHING_SENT, 'SecurityError'); };
  }
  ['XMLHttpRequest', 'WebSocket', 'WebSocketStream', 'EventSource', 'WebTransport', 'fetchLater',
    'RTCPeerConnection', 'webkitRTCPeerConnection'].forEach(function (name) {
    if (name in window) try { window[name] = refusing(name); } catch (e) {}
  });
  try { navigator.sendBeacon = function () { return false; }; } catch (e) {}

  // Windows and links. The app's "Open http://..." would send a visitor's browser to an address
  // on the recording's home network; the sandbox blocks popups, and this says so instead.
  var OPENS_NOTHING = ['A recording: nothing opens from here.', RECORDING + 'nothing opens from the demo. In pingtable, this opens the host in your browser.'];
  var NO_LINKS = ['A recording: links do not open here.', RECORDING + 'its links do not open. The pingtable.net page around it links to GitHub.'];
  function noWindow() { notice(OPENS_NOTHING); return null; }
  try { Object.defineProperty(window, 'open', { value: noWindow, writable: false, configurable: false }); } catch (e) { window.open = noWindow; }
  function guardLink(e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href], area[href]') : null;
    if (!a) return;
    // A link within the page, and the files the app saves (blob: URLs it makes itself), are fine.
    if ((a.getAttribute('href') || '').charAt(0) === '#' || /^blob:/.test(String(a.href))) return;
    e.preventDefault();
    if (e.type === 'click') notice(NO_LINKS);
  }
  document.addEventListener('click', guardLink, true);
  document.addEventListener('auxclick', guardLink, true);

  // Forms. The sandbox blocks every form submission, the app's <form method="dialog"> too
  // (its Close and Cancel buttons). Do here what method="dialog" does: check the fields, fire
  // submit, close the window with the button's value. Any other form sends nothing.
  function submitHere(form, submitter, fromScript) {
    var method = ((submitter && submitter.getAttribute('formmethod')) || form.getAttribute('method') || 'get').toLowerCase();
    if (method !== 'dialog') { notice(['A recording: nothing is sent.', RECORDING + 'nothing is sent anywhere from it.']); return; }
    if (!fromScript) {
      if (!form.noValidate && !(submitter && submitter.formNoValidate) && !form.reportValidity()) return;
      var ev;
      try { ev = new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: submitter || null }); } catch (er) { ev = new Event('submit', { bubbles: true, cancelable: true }); }
      if (!form.dispatchEvent(ev)) return;
    }
    var dialog = form.closest('dialog');
    if (dialog && dialog.open) dialog.close(submitter && submitter.hasAttribute('value') ? submitter.value : undefined);
  }
  // After the app's own handlers (bubbling, on window), unless one of them cancelled the click.
  // Enter in a field "clicks" the form's first submit button, so it arrives here as well.
  window.addEventListener('click', function (e) {
    if (e.defaultPrevented) return;
    var b = e.target && e.target.closest ? e.target.closest('button, input') : null;
    if (!b || b.disabled || !b.form || (b.type !== 'submit' && b.type !== 'image')) return;
    e.preventDefault();
    submitHere(b.form, b, false);
  });
  HTMLFormElement.prototype.submit = function () { submitHere(this, null, true); };
  HTMLFormElement.prototype.requestSubmit = function (submitter) { submitHere(this, submitter || null, false); };

  // The first seconds in view: a host goes down, then the real right-click menu opens on it
  // for a few seconds, to show what can be copied. Any real click or key stops this.
  var touched = false;
  var touch = !!(window.matchMedia && matchMedia('(pointer: coarse)').matches);
  ['pointerdown', 'keydown', 'wheel', 'touchstart'].forEach(function (type) {
    document.addEventListener(type, function (e) {
      if (!e.isTrusted) return;
      touched = true;
      if (replay) replay.release();
    }, { capture: true, passive: true });
  });

  function rowElement(id) {
    return id != null ? document.querySelector('#tbody [data-key="h:' + id + '"]') : null;
  }

  function showMenu() {
    if (touched || !OPTIONS.tour || !replay) return;
    // On a phone the menu would cover the whole table, and iOS has no right-click
    // to discover: say what a tap does instead and leave the table in view.
    if (touch || window.innerWidth < 720) {
      notice('Tap a row for its history.');
      return;
    }
    var grid = document.getElementById('grid');
    var el = rowElement(replay.recentDown()) || document.querySelector('#tbody .tr.is-down');
    if (!el || !grid) return;
    var box = el.getBoundingClientRect();
    var view = grid.getBoundingClientRect();
    if (box.top < view.top || box.bottom > view.bottom) return;
    var cell = el.children[1] || el;
    var at = cell.getBoundingClientRect();
    var x = Math.round(at.left + Math.min(60, at.width / 2)), y = Math.round(box.top + box.height / 2);
    // While the visitor's focus is out in the page, open the menu without taking it: the demo
    // is on the page's first screen, so the first Space or Page Down would otherwise go to the
    // menu instead of scrolling the page, and a keyboard user would lose their place. (The
    // sandboxed frame cannot see the page's focus; it does not need to, as it never takes it.)
    // The app focuses #ctx inside this dispatch.
    var ctx = embedded && !document.hasFocus() ? document.getElementById('ctx') : null;
    if (ctx) ctx.focus = function () {};
    try {
      cell.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y, button: 2 }));
    } finally {
      if (ctx) delete ctx.focus;
    }
    notice('Right-click a row to copy it.');
    setTimeout(function () {
      var menu = document.getElementById('ctx');
      if (touched || !menu) return;
      if (!menu.hidden) menu.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    }, 6500);
  }

  /** Call fn once, when at least `ratio` of the table is on screen (in the page around the frame, too). */
  function whenVisible(ratio, fn) {
    var grid = document.getElementById('grid');
    if (!embedded || !grid || typeof IntersectionObserver === 'undefined') { fn(); return; }
    var seen = new IntersectionObserver(function (entries) {
      if (!entries.some(function (e) { return e.intersectionRatio >= ratio; })) return;
      seen.disconnect();
      fn();
    }, { threshold: [ratio] });
    seen.observe(grid);
  }

  document.addEventListener('DOMContentLoaded', function () {
    if (wanted) follow(wanted);
    document.documentElement.classList.add('is-demo');
    var rec = window.PINGTABLE_RECORDING;
    // Say so for as long as the page is open: the footer carries a mark of its own.
    var foot = document.getElementById('foot');
    if (rec && foot) {
      var mark = document.createElement('span');
      mark.className = 'foot-item demo-mark';
      mark.title = 'This demo replays a run of pingtable recorded on ' + rec.recorded_on + '. Nothing is pinged from your browser.';
      mark.textContent = 'Recorded ' + rec.recorded_on + ', replayed';
      var ver = document.getElementById('foot-ver');
      foot.insertBefore(mark, ver && ver.parentNode === foot ? ver : null);
    }
    if (rec && !embedded) setTimeout(function () { notice('A recorded run, replayed here.'); }, 800);
    // Time starts when the table comes into view; the menu waits until most of it can be seen,
    // and until a few seconds after the first host goes down.
    var menuAt = null, menuReady = false;
    var maybeMenu = function () {
      if (menuAt == null || !menuReady) return;
      setTimeout(showMenu, Math.max(0, menuAt - Date.now()));
    };
    whenVisible(0.15, function () {
      try { engine().release(); } catch (e) { return; }
      var firstDown = replay.timeline.filter(function (e) { return e.kind === 1 && e.data.kind === 'down' && e.t >= replay.startAt; })[0];
      var lead = Math.min(firstDown ? firstDown.t - replay.startAt : 0, 8000) / OPTIONS.speed;
      menuAt = Date.now() + lead + 2500;
      maybeMenu();
    });
    whenVisible(0.6, function () { menuReady = true; maybeMenu(); });
  });
})();
