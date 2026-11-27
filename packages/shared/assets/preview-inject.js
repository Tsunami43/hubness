/*
 * hubness preview helper. The preview proxy adds it to every HTML page of a previewed app
 * (served from the app's own origin as /__hubness/preview.js), so it runs inside the app:
 *
 * - console errors and warnings, uncaught errors, rejected promises and failed requests go
 *   to the hub (POST /__hubness/log on the same proxy) — the Preview panel shows them and the
 *   agent reads them with preview_console;
 * - the page tells the panel around it where it is (address bar) and obeys back / forward /
 *   reload (the panel is another origin and can't touch the frame's history itself);
 * - "pick an element": hover outlines elements, a click reports a selector, the text and a
 *   bit of HTML to the panel, which drafts a message for the agent.
 *
 * Plain ES5-ish code with no dependencies; it must never break the app it lives in.
 */
(function () {
  if (window.__hubnessPreview) return;
  window.__hubnessPreview = true;
  var ENDPOINT = '/__hubness/log';
  var queue = [];
  var timer = null;
  var inFrame = window.parent !== window;

  function str(v) {
    try {
      if (v instanceof Error) return (v.stack || v.name + ': ' + v.message).slice(0, 4000);
      if (typeof v === 'string') return v;
      return JSON.stringify(v);
    } catch (e) {
      return String(v);
    }
  }

  function flush() {
    timer = null;
    if (!queue.length) return;
    var body = JSON.stringify(queue.splice(0, 50));
    // the hub takes 50 entries a request: the rest goes with the next one
    if (queue.length) timer = setTimeout(flush, 300);
    try {
      if (navigator.sendBeacon && navigator.sendBeacon(ENDPOINT, new Blob([body], { type: 'text/plain' }))) return;
    } catch (e) {}
    try {
      origFetch(ENDPOINT, { method: 'POST', body: body, keepalive: true, headers: { 'content-type': 'text/plain' } });
    } catch (e) {}
  }

  function report(level, text, extra) {
    if (queue.length > 200) return;
    var entry = { level: level, text: String(text).slice(0, 4000), url: location.href, ts: new Date().toISOString() };
    if (extra) for (var k in extra) entry[k] = extra[k];
    queue.push(entry);
    if (!timer) timer = setTimeout(flush, 300);
  }

  // ---- the panel around the page ----
  // Only a hubness panel on this very host is talked to (the preview is another port of the
  // hub's host): a foreign page that frames the app learns nothing and commands nothing.
  var panel = null; // the panel's origin once known
  var waiting = []; // messages for the panel until then

  function trusted(origin) {
    try {
      return new URL(origin).hostname === location.hostname;
    } catch (e) {
      return false;
    }
  }

  function meet(origin) {
    if (panel || !trusted(origin)) return;
    panel = origin;
    var list = waiting;
    waiting = [];
    for (var i = 0; i < list.length; i++) post(list[i]);
  }

  function post(msg) {
    if (!inFrame) return;
    msg.hubness = 1;
    if (!panel) {
      if (waiting.length < 50) waiting.push(msg);
      return;
    }
    try {
      window.parent.postMessage(msg, panel);
    } catch (e) {}
  }

  if (inFrame) {
    // Chromium and Safari name the frame's ancestors; elsewhere the panel says hello first
    var ancestors = location.ancestorOrigins;
    if (ancestors && ancestors.length) meet(ancestors[0]);
  }

  // ---- console and errors ----
  ['error', 'warn'].forEach(function (level) {
    var orig = console[level];
    console[level] = function () {
      try {
        report(level, Array.prototype.map.call(arguments, str).join(' '));
      } catch (e) {}
      return orig.apply(console, arguments);
    };
  });
  window.addEventListener('error', function (e) {
    if (e.target && e.target !== window && (e.target.src || e.target.href)) {
      report('network', 'Failed to load ' + (e.target.src || e.target.href));
      return;
    }
    report('error', e.error ? str(e.error) : e.message + (e.filename ? ' (' + e.filename + ':' + e.lineno + ')' : ''));
  }, true);
  window.addEventListener('unhandledrejection', function (e) {
    report('error', 'Unhandled rejection: ' + str(e.reason));
  });

  // ---- failed requests ----
  var origFetch = window.fetch ? window.fetch.bind(window) : null;
  if (origFetch)
    window.fetch = function (input, init) {
      var url = typeof input === 'string' ? input : input && (input.url || input.href || String(input));
      var method = (init && init.method) || (input && input.method) || 'GET';
      return origFetch(input, init).then(
        function (r) {
          if (r.status >= 400 && String(url).indexOf('/__hubness/') < 0) report('network', method + ' ' + url + ' → ' + r.status);
          return r;
        },
        function (err) {
          // a request the app cancelled itself is not a failure
          if (!err || err.name !== 'AbortError') report('network', method + ' ' + url + ' failed: ' + str(err));
          throw err;
        },
      );
    };
  var XO = XMLHttpRequest.prototype.open;
  var XS = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (m, u) {
    this.__hub = m + ' ' + u;
    return XO.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    var x = this;
    var aborted = false;
    x.addEventListener('abort', function () {
      aborted = true;
    });
    x.addEventListener('loadend', function () {
      if (x.status >= 400) report('network', x.__hub + ' → ' + x.status);
      else if (x.status === 0 && !aborted) report('network', x.__hub + ' failed');
    });
    return XS.apply(this, arguments);
  };

  if (!inFrame) return;

  // ---- address bar and history ----
  var last = '';
  function where() {
    var now = location.pathname + location.search + location.hash;
    if (now === last) return;
    last = now;
    post({ type: 'location', path: now, title: document.title });
  }
  ['pushState', 'replaceState'].forEach(function (name) {
    var orig = history[name];
    history[name] = function () {
      var r = orig.apply(this, arguments);
      setTimeout(where, 0);
      return r;
    };
  });
  window.addEventListener('popstate', where);
  window.addEventListener('hashchange', where);
  window.addEventListener('load', where);
  setTimeout(where, 0);

  // ---- element picker ----
  var picking = false;
  var box = null;

  function selectorOf(el) {
    if (!el || el.nodeType !== 1) return '';
    var parts = [];
    while (el && el.nodeType === 1 && el !== document.body && el !== document.documentElement && parts.length < 6) {
      if (el.id && /^[A-Za-z][\w-]*$/.test(el.id)) {
        parts.unshift('#' + el.id);
        break;
      }
      var attr = el.hasAttribute('data-testid') ? 'data-testid' : el.hasAttribute('data-test') ? 'data-test' : '';
      if (attr && /^[\w .:/-]+$/.test(el.getAttribute(attr))) {
        parts.unshift('[' + attr + '="' + el.getAttribute(attr) + '"]');
        break;
      }
      var part = el.tagName.toLowerCase();
      var cls = (el.getAttribute('class') || '').trim().split(/\s+/).filter(function (c) {
        return c && /^[A-Za-z][\w-]*$/.test(c) && c.length < 30;
      });
      if (cls.length) part += '.' + cls.slice(0, 2).join('.');
      var parent = el.parentElement;
      if (parent) {
        var same = Array.prototype.filter.call(parent.children, function (c) {
          return c.tagName === el.tagName;
        });
        if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
      }
      parts.unshift(part);
      el = parent;
    }
    return parts.join(' > ');
  }

  function outline(el) {
    if (!box) {
      box = document.createElement('div');
      box.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #39ff7a;background:rgba(57,255,122,.12);border-radius:3px;transition:all .05s';
      document.documentElement.appendChild(box);
    }
    var r = el.getBoundingClientRect();
    box.style.left = r.left + 'px';
    box.style.top = r.top + 'px';
    box.style.width = r.width + 'px';
    box.style.height = r.height + 'px';
    box.style.display = 'block';
  }

  function stopPicking() {
    picking = false;
    if (box) box.style.display = 'none';
    document.documentElement.style.cursor = '';
  }

  document.addEventListener('mousemove', function (e) {
    if (picking && e.target && e.target !== box) outline(e.target);
  }, true);
  document.addEventListener('click', function (e) {
    if (!picking) return;
    e.preventDefault();
    e.stopPropagation();
    var el = e.target;
    var r = el.getBoundingClientRect();
    stopPicking();
    post({
      type: 'pick',
      selector: selectorOf(el),
      text: (el.innerText || el.textContent || '').trim().slice(0, 300),
      html: el.outerHTML.slice(0, 1500),
      rect: { x: Math.round(r.left), y: Math.round(r.top), width: Math.round(r.width), height: Math.round(r.height) },
      viewport: { width: window.innerWidth, height: window.innerHeight },
      path: location.pathname + location.search + location.hash,
    });
  }, true);
  // while picking, the app must not react to the press that precedes the click
  ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'touchstart', 'touchend'].forEach(function (name) {
    document.addEventListener(name, function (e) {
      if (!picking) return;
      e.preventDefault();
      e.stopPropagation();
    }, true);
  });
  document.addEventListener('keydown', function (e) {
    if (picking && e.key === 'Escape') {
      stopPicking();
      post({ type: 'pick-cancel' });
    }
  }, true);

  window.addEventListener('message', function (e) {
    var m = e.data;
    if (!m || m.hubness !== 1 || e.source !== window.parent || !trusted(e.origin)) return;
    meet(e.origin);
    if (m.type === 'hello') post({ type: 'ready' });
    else if (m.type === 'back') history.back();
    else if (m.type === 'forward') history.forward();
    else if (m.type === 'reload') location.reload();
    else if (m.type === 'pick') {
      picking = true;
      document.documentElement.style.cursor = 'crosshair';
    } else if (m.type === 'pick-cancel') stopPicking();
  });
  post({ type: 'ready' });
})();
