/*
 * Patina's host services in the Worker, for iptv-app on nsh-platform-tizen:
 * the imports of module `host` that iptv-web's adapters link (fetch and
 * its stream, timers, key-value storage, clocks, randomness, the time
 * zone) and the browser store's chunks in IndexedDB. The platform loads
 * this with importScripts before it instantiates the module.
 *
 * Decisions stay in Rust: every address a request reaches is checked
 * there (iptv-web's destination policy); this file performs the request.
 * After a reply calls into the module (an `on_*` export, which polls
 * iptv-web's executor), `engine.pump()` sends what the app committed.
 *
 * The store's chunks and the app's small files (settings, last channels:
 * kv entries `file.<path>`) are read before the module is instantiated
 * (the engine's `instantiate` waits for them), so the store's open and the
 * app's start, which cannot wait, find them at hand (iptv-app's
 * host/web/store.rs, iptv-web's files.rs).
 *
 * Package files the module reads (an interface language's messages,
 * iptv-web's package.rs) are read synchronously: they are local, and the
 * module asks only for paths inside the package.
 *
 * Plain ES2017: the TV is Chromium 69.
 */
'use strict';

(function () {
    var ex = null;          /* the module's exports (wrapped by the engine) */
    var engine = null;
    var encoder = new TextEncoder();
    var decoder = new TextDecoder();
    var requests = {};
    /* The browser decides these itself; a request that sets them fails. */
    var FORBIDDEN = /^(user-agent|referer|host|connection|content-length|accept-encoding|cookie|origin)$/i;
    /* Error kinds (iptv-web lib.rs on_error). */
    var CONNECT = 1, IDLE = 2, BODY = 3, OTHER = 6, DEADLINE = 7;

    function memory() { return ex.memory.buffer; }
    function text(ptr, len) { return decoder.decode(new Uint8Array(memory(), ptr, len).slice()); }
    /* Copies bytes into the module's input buffer; returns their length. */
    function put(bytes) {
        var p = ex.input(bytes.length);
        new Uint8Array(memory(), p, bytes.length).set(bytes);
        return bytes.length;
    }
    /* A delivery into the module, then what the app committed to the page. */
    function deliver(f) {
        if (!ex) return;
        f();
        if (engine) engine.pump();
    }

    /* ---------- fetch ---------- */
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
        deliver(function () { ex.on_error(id, kind, put(encoder.encode(detail))); });
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
        /* `error` for a request carrying a header another origin must not
         * receive (iptv-web exec.rs request_json). */
        var init = { method: request.method, headers: headers, redirect: request.redirect === 'error' ? 'error' : 'follow', cache: 'no-store' };
        if (request.body) init.body = new Uint8Array(request.body);
        if (r.controller) init.signal = r.controller.signal;
        armIdle(id);
        fetch(request.url, init).then(function (res) {
            if (r.closed) return null;
            if (res.redirected) {
                /* Rust checks the final address; it may refuse and close. */
                deliver(function () { ex.on_redirect(id, put(encoder.encode(res.url))); });
                if (r.closed || !requests[id]) return null;
            }
            armIdle(id);
            var length = Number(res.headers.get('content-length'));
            var declared = isFinite(length) && length > 0 ? length : -1;
            if (stream) {
                r.reader = res.body ? res.body.getReader() : null;
                deliver(function () { ex.on_status(id, res.status, declared); });
                return null;
            }
            return res.arrayBuffer().then(function (buf) {
                if (!finish(id) || r.closed) return;
                deliver(function () {
                    ex.on_status(id, res.status, buf.byteLength);
                    ex.on_body(id, put(new Uint8Array(buf)));
                });
            });
        }).catch(function (err) { fail(id, err, err && err.name === 'TypeError' ? CONNECT : OTHER); });
    }

    /* ---------- IndexedDB: key-value and the store's chunks ---------- */
    var CHUNK = 64 * 1024;
    var db = null;
    var dbWaiting = [];
    function withDb(f) { if (db === null) dbWaiting.push(f); else f(); }
    function opened(result) {
        db = result;
        var waiting = dbWaiting;
        dbWaiting = [];
        waiting.forEach(function (f) { f(); });
    }
    /* The stored database, read before the module starts: its size (-1
     * none stored, -2 unreadable) and its chunks by index. */
    var stored = { size: -1, chunks: [] };
    /* The app's small files, packed as iptv-web's files.rs reads them,
     * until the module copies them. */
    var storedFiles = null;
    var FILE = 'file.';
    function packFiles(names, values) {
        var parts = [], total = 0;
        for (var i = 0; i < names.length; i++) {
            if (!(values[i] instanceof ArrayBuffer)) continue;
            var name = encoder.encode(String(names[i]).slice(FILE.length));
            parts.push(name, new Uint8Array(values[i]));
            total += 8 + name.length + values[i].byteLength;
        }
        var out = new Uint8Array(total), view = new DataView(out.buffer), at = 0;
        for (var j = 0; j < parts.length; j++) {
            view.setUint32(at, parts[j].length, true);
            out.set(parts[j], at + 4);
            at += 4 + parts[j].length;
        }
        return out;
    }
    var restored = new Promise(function (done) {
        var req;
        function unavailable() {
            stored.size = -2;
            opened(false);
            done();
        }
        try { req = indexedDB.open('patina-nsh', 1); } catch (e) { unavailable(); return; }
        req.onupgradeneeded = function () {
            req.result.createObjectStore('kv');
            req.result.createObjectStore('chunks');
        };
        req.onerror = unavailable;
        req.onsuccess = function () {
            opened(req.result);
            var t0 = performance.now();
            var tx = db.transaction(['kv', 'chunks'], 'readonly');
            var size = tx.objectStore('kv').get('db.size');
            var chunkKeys = tx.objectStore('chunks').getAllKeys();
            var chunkValues = tx.objectStore('chunks').getAll();
            /* '0' is the code point after '.': every `file.` key. */
            var files = IDBKeyRange.bound(FILE, 'file0', false, true);
            var fileNames = tx.objectStore('kv').getAllKeys(files);
            var fileValues = tx.objectStore('kv').getAll(files);
            tx.oncomplete = function () {
                storedFiles = packFiles(fileNames.result || [], fileValues.result || []);
                if (typeof size.result === 'number') {
                    var count = Math.ceil(size.result / CHUNK);
                    var chunks = [];
                    var keys = chunkKeys.result;
                    var values = chunkValues.result;
                    for (var k = 0; k < keys.length; k++) {
                        var index = keys[k];
                        if (Number.isInteger(index) && index >= 0 && index < count) chunks[index] = values[k];
                    }
                    var whole = chunks.length === count;
                    for (var i = 0; i < count && whole; i++) {
                        var expected = Math.min(CHUNK, size.result - i * CHUNK);
                        whole = chunks[i] instanceof ArrayBuffer && chunks[i].byteLength === expected;
                    }
                    stored = whole ? { size: size.result, chunks: chunks } : { size: -2, chunks: [] };
                }
                /* For DevTools on the page (the Worker is not inspectable
                 * there): how the last launch's restore went. */
                var restore = { at: Date.now(), ms: Math.round(performance.now() - t0), size: stored.size, chunks: chunkKeys.result.length };
                try { db.transaction('kv', 'readwrite').objectStore('kv').put(restore, 'diag.restore'); } catch (e) { /* optional */ }
                done();
            };
            tx.onerror = tx.onabort = function () { stored.size = -2; done(); };
        };
    });
    /* The engine instantiates only once the chunks are read (it compiles
     * meanwhile): the store's open reads them synchronously. */
    var instantiate = NshEngine.prototype.instantiate;
    NshEngine.prototype.instantiate = function (module) {
        var engineSelf = this;
        return restored.then(function () { return instantiate.call(engineSelf, module); });
    };

    function storeGet(id, key) {
        withDb(function () {
            if (!db) return deliver(function () { ex.on_missing(id); });
            var req = db.transaction('kv', 'readonly').objectStore('kv').get('app.' + key);
            req.onsuccess = function () {
                var v = req.result;
                deliver(function () {
                    if (v instanceof ArrayBuffer) ex.on_stored(id, put(new Uint8Array(v)));
                    else ex.on_missing(id);
                });
            };
            req.onerror = function () { deliver(function () { ex.on_missing(id); }); };
        });
    }
    function storePut(key, value) {
        withDb(function () {
            if (!db) return;
            var kv = db.transaction('kv', 'readwrite').objectStore('kv');
            if (value.byteLength) kv.put(value, 'app.' + key); else kv.delete('app.' + key);
        });
    }

    /* Chunk saves being assembled: id -> [[index, ArrayBuffer]]. */
    var saves = {};
    function commitChunks(id, size) {
        var puts = saves[id] || [];
        delete saves[id];
        var answer = function (ok) { deliver(function () { ex.on_chunks_saved(id, ok ? 1 : 0); }); };
        withDb(function () {
            if (!db) return answer(false);
            var tx;
            try { tx = db.transaction(['kv', 'chunks'], 'readwrite'); } catch (e) { return answer(false); }
            var chunks = tx.objectStore('chunks');
            puts.forEach(function (p) { chunks.put(p[1], p[0]); });
            var count = Math.ceil(size / CHUNK);
            chunks.delete(IDBKeyRange.lowerBound(count));
            tx.objectStore('kv').put(size, 'db.size');
            tx.oncomplete = function () { answer(true); };
            tx.onabort = function () { answer(false); };
        });
    }

    /* A small file's new content (null: removed), in its own transaction;
     * IndexedDB runs them in the order they were made. */
    function putFile(id, name, value) {
        var answer = function (ok) { deliver(function () { ex.on_chunks_saved(id, ok ? 1 : 0); }); };
        withDb(function () {
            if (!db) return answer(false);
            var tx;
            try {
                tx = db.transaction('kv', 'readwrite');
                var kv = tx.objectStore('kv');
                if (value) kv.put(value, FILE + name); else kv.delete(FILE + name);
            } catch (e) {
                if (tx) tx.abort();
                return answer(false);
            }
            tx.oncomplete = function () { answer(true); };
            tx.onabort = function () { answer(false); };
        });
    }

    /* The package file the module last asked for, until it copies it. */
    var packageFile = null;
    function readPackageFile(path) {
        try {
            var x = new XMLHttpRequest();
            x.open('GET', path, false);
            x.responseType = 'arraybuffer';
            x.send();
            /* A local package file has status 0 on the TV, 200 when served. */
            return (x.status === 200 || x.status === 0) && x.response ? x.response : null;
        } catch (e) {
            return null;
        }
    }

    NshTizenWorker.addImports('host', {
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
            setTimeout(function () { startFetch(id, request, stream === 1); }, 0);
        },
        fetch_next: function (id) {
            var r = requests[id];
            if (!r || !r.reader) {
                finish(id);
                return setTimeout(function () { deliver(function () { ex.on_end(id); }); }, 0);
            }
            armIdle(id);
            r.reader.read().then(function (c) {
                if (r.closed) return;
                if (c.done) {
                    finish(id);
                    deliver(function () { ex.on_end(id); });
                } else {
                    armIdle(id);
                    deliver(function () { ex.on_chunk(id, put(c.value)); });
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
            setTimeout(function () { deliver(function () { ex.on_wake(id); }); }, ms);
        },
        store_get: function (id, ptr, len) {
            var key = text(ptr, len);
            setTimeout(function () { storeGet(id, key); }, 0);
        },
        store_put: function (kptr, klen, vptr, vlen) {
            /* Copied now: the memory may move before the write runs. */
            storePut(text(kptr, klen), new Uint8Array(memory(), vptr, vlen).slice().buffer);
        },
        emit: function () { /* the Live TV bridge's events: unused here */ },
        monotonic_ms: function () { return performance.now(); },
        wall_ms: function () { return Date.now(); },
        random: function (ptr, len) {
            try { crypto.getRandomValues(new Uint8Array(memory(), ptr, len)); return 1; } catch (e) { return 0; }
        },
        utc_offset_minutes: function (ms) { return -new Date(ms).getTimezoneOffset(); },
        package_file_size: function (ptr, len) {
            packageFile = readPackageFile(text(ptr, len));
            return packageFile ? packageFile.byteLength : -1;
        },
        package_file_read: function (ptr, len) {
            var file = packageFile;
            packageFile = null;
            if (!file || file.byteLength !== len) return 0;
            new Uint8Array(memory(), ptr, len).set(new Uint8Array(file));
            return len;
        },
        chunks_stored_size: function () { return stored.size; },
        chunks_stored_read: function (index, ptr, len) {
            var chunk = stored.chunks[index];
            if (!(chunk instanceof ArrayBuffer) || chunk.byteLength !== len) return 0;
            new Uint8Array(memory(), ptr, len).set(new Uint8Array(chunk));
            return len;
        },
        chunks_stored_release: function () { stored.chunks = []; },
        chunks_put: function (id, index, ptr, len) {
            (saves[id] = saves[id] || []).push([index, new Uint8Array(memory(), ptr, len).slice().buffer]);
        },
        chunks_commit: function (id, size) {
            setTimeout(function () { commitChunks(id, size); }, 0);
        },
        files_stored_size: function () { return storedFiles ? storedFiles.length : -1; },
        files_stored_read: function (ptr, len) {
            var files = storedFiles;
            storedFiles = null;
            if (!files || files.length !== len) return 0;
            new Uint8Array(memory(), ptr, len).set(files);
            return len;
        },
        files_put: function (id, nptr, nlen, ptr, len, present) {
            /* Copied now: the memory may move before the write runs. */
            var name = text(nptr, nlen);
            var value = present ? new Uint8Array(memory(), ptr, len).slice().buffer : null;
            /* Answered later, never from inside this call. */
            setTimeout(function () { putFile(id, name, value); }, 0);
        }
    });

    NshTizenWorker.onInstance(function (exports, e) {
        ex = exports;
        engine = e;
        /* The module's memory, current and peak, for DevTools on the page
         * (`diag.memory`): the Worker's heap is not visible there. */
        var peak = 0;
        setInterval(function () {
            var bytes = ex.memory.buffer.byteLength;
            if (bytes <= peak) return;
            peak = bytes;
            withDb(function () {
                if (!db) return;
                try { db.transaction('kv', 'readwrite').objectStore('kv').put({ at: Date.now(), peak: peak }, 'diag.memory'); } catch (err) { /* optional */ }
            });
        }, 2000);
    });
})();
