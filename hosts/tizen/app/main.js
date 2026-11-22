/*
 * The page: renders the core's views, sends remote keys to it and plays
 * what it asks for with the TV's native player (AVPlay). The state is the
 * core's (worker.js runs it); this file keeps only presentation state.
 * Plain ES2017: the oldest supported engine is Chromium 69 (Tizen 5.5).
 */
'use strict';

var STORAGE_KEY = 'patina.source';
var PREVIEW = [1144, 48, 752, 423]; /* #search-side + #preview-frame, in 1920x1080 */
var GUIDE_PREVIEW = [24, 24, 548, 308]; /* #guide-frame: the guide screen's top left */
var FULL = [0, 0, 1920, 1080];
var ENTRIES = 10; /* the groups column (the core's VISIBLE_ENTRIES) */
/* The guide grid (style.css): rows, the channel column, the time area. */
var GRID = { rows: 8, left: 690, width: 1206, top: 58, rowHeight: 80 };
var SEARCH_ROWS = 8;

/*
 * Timings of this launch, kept in local storage (last 5 launches) so they
 * can be read after a normal launch, without a debugger attached.
 */
var metrics = { at: Date.now(), keys: [], guideKeys: [], searchKeys: [], search: [], zaps: [] };
var METRIC_SAMPLES = 200;
function mark(name) {
    if (metrics[name] !== undefined) return;
    metrics[name] = Math.round(performance.now());
    saveMetrics();
}
function sample(list, value) {
    list.push(value);
    if (list.length > METRIC_SAMPLES) list.shift();
    saveMetrics();
}
var metricsTimer = null;
function saveMetrics() {
    clearTimeout(metricsTimer);
    metricsTimer = setTimeout(function () {
        try {
            var all = JSON.parse(localStorage.getItem('patina.metrics') || '[]');
            if (all.length && all[all.length - 1].at === metrics.at) all.pop();
            all.push(metrics);
            localStorage.setItem('patina.metrics', JSON.stringify(all.slice(-5)));
        } catch (e) { /* storage full: timings are optional */ }
    }, 500);
}
/* When the keys the core has answered were pressed: the next render is
 * their answer on screen (render() measures and empties it). */
var keyTimes = [];

var worker = new Worker('worker.js');
/* The query typed but not yet answered: one search is asked at a time and
 * only the newest text waits, so typing never queues work in the worker. */
var searching = { busy: false, text: '', pending: null, sentAt: 0, answeredAt: 0 };
var view = null;
var drawn = false;
var bannerTimer = null;
var noticeTimer = null;

function $(id) { return document.getElementById(id); }
function send(cmd) { worker.postMessage(cmd); }
function hasTizen() { return typeof tizen !== 'undefined'; }
function hasAvplay() { return typeof webapis !== 'undefined' && webapis.avplay; }

/* ---------- formatting ---------- */
function programLine(p) { return p ? p.line : 'No information'; }
function percent(p) { return Math.round((p ? p.progress : 0) * 100) + '%'; }
function setLogo(img, logo) {
    logo = logo || '';
    if (img.getAttribute('data-src') === logo) return;
    img.setAttribute('data-src', logo);
    /* No logo hides the image; one inherits its row's visibility (an
     * explicit `visible` would show it in a row hidden as unused). */
    img.style.visibility = logo ? '' : 'hidden';
    if (logo) img.src = logo; else img.removeAttribute('src');
}

/* ---------- player ---------- */
/* Places the video plane: the method first, then the rectangle. On a 2020
 * TV (UN55TU7000, Tizen 5.5) LETTER_BOX in a rectangle smaller than the
 * screen draws no picture at all while the sound plays, so the previews
 * (16:9, as most sources) use FULL_SCREEN, which fills the rectangle. Full
 * screen uses LETTER_BOX, which works there and keeps 4:3 streams from
 * being stretched. */
function setDisplay(r) {
    var av = webapis.avplay;
    var whole = r[2] >= FULL[2] && r[3] >= FULL[3];
    av.setDisplayMethod(whole ? 'PLAYER_DISPLAY_MODE_LETTER_BOX' : 'PLAYER_DISPLAY_MODE_FULL_SCREEN');
    av.setDisplayRect(r[0], r[1], r[2], r[3]);
}

var player = {
    token: 0,
    rect: GUIDE_PREVIEW,
    lastStart: 0,
    deferred: null,
    /* The core's latest `play` ({ id, url, userAgent }), also while it
     * waits below or the app is hidden; null once nothing is to play. */
    wanted: null,
    /* Held channel keys: a zap within 300 ms of the last one waits, and only
     * the newest is opened (opening is synchronous on the main thread). */
    play: function (req) {
        this.wanted = req;
        clearTimeout(this.deferred);
        /* Hidden: nothing opens in the background; the newest request is
         * opened when the app shows again. */
        if (document.hidden) return;
        var now = performance.now();
        if (now - this.lastStart < 300) {
            var self = this;
            this.deferred = setTimeout(function () { self.lastStart = 0; self.play(req); }, 300 - (now - this.lastStart));
            return;
        }
        this.lastStart = now;
        var token = ++this.token;
        var asked = this.askedAt || performance.now();
        this.askedAt = null;
        var timed = false;
        /* Only the stream that is open reports (not one replaced since). */
        var failed = function (why) { if (token === player.token) player.failed(req.id, why); };
        screensaver(false);
        if (!hasAvplay()) return this.playFallback(req.url, failed);
        var av = webapis.avplay;
        try { av.stop(); } catch (e) { /* not playing */ }
        try { av.close(); } catch (e) { /* not open */ }
        try {
            av.open(req.url);
            if (req.userAgent) av.setStreamingProperty('USER_AGENT', req.userAgent);
            setDisplay(this.rect);
            av.setListener({
                onbufferingstart: function () { if (token === player.token) loading(true, 'Buffering…'); },
                onbufferingcomplete: function () { if (token === player.token) loading(false); },
                oncurrentplaytime: function () {
                    if (timed || token !== player.token) return;
                    timed = true;
                    sample(metrics.zaps, Math.round(performance.now() - asked));
                    loading(false);
                },
                onerror: function (e) { failed(String(e)); },
                onstreamcompleted: function () { failed('The stream ended.'); }
            });
            av.prepareAsync(function () {
                if (token !== player.token) return;
                try { av.play(); } catch (e) { failed(String(e)); }
            }, function (e) {
                failed(String(e && (e.name || e.message) || e));
            });
        } catch (e) {
            failed(String(e && e.message || e));
        }
    },
    playFallback: function (url, failed) {
        var v = $('fallback');
        v.style.display = 'block';
        this.place();
        v.src = url;
        var p = v.play();
        if (p && p.catch) p.catch(function (e) { failed(String(e)); });
    },
    place: function () {
        var r = this.rect;
        if (hasAvplay()) {
            try { setDisplay(r); } catch (e) { /* not open yet */ }
        } else {
            var s = $('fallback').style;
            s.left = r[0] + 'px'; s.top = r[1] + 'px'; s.width = r[2] + 'px'; s.height = r[3] + 'px';
        }
    },
    setRect: function (rect) {
        if (this.rect === rect) return;
        this.rect = rect;
        this.place();
    },
    /* Closes the stream that is open. What the core wants played stays
     * (`wanted`), and so does a zap waiting to open. */
    close: function () {
        this.token++;
        loading(false);
        if (hasAvplay()) {
            try { webapis.avplay.stop(); } catch (e) { /* idle */ }
            try { webapis.avplay.close(); } catch (e) { /* closed */ }
        } else {
            $('fallback').removeAttribute('src');
        }
    },
    /* Nothing is to play any more. */
    stop: function () {
        this.wanted = null;
        clearTimeout(this.deferred);
        this.close();
        screensaver(true);
    },
    /* The app is hidden: the stream closes, `wanted` opens on return. */
    suspend: function () {
        clearTimeout(this.deferred);
        this.close();
        screensaver(true);
    },
    resume: function () {
        if (this.wanted) this.play(this.wanted);
    },
    /* The stream of the core's play `id` failed. The core is told which
     * one: it ignores a failure of a stream it has replaced since, and a
     * newer play waiting here (a zap) still opens. The notice is the
     * core's: only it knows whether a newer play is still on its way. */
    failed: function (id, why) {
        if (this.wanted && this.wanted.id === id) this.stop();
        else this.close();
        send({ cmd: 'playFailed', id: id, why: why });
    }
};

function screensaver(on) {
    try {
        var c = webapis.appcommon;
        c.setScreenSaver(on ? c.AppCommonScreenSaverState.SCREEN_SAVER_ON : c.AppCommonScreenSaverState.SCREEN_SAVER_OFF,
            function () {}, function () {});
    } catch (e) { /* not a Samsung TV */ }
}

/* ---------- views ---------- */
/* The core stopped (`fatal`): its message covers everything, in a layer of
 * its own that nothing else shows, hides or paints over. */
var stopped = false;
function loading(on, text) {
    var box = $('loading');
    box.classList.toggle('hidden', !on);
    if (text) $('loading-text').textContent = text;
    /* Before the list exists it covers the screen; afterwards it only
     * marks the video (the preview, or the full screen). */
    var over = view && view.ready;
    var r = over ? player.rect : FULL;
    box.style.background = over ? 'transparent' : '#141C21';
    box.style.left = r[0] + 'px';
    box.style.top = r[1] + 'px';
    box.style.width = r[2] + 'px';
    box.style.height = r[3] + 'px';
    $('loading-text').style.top = Math.round(r[3] / 2 - 20) + 'px';
}
function notice(text, info) {
    var n = $('notice');
    n.textContent = text;
    n.classList.toggle('info', !!info);
    n.classList.remove('hidden');
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(function () { n.classList.add('hidden'); }, 5000);
}

function makeRows(list, count, cls) {
    for (var i = 0; i < count; i++) {
        var li = document.createElement('li');
        if (cls) {
            li.innerHTML = '<span class="num"></span><img alt=""><div class="name"></div><div class="prog"></div>';
        }
        list.appendChild(li);
    }
}

function statusText(s) {
    if (!s) return '';
    var mb = s.bytes ? ' · ' + (s.bytes / 1048576).toFixed(1) + ' MB' : '';
    switch (s.phase) {
    case 'connecting': case 'account': case 'authenticating': return 'Connecting to your provider…';
    case 'playlist': case 'groups': case 'channels': return 'Loading channels…' + (s.items ? ' ' + s.items : '') + mb;
    case 'guide': case 'matching': return (s.updating ? 'Updating the TV guide…' : 'Loading the TV guide…') + mb;
    case 'failed': return 'Could not load the playlist: ' + (s.error || 'unknown error') + '. Red button: change it.';
    case 'done': return s.guide ? '' : 'No TV guide' + (s.error ? ': ' + s.error : '.');
    default: return '';
    }
}

function render() {
    drawn = false;
    var v = view;
    if (!v || stopped) return;
    if (v.ready) mark('channelsShown');
    if (v.guide) mark('guideShown');
    if (!v.ready) {
        /* Keys answered before the list shows are not measured. */
        keyTimes.length = 0;
        loading(true, statusText(v.status) || 'Loading…');
        return;
    }
    if (!$('loading').classList.contains('hidden') && $('loading').style.background !== 'transparent') loading(false);
    var mode = v.mode;
    var full = mode === 'full';
    document.body.classList.toggle('full', full);
    document.body.classList.toggle('guide', mode === 'guide');
    document.body.classList.toggle('search', mode === 'search');
    var rect = full ? FULL : mode === 'guide' ? GUIDE_PREVIEW : PREVIEW;
    if (player.rect !== rect) {
        player.setRect(rect);
        /* "Buffering…" marks the video where it now is. */
        if (!$('loading').classList.contains('hidden')) loading(true);
    }
    $('status').textContent = statusText(v.status);
    if (mode === 'guide') renderGuide(v);
    else if (mode === 'search') { renderSearch(v.search); renderInfo(v.playing); }
    if (!full) $('banner').classList.add('hidden');
    renderBar(full ? v : null);
    /* Key to rendered page: the core's answer and the DOM updates (keys:
     * full screen, guideKeys: the guide screen, searchKeys: search). */
    var drawnAt = performance.now();
    var samples = mode === 'guide' ? metrics.guideKeys : mode === 'search' ? metrics.searchKeys : metrics.keys;
    while (keyTimes.length) sample(samples, Math.round(drawnAt - keyTimes.shift()));
    if (searching.answeredAt) {
        sample(metrics.search, Math.round(drawnAt - searching.answeredAt));
        searching.answeredAt = 0;
    }
}

function renderInfo(p) {
    $('info').style.visibility = p ? 'visible' : 'hidden';
    if (p) {
        $('info-channel').textContent = p.number + '  ' + p.name;
        $('info-now').textContent = programLine(p.now);
        $('info-progress').firstChild.style.width = percent(p.now);
        $('info-next').textContent = p.next ? 'Next  ' + programLine(p.next) : '';
    }
}

/* ---------- guide grid ---------- */
var guideRuler = null;
var PANES = ['guide', 'groups', 'rail', 'preview'];
function makeGuideRows() {
    var box = $('guide-rows');
    for (var i = 0; i < GRID.rows; i++) {
        var row = document.createElement('div');
        row.className = 'grow';
        row.style.top = (GRID.top + i * GRID.rowHeight) + 'px';
        row.innerHTML = '<div class="gch"><span class="num"></span><img alt=""><span class="name"></span></div><div class="gcells"></div>';
        row.sig = null;
        row.cellEls = [];
        row.focused = -1;
        box.appendChild(row);
    }
}
function renderGuide(v) {
    var g = v.grid;
    var perMinute = GRID.width / g.minutes;
    var f = g.focused;
    var body = document.body.classList;
    body.toggle('columns', g.columns);
    body.toggle('rail-open', g.layout === 'rail');
    body.toggle('idle', !v.playing);
    for (var p = 0; p < PANES.length; p++) body.toggle('pane-' + PANES[p], g.pane === PANES[p]);

    /* The focused program, beside the preview. */
    $('guide-channel').textContent = f ? f.number + '  ' + f.name : '';
    $('guide-title').textContent = f ? (f.title || 'No information') + (f.favorite ? '  ★' : '') : '';
    $('guide-time').textContent = f ? f.timeLabel : '';
    var live = f && f.airing === 'live';
    $('guide-progress').style.display = live ? 'inline-block' : 'none';
    if (live) $('guide-progress').firstChild.style.width = percent(f);
    $('guide-left').textContent = f ? f.timingLabel : '';
    /* While a held key moves through the groups, the list is built once it
     * is released (the core's `pending`). */
    var count = v.pending ? '…' : g.rows.total ? (g.rows.focus + 1) + ' / ' + g.rows.total : 'No channels';
    $('guide-list').textContent = v.title + '  ·  ' + count;
    $('guide-hint').textContent = v.hint;

    /* The rail and the groups column, while they show. */
    var rail = $('rail').children;
    for (var r = 0; r < rail.length; r++) {
        var name = rail[r].getAttribute('data-item');
        rail[r].classList.toggle('focus', name === g.rail);
        rail[r].classList.toggle('current', name === 'tv');
    }
    if (v.groups) {
        var groups = $('group-rows').children;
        for (var e = 0; e < groups.length; e++) {
            /* A group may be named "" (still a row). */
            var entry = v.groups.items[e];
            var exists = entry !== undefined && entry !== null;
            groups[e].style.visibility = exists ? 'visible' : 'hidden';
            if (!exists) continue;
            groups[e].textContent = entry;
            groups[e].classList.toggle('shown', v.groups.start + e === v.groups.focus);
        }
    }

    $('guide-clock').textContent = g.clock;
    var ruler = g.start + ':' + g.marks.map(function (mark) { return mark.time; }).join(',');
    if (guideRuler !== ruler) {
        guideRuler = ruler;
        var marks = $('guide-marks');
        marks.textContent = '';
        g.marks.forEach(function (mark) {
            var tick = document.createElement('span');
            tick.style.left = Math.round(GRID.left + (mark.at - g.start) / 60 * perMinute) + 'px';
            tick.textContent = mark.time;
            marks.appendChild(tick);
        });
    }
    var nowX = (g.now - g.start) / 60 * perMinute;
    var line = $('guide-now');
    line.style.visibility = nowX >= 0 && nowX <= GRID.width ? 'visible' : 'hidden';
    line.style.left = Math.round(GRID.left + nowX) + 'px';

    var rows = $('guide-rows').children;
    for (var i = 0; i < rows.length; i++) {
        var row = rows[i];
        var item = g.rows.items[i];
        if (!item) {
            row.style.visibility = 'hidden';
            continue;
        }
        row.style.visibility = 'visible';
        var focusedRow = g.rows.start + i === g.rows.focus;
        row.classList.toggle('focus', focusedRow);
        row.classList.toggle('moving', focusedRow && !!v.reorder);
        /* The core says what changed: a row's cells are rebuilt only when
         * its signature did; otherwise only the focus mark moves. */
        if (row.sig !== item.sig) {
            row.sig = item.sig;
            var head = row.firstChild;
            head.children[0].textContent = item.number;
            setLogo(head.children[1], item.logo);
            head.children[2].textContent = item.name;
            row.classList.toggle('playing', item.playing);
            var box = row.lastChild;
            var frag = document.createDocumentFragment();
            row.cellEls = [];
            for (var c = 0; c < item.cells.length; c++) {
                var cell = item.cells[c];
                var el = document.createElement('div');
                el.className = 'gc ' + cell.airing;
                el.style.left = Math.round(cell.x * perMinute) + 'px';
                el.style.width = Math.max(0, Math.round(cell.w * perMinute) - 4) + 'px';
                el.textContent = cell.title;
                frag.appendChild(el);
                row.cellEls.push(el);
            }
            box.textContent = '';
            box.appendChild(frag);
            row.focused = -1;
        }
        if (row.focused !== item.focus) {
            if (row.focused >= 0 && row.cellEls[row.focused]) row.cellEls[row.focused].classList.remove('focus');
            if (item.focus >= 0 && row.cellEls[item.focus]) row.cellEls[item.focus].classList.add('focus');
            row.focused = item.focus;
        }
    }
}

/* ---------- search ---------- */
/* Search closed (or the playlist reloads): an answer still due belongs to
 * that session, and nothing waits for it. */
function resetSearch() {
    searching.busy = false;
    searching.text = '';
    searching.pending = null;
    searching.answeredAt = 0;
}
function askSearch(text) {
    if (searching.busy) { searching.pending = text; return; }
    searching.busy = true;
    searching.text = text;
    searching.pending = null;
    searching.sentAt = performance.now();
    send({ cmd: 'searchText', text: text });
}
/* The newest text goes now (before Done, so Done applies to it). */
function flushSearch() {
    var input = $('search-input');
    if (input.value !== searching.text || searching.pending !== null) {
        searching.busy = false;
        askSearch(input.value);
    }
}
function searchAnswered(s) {
    if (!searching.busy || s.query !== searching.text) return;
    searching.busy = false;
    searching.answeredAt = searching.sentAt;
    if (searching.pending !== null && searching.pending !== searching.text) askSearch(searching.pending);
    searching.pending = null;
}
function renderSearch(s) {
    var input = $('search-input');
    var typing = s.focus === 'input' && s.keyboard;
    /* DOM focus follows the core's focus: the field (and the TV's
     * keyboard) only while the core says the keyboard is up. */
    if ((!typing || setup.open) && document.activeElement === input) input.blur();
    if (document.activeElement !== input && input.value !== s.query) input.value = s.query;
    /* The setup owns the keyboard while it is open. */
    if (typing && !setup.open && document.activeElement !== input) {
        keysLost();
        input.focus();
    }
    $('search-field').classList.toggle('focus', s.focus === 'input');

    var idle = s.phase === 'idle';
    $('search-tabs').style.visibility = idle ? 'hidden' : 'visible';
    var tabs = [['channels', 'Channels'], ['programs', 'Programs']];
    for (var t = 0; t < tabs.length; t++) {
        var tab = $('tab-' + tabs[t][0]);
        var shown = s.tab === tabs[t][0];
        tab.textContent = tabs[t][1] + '  ' + s.counts[tabs[t][0]];
        tab.classList.toggle('shown', shown);
        tab.classList.toggle('focus', shown && s.focus === 'tabs');
    }

    var rows = $('search-rows').children;
    var onRows = s.focus === 'result' || s.focus === 'history';
    for (var r = 0; r < rows.length; r++) {
        var item = s.rows.items[r];
        var li = rows[r];
        li.style.visibility = item ? 'visible' : 'hidden';
        if (!item) continue;
        var history = item.kind === 'history';
        li.classList.toggle('history', history);
        li.children[0].textContent = history ? '' : item.number;
        setLogo(li.children[1], history ? '' : item.logo);
        if (item.kind === 'program') {
            li.children[2].textContent = item.title;
            li.children[3].textContent = item.timeLabel + '  ' + item.name + (item.live ? '  ·  On now' : '');
        } else {
            li.children[2].textContent = history ? item.text : item.name;
            li.children[3].textContent = '';
        }
        li.classList.toggle('focus', onRows && s.rows.start + r === s.rows.focus);
    }

    var status = '';
    if (idle) status = s.rows.total ? 'Recent searches' : 'Type a channel or a program name.';
    else if (s.phase === 'none') status = 'Nothing found.' + (s.preparing ? ' Programs are still being prepared…' : '');
    else if (s.preparing) status = 'Preparing program search…';
    else status = 'OK: watch · Left/Right: Channels or Programs · Back: the field';
    $('search-status').textContent = status;
}

function scheduleRender() {
    if (drawn) return;
    drawn = true;
    requestAnimationFrame(render);
}

/* The play bar (OK over full-screen video). */
function renderBar(v) {
    var bar = v && v.bar, p = v && v.playing;
    $('bar').classList.toggle('hidden', !bar || !p);
    if (!bar || !p) return;
    $('banner').classList.add('hidden');
    $('bar-channel').textContent = p.number + '  ' + p.name;
    $('bar-now').textContent = p.now ? p.now.title : 'No information';
    $('bar-start').textContent = p.now ? p.now.startLabel : '';
    $('bar-end').textContent = p.now ? p.now.endLabel : '';
    $('bar-progress').firstChild.style.width = percent(p.now);
    $('bar-next').textContent = p.next ? 'Next  ' + programLine(p.next) : '';
    var buttons = $('bar-buttons').children;
    for (var i = 0; i < buttons.length; i++) {
        var b = buttons[i], name = b.getAttribute('data-button');
        if (name === 'favorite') b.textContent = bar.favorite ? '\u2605 Favorite' : '\u2606 Favorite';
        b.classList.toggle('focus', bar.focus === name);
    }
}

function showBanner(channel) {
    if (!view || view.mode !== 'full' || view.bar) return;
    $('banner-channel').textContent = channel.number + '  ' + channel.name;
    $('banner-now').textContent = programLine(channel.now);
    $('banner-progress').firstChild.style.width = percent(channel.now);
    $('banner-next').textContent = channel.next ? 'Next  ' + programLine(channel.next) : '';
    $('banner').classList.remove('hidden');
    clearTimeout(bannerTimer);
    bannerTimer = setTimeout(function () { $('banner').classList.add('hidden'); }, 4000);
}

worker.onmessage = function (e) {
    var m = JSON.parse(e.data);
    switch (m.type) {
    case 'ready': break;
    case 'view':
        var wasFull = view && view.mode === 'full';
        view = m;
        if (m.ready && m.mode === 'search') searchAnswered(m.search);
        else if (m.ready) resetSearch();
        if (m.seq !== undefined) keyAnswered(m.seq);
        if (m.session === session) armInputTimer(m.keyDeadlineMs);
        scheduleRender();
        if (m.ready && m.mode === 'full' && m.playing && !wasFull) {
            setTimeout(function () { showBanner(m.playing); }, 0);
        }
        break;
    case 'progress':
        if (view) { view.status = m.status; scheduleRender(); }
        break;
    case 'play':
        /* From a source replaced since (sent before the core had the new
         * one): the player stopped with it. */
        if (m.session !== session) break;
        player.play({ id: m.id, url: m.url, userAgent: m.userAgent });
        if (view && view.mode === 'full') showBanner(m.channel);
        break;
    case 'notice': notice(m.text, m.kind === 'info'); break;
    case 'sourceRejected': loading(false); openSetup(); notice(m.text); break;
    case 'fatal':
        /* The core stopped: nothing it showed works any more, and only
         * the Exit key still does something. */
        stopped = true;
        resetKeys();
        player.stop();
        $('fatal-text').textContent = m.text;
        $('fatal').classList.remove('hidden');
        break;
    case 'metric': metrics[m.name] = m; saveMetrics(); break;
    }
};

/* ---------- setup ---------- */
var setup = { open: false, kind: 'xtream', index: 0, editing: null };
function setupFields() {
    return Array.prototype.filter.call(document.querySelectorAll('#setup .field'), function (f) {
        var only = f.getAttribute('data-for');
        return !only || only === setup.kind;
    });
}
function drawSetup() {
    $('kind-label').textContent = setup.kind === 'xtream' ? 'Xtream Codes' : 'M3U playlist';
    Array.prototype.forEach.call(document.querySelectorAll('#setup .field'), function (f) {
        var only = f.getAttribute('data-for');
        f.classList.toggle('hidden', !!only && only !== setup.kind);
        f.classList.remove('focus');
    });
    var fields = setupFields();
    setup.index = Math.max(0, Math.min(setup.index, fields.length - 1));
    fields[setup.index].classList.add('focus');
}
function openSetup() {
    var saved = savedSource();
    if (saved) {
        setup.kind = saved.kind;
        $('server').value = saved.server || '';
        $('username').value = saved.username || '';
        $('password').value = saved.password || '';
        $('m3u').value = saved.url || '';
    }
    setup.open = true;
    keysLost();
    setup.index = 0;
    /* The search field must not keep the TV's keyboard under the setup. */
    $('search-input').blur();
    $('setup').classList.remove('hidden');
    drawSetup();
}
function closeSetup() {
    setup.open = false;
    $('setup').classList.add('hidden');
}
function savedSource() {
    try { return JSON.parse(localStorage.getItem(STORAGE_KEY)); } catch (e) { return null; }
}
/* Numbers the sources sent to the core: its `play` events carry the
 * number of the source they belong to. */
var session = 0;
function load(source) {
    player.stop();
    resetSearch();
    resetKeys();
    session++;
    send({ cmd: 'source', source: source, session: session, utcOffsetSecs: -new Date().getTimezoneOffset() * 60 });
}
/* Returns whether the key was the setup's own (so the page swallows it). */
function setupKey(code, repeat) {
    /* OK, Back and the keyboard's Done and Cancel act once per press: a
     * held OK would toggle the type, or open and close a field, over and
     * over. */
    if (repeat && (code === 13 || code === 10009 || code === IME_DONE || code === IME_CANCEL)) return true;
    if (setup.editing) {
        /* The IME and the field own the keys; Done/OK, Cancel and Back leave it. */
        if (code === 13 || code === IME_DONE || code === IME_CANCEL || code === 10009) {
            setup.editing.blur();
            setup.editing = null;
            return true;
        }
        return false;
    }
    var fields = setupFields();
    var field = fields[setup.index];
    if (code === 38) setup.index--;
    else if (code === 40) setup.index++;
    else if (code === 13) {
        if (field.hasAttribute('data-kind-toggle')) setup.kind = setup.kind === 'xtream' ? 'm3u' : 'xtream';
        else if (field.hasAttribute('data-save')) {
            var source = setup.kind === 'xtream'
                ? { kind: 'xtream', server: $('server').value.trim(), username: $('username').value.trim(), password: $('password').value }
                : { kind: 'm3u', url: $('m3u').value.trim() };
            var missing = setup.kind === 'xtream' ? !source.server || !source.username || !source.password : !source.url;
            if (missing) { notice('Fill in every field first.'); return true; }
            try {
                localStorage.setItem(STORAGE_KEY, JSON.stringify(source));
            } catch (e) {
                notice('The TV could not save the playlist (storage full?).');
            }
            closeSetup();
            loading(true, 'Connecting to your provider…');
            load(source);
            return true;
        } else {
            setup.editing = field.querySelector('input');
            setup.editing.focus();
            return true;
        }
    } else if (code === 10009) {
        if (savedSource()) {
            closeSetup();
            /* Nothing was loaded: what the setup held back shows at once
             * (the search field's focus and the TV's keyboard, when the
             * core says the keyboard is up). */
            scheduleRender();
        }
        return true;
    } else {
        return false;
    }
    drawSetup();
    return true;
}

/* ---------- keys ---------- */
var KEYS = { 37: 'left', 38: 'up', 39: 'right', 40: 'down', 13: 'ok', 10009: 'back',
    427: 'channel_up', 428: 'channel_down', 33: 'page_up', 34: 'page_down',
    405: 'yellow', 406: 'blue', 458: 'guide', 10225: 'search' };
/* The TV keyboard's Done and Cancel. */
var IME_DONE = 65376, IME_CANCEL = 65385;
var EXIT = 10182;

/* Only one input command waits on the worker. Physical phases and the
 * timer use the same page clock even when commands queue behind a reply. */
var KEY_TIMEOUT = 1000;
var keys = { seq: 0, inFlight: null, timer: null, deadline: null, deadlineTimer: null, pending: [] };
var pressed = null;

function sendKey(cmd, measure) {
    var entry = { cmd: cmd, at: measure ? cmd.atMs : null };
    var last = keys.pending[keys.pending.length - 1];
    if (cmd.phase === 'repeat' && last && last.cmd.phase === 'repeat' && last.cmd.key === cmd.key) {
        keys.pending[keys.pending.length - 1] = entry;
    } else {
        keys.pending.push(entry);
    }
    if (!keys.inFlight) nextKey();
}
function nextKey() {
    clearTimeout(keys.timer);
    var next = keys.pending.shift();
    keys.inFlight = next || null;
    if (!next) return;
    next.cmd.seq = ++keys.seq;
    send(next.cmd);
    /* Unanswered (the core is still starting): the queue goes on anyway. */
    keys.timer = setTimeout(nextKey, KEY_TIMEOUT);
}
function keyAnswered(seq) {
    var answered = keys.inFlight;
    if (!answered || answered.cmd.seq !== seq) return;
    if (answered.at !== null) keyTimes.push(answered.at);
    nextKey();
}
function inputNow() { return Math.floor(performance.now()); }
function armInputTimer(at) {
    if (keys.deadline === at) return;
    keys.deadline = at;
    clearTimeout(keys.deadlineTimer);
    if (at !== undefined && !stopped) {
        keys.deadlineTimer = setTimeout(function () {
            sendKey({ cmd: 'keyPoll', atMs: Math.max(at, inputNow()) }, false);
        }, Math.max(0, at - inputNow()));
    }
}
function resetKeys() {
    clearTimeout(keys.timer);
    armInputTimer(undefined);
    keys.inFlight = null;
    keys.pending = [];
    keyTimes.length = 0;
    pressed = null;
}

function keysLost() {
    pressed = null;
    armInputTimer(undefined);
    keys.pending = [];
    sendKey({ cmd: 'cancelKeys' }, false);
}

function dropRepeats(key) {
    keys.pending = keys.pending.filter(function (entry) {
        return !(entry.cmd.phase === 'repeat' && entry.cmd.key === key);
    });
}

function exit() {
    player.stop();
    try { tizen.application.getCurrentApplication().exit(); } catch (e) { window.close(); }
}

document.addEventListener('keydown', function (e) {
    var code = e.keyCode;
    /* The remote's Exit key leaves the app, from any screen. */
    if (code === EXIT) { e.preventDefault(); if (!e.repeat) exit(); return; }
    if (stopped) { e.preventDefault(); return; }
    if (setup.open) { if (setupKey(code, e.repeat)) e.preventDefault(); return; }
    if (code === 403) { if (!e.repeat) openSetup(); return; }
    if (code === 404) {
        if (!e.repeat) { send({ cmd: 'refreshGuide' }); notice('Updating the TV guide…', true); }
        return;
    }
    /* The TV keyboard owns input while the query field is focused. */
    if (document.activeElement === $('search-input')) {
        var done = code === IME_DONE || code === 13;
        if (done || code === IME_CANCEL || code === 10009) {
            e.preventDefault();
            if (e.repeat) return;
            if (done) flushSearch();
            sendKey({ cmd: 'keyboard', complete: done }, false);
        }
        return;
    }
    var key = KEYS[code];
    if (!key) return;
    e.preventDefault();
    if (e.repeat && pressed !== code) return;
    if (!e.repeat) {
        /* A new key takes over when the previous key-up was lost. */
        if (pressed !== null) dropRepeats(KEYS[pressed]);
        pressed = code;
    }
    var at = inputNow();
    var full = view && view.mode === 'full';
    if (key === 'ok' || key === 'channel_up' || key === 'channel_down'
        || (full && (key === 'up' || key === 'down'))) {
        player.askedAt = at;
    } else if (!full) {
        player.askedAt = null;
    }
    sendKey({ cmd: 'key', key: key, phase: e.repeat ? 'repeat' : 'down', atMs: at }, true);
});

document.addEventListener('keyup', function (e) {
    if (e.keyCode !== pressed) return;
    var key = KEYS[pressed];
    pressed = null;
    dropRepeats(key);
    sendKey({ cmd: 'key', key: key, phase: 'up', atMs: inputNow() }, key === 'left');
});
/* No keyup reaches a page that lost the focus or is hidden. */
window.addEventListener('blur', keysLost);

document.addEventListener('visibilitychange', function () {
    if (document.hidden) {
        keysLost();
        player.suspend();
    } else if (!stopped) {
        player.resume();
    }
});

(function start() {
    mark('scriptStart');
    if (hasTizen()) {
        ['ChannelUp', 'ChannelDown', 'ColorF0Red', 'ColorF1Green', 'ColorF2Yellow', 'ColorF3Blue', 'Guide', 'Search', 'Exit'].forEach(function (k) {
            try { tizen.tvinputdevice.registerKey(k); } catch (e) { /* the remote may lack it */ }
        });
    }
    makeRows($('group-rows'), ENTRIES, false);
    makeRows($('search-rows'), SEARCH_ROWS, true);
    makeGuideRows();
    $('search-input').addEventListener('input', function () { askSearch(this.value); });
    var source = savedSource();
    if (source) { loading(true, 'Connecting to your provider…'); load(source); } else openSetup();
    /* Program progress and "now" move on with the clock. */
    setInterval(function () { if (view && view.ready && !setup.open) send({ cmd: 'view' }); }, 30000);
})();
