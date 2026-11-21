/*
 * The core's worker: runs iptv-web (WebAssembly) and performs the HTTP
 * requests it describes. Views and play requests go to the page.
 * Plain ES2017: the oldest supported engine is Chromium 69 (Tizen 5.5).
 */
'use strict';

var ex = null;
var encoder = new TextEncoder();
var decoder = new TextDecoder();
var streams = {};
var aborts = {};

/* The browser decides these itself; a request that sets them fails. */
var FORBIDDEN = /^(user-agent|referer|host|connection|content-length|accept-encoding|cookie|origin)$/i;

function put(bytes) {
    var p = ex.input(bytes.length);
    new Uint8Array(ex.memory.buffer, p, bytes.length).set(bytes);
    return bytes.length;
}
function text(ptr, len) {
    return decoder.decode(new Uint8Array(ex.memory.buffer, ptr, len).slice());
}
function fail(id, err, aborted) {
    delete streams[id];
    delete aborts[id];
    var kind = aborted ? 2 : (err && err.name === 'TypeError' ? 1 : 6);
    var len = put(encoder.encode(String(err && err.message || err)));
    ex.on_error(id, kind, len);
}

function startFetch(id, request, stream) {
    var controller = typeof AbortController === 'function' ? new AbortController() : null;
    var timedOut = false;
    aborts[id] = controller;
    if (controller && request.timeoutMs) {
        setTimeout(function () { timedOut = true; controller.abort(); }, request.timeoutMs);
    }
    var headers = {};
    for (var i = 0; i < request.headers.length; i++) {
        if (!FORBIDDEN.test(request.headers[i][0])) headers[request.headers[i][0]] = request.headers[i][1];
    }
    var init = { method: request.method, headers: headers, redirect: 'follow', cache: 'no-store' };
    if (request.body) init.body = new Uint8Array(request.body);
    if (controller) init.signal = controller.signal;
    fetch(request.url, init).then(function (res) {
        var length = Number(res.headers.get('content-length'));
        if (stream) {
            streams[id] = res.body.getReader();
            ex.on_status(id, res.status, isFinite(length) && length > 0 ? length : -1);
            return null;
        }
        return res.arrayBuffer().then(function (buf) {
            delete aborts[id];
            ex.on_status(id, res.status, buf.byteLength);
            ex.on_body(id, put(new Uint8Array(buf)));
        });
    }).catch(function (err) { fail(id, err, timedOut); });
}

/* ---------- storage: one IndexedDB object store, writes in call order ---------- */
var db = null;
var dbWaiting = [];
(function openDb() {
    var req = indexedDB.open('patina', 1);
    req.onupgradeneeded = function () { req.result.createObjectStore('kv'); };
    req.onsuccess = function () {
        db = req.result;
        var waiting = dbWaiting;
        dbWaiting = [];
        waiting.forEach(function (f) { f(); });
    };
    req.onerror = function () {
        /* Without storage the app still works; it only forgets. */
        db = false;
        var waiting = dbWaiting;
        dbWaiting = [];
        waiting.forEach(function (f) { f(); });
    };
})();
function withDb(f) { if (db === null) dbWaiting.push(f); else f(); }
function storeGet(id, key) {
    withDb(function () {
        if (!db) return ex.on_missing(id);
        var req = db.transaction('kv', 'readonly').objectStore('kv').get(key);
        req.onsuccess = function () {
            var v = req.result;
            if (v instanceof ArrayBuffer) ex.on_stored(id, put(new Uint8Array(v)));
            else ex.on_missing(id);
        };
        req.onerror = function () { ex.on_missing(id); };
    });
}
function storePut(key, value) {
    withDb(function () {
        if (!db) return;
        var tx = db.transaction('kv', 'readwrite');
        var kv = tx.objectStore('kv');
        if (value.byteLength) kv.put(value, key); else kv.delete(key);
        tx.onerror = function () {
            postMessage(JSON.stringify({ type: 'notice', text: 'The TV could not save Patina\'s data (storage full?).' }));
        };
    });
}

var imports = {
    host: {
        fetch: function (id, ptr, len, stream) {
            var request = JSON.parse(text(ptr, len));
            /* Answer later: the core never expects a reply inside its call. */
            setTimeout(function () { startFetch(id, request, stream === 1); }, 0);
        },
        fetch_next: function (id) {
            var reader = streams[id];
            if (!reader) return setTimeout(function () { ex.on_end(id); }, 0);
            reader.read().then(function (c) {
                if (c.done) {
                    delete streams[id];
                    delete aborts[id];
                    ex.on_end(id);
                } else {
                    ex.on_chunk(id, put(c.value));
                }
            }).catch(function (err) { fail(id, err, false); });
        },
        fetch_close: function (id) {
            var reader = streams[id];
            delete streams[id];
            if (reader) reader.cancel().catch(function () {});
            if (aborts[id]) aborts[id].abort();
            delete aborts[id];
        },
        sleep: function (id, ms) {
            setTimeout(function () { ex.on_wake(id); }, ms);
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
    ex.command(put(encoder.encode(JSON.stringify(cmd))));
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
