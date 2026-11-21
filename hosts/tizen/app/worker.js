/*
 * The core's worker: runs iptv-web (WebAssembly) and performs the HTTP
 * requests and storage operations it asks for. Views and play requests go
 * to the page. Every import is answered later, never inside the call.
 * Plain ES2017: the oldest supported engine is Chromium 69 (Tizen 5.5).
 */
'use strict';

var ex = null;
var dead = false;
var encoder = new TextEncoder();
var decoder = new TextDecoder();
/* Per request: controller, reader, timers and why it was aborted. */
var requests = {};

/* The browser decides these itself; a request that sets them fails. */
var FORBIDDEN = /^(user-agent|referer|host|connection|content-length|accept-encoding|cookie|origin)$/i;
/* Error kinds (lib.rs on_error). */
var CONNECT = 1, IDLE = 2, BODY = 3, OTHER = 6, DEADLINE = 7;

function put(bytes) {
    var p = ex.input(bytes.length);
    new Uint8Array(ex.memory.buffer, p, bytes.length).set(bytes);
    return bytes.length;
}
function text(ptr, len) {
    return decoder.decode(new Uint8Array(ex.memory.buffer, ptr, len).slice());
}

/* Calls an export; a trap (a Rust panic) stops the core for good. */
function call(f) {
    if (dead || !ex) return;
    try {
        f();
    } catch (err) {
        dead = true;
        postMessage(JSON.stringify({ type: 'fatal', text: 'Patina stopped unexpectedly. Restart the app. (' + err + ')' }));
    }
}

function finish(id) {
    var r = requests[id];
    if (!r) return null;
    delete requests[id];
    clearTimeout(r.deadline);
    clearTimeout(r.idle);
    return r;
}
function fail(id, err, kind) {
    var r = finish(id);
    if (!r || r.closed) return;
    if (r.reason) kind = r.reason;
    var detail = String(err && err.message || err);
    call(function () { ex.on_error(id, kind, put(encoder.encode(detail))); });
}
function armIdle(id) {
    var r = requests[id];
    if (!r || !r.idleMs || !r.controller) return;
    clearTimeout(r.idle);
    r.idle = setTimeout(function () { r.reason = IDLE; r.controller.abort(); }, r.idleMs);
}

function startFetch(id, request, stream) {
    var r = requests[id];
    if (!r || r.closed) return;
    var headers = {};
    for (var i = 0; i < request.headers.length; i++) {
        if (!FORBIDDEN.test(request.headers[i][0])) headers[request.headers[i][0]] = request.headers[i][1];
    }
    var init = { method: request.method, headers: headers, redirect: 'follow', cache: 'no-store' };
    if (request.body) init.body = new Uint8Array(request.body);
    if (r.controller) init.signal = r.controller.signal;
    armIdle(id);
    fetch(request.url, init).then(function (res) {
        if (r.closed) return null;
        if (res.redirected) {
            /* The core checks every hop it can see; it may refuse and close. */
            call(function () { ex.on_redirect(id, put(encoder.encode(res.url))); });
            if (r.closed || !requests[id]) return null;
        }
        armIdle(id);
        var length = Number(res.headers.get('content-length'));
        var declared = isFinite(length) && length > 0 ? length : -1;
        if (stream) {
            r.reader = res.body ? res.body.getReader() : null;
            call(function () { ex.on_status(id, res.status, declared); });
            return null;
        }
        return res.arrayBuffer().then(function (buf) {
            if (!finish(id) || r.closed) return;
            call(function () {
                ex.on_status(id, res.status, buf.byteLength);
                ex.on_body(id, put(new Uint8Array(buf)));
            });
        });
    }).catch(function (err) { fail(id, err, err && err.name === 'TypeError' ? CONNECT : OTHER); });
}

/* ---------- storage: one IndexedDB object store ---------- */
var db = null;
var dbWaiting = [];
(function openDb() {
    var req = indexedDB.open('patina', 1);
    function ready(value) {
        db = value;
        var waiting = dbWaiting;
        dbWaiting = [];
        waiting.forEach(function (f) { f(); });
    }
    req.onupgradeneeded = function () { req.result.createObjectStore('kv'); };
    req.onsuccess = function () { ready(req.result); };
    /* Without storage the app still works; it only forgets. */
    req.onerror = function () { ready(false); };
})();
function withDb(f) { if (db === null) dbWaiting.push(f); else f(); }

function storeGet(id, key) {
    withDb(function () {
        if (!db) return call(function () { ex.on_missing(id); });
        var req = db.transaction('kv', 'readonly').objectStore('kv').get(key);
        req.onsuccess = function () {
            var v = req.result;
            call(function () {
                if (v instanceof ArrayBuffer) ex.on_stored(id, put(new Uint8Array(v)));
                else ex.on_missing(id);
            });
        };
        req.onerror = function () { call(function () { ex.on_missing(id); }); };
    });
}

/*
 * Writes made in one turn of the core (a guide and its state) go into one
 * transaction: either all of them are stored or none (a full disk aborts it).
 */
var batch = null;
function storePut(key, value) {
    if (!batch) {
        batch = [];
        Promise.resolve().then(function () {
            var writes = batch;
            batch = null;
            withDb(function () {
                if (!db) return;
                var tx = db.transaction('kv', 'readwrite');
                var kv = tx.objectStore('kv');
                writes.forEach(function (w) {
                    if (w[1].byteLength) kv.put(w[1], w[0]); else kv.delete(w[0]);
                });
                tx.onabort = function () {
                    postMessage(JSON.stringify({ type: 'notice', text: 'The TV could not save Patina\'s data (storage full?).' }));
                };
            });
        });
    }
    batch.push([key, value]);
}

var imports = {
    host: {
        fetch: function (id, ptr, len, stream) {
            var request = JSON.parse(text(ptr, len));
            var r = {
                controller: typeof AbortController === 'function' ? new AbortController() : null,
                idleMs: request.idleMs || 0,
                reason: 0,
                closed: false
            };
            requests[id] = r;
            if (r.controller && request.timeoutMs) {
                r.deadline = setTimeout(function () { r.reason = DEADLINE; r.controller.abort(); }, request.timeoutMs);
            }
            /* Answer later: the core never expects a reply inside its call. */
            setTimeout(function () { startFetch(id, request, stream === 1); }, 0);
        },
        fetch_next: function (id) {
            var r = requests[id];
            if (!r || !r.reader) {
                finish(id);
                return setTimeout(function () { call(function () { ex.on_end(id); }); }, 0);
            }
            armIdle(id);
            r.reader.read().then(function (c) {
                if (r.closed) return;
                if (c.done) {
                    finish(id);
                    call(function () { ex.on_end(id); });
                } else {
                    armIdle(id);
                    call(function () { ex.on_chunk(id, put(c.value)); });
                }
            }).catch(function (err) { fail(id, err, BODY); });
        },
        fetch_close: function (id) {
            var r = finish(id);
            if (!r) return;
            r.closed = true;
            if (r.reader) r.reader.cancel().catch(function () {});
            if (r.controller) r.controller.abort();
        },
        sleep: function (id, ms) {
            setTimeout(function () { call(function () { ex.on_wake(id); }); }, ms);
        },
        store_get: function (id, ptr, len) {
            var key = text(ptr, len);
            setTimeout(function () { storeGet(id, key); }, 0);
        },
        store_put: function (kptr, klen, vptr, vlen) {
            /* Copied now: the memory may move before the write runs. */
            storePut(text(kptr, klen), new Uint8Array(ex.memory.buffer, vptr, vlen).slice().buffer);
        },
        emit: function (ptr, len) {
            postMessage(text(ptr, len));
        },
        monotonic_ms: function () { return performance.now(); },
        wall_ms: function () { return Date.now(); }
    }
};

var queue = [];
function command(cmd) {
    if (!ex) return queue.push(cmd);
    call(function () { ex.command(put(encoder.encode(JSON.stringify(cmd)))); });
}
onmessage = function (e) { command(e.data); };

(function load() {
    var x = new XMLHttpRequest();
    x.open('GET', 'iptv_web.wasm', true);
    x.responseType = 'arraybuffer';
    x.onload = function () {
        WebAssembly.instantiate(x.response, imports).then(function (r) {
            ex = r.instance.exports;
            postMessage(JSON.stringify({ type: 'ready' }));
            var pending = queue;
            queue = [];
            pending.forEach(command);
        }).catch(function (err) {
            postMessage(JSON.stringify({ type: 'fatal', text: 'The core could not start: ' + err }));
        });
    };
    x.onerror = function () { postMessage(JSON.stringify({ type: 'fatal', text: 'The core is missing.' })); };
    x.send();
})();
