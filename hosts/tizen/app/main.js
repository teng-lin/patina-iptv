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
function two(n) { return n < 10 ? '0' + n : String(n); }
function clock(unix) { var d = new Date(unix * 1000); return two(d.getHours()) + ':' + two(d.getMinutes()); }
function progress(p) {
    if (!p) return 0;
    var now = Date.now() / 1000;
    return Math.max(0, Math.min(1, (now - p.start) / Math.max(1, p.end - p.start)));
}
function programLine(p) { return p ? clock(p.start) + '  ' + p.title : 'No information'; }
var DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
function day(unix) {
    var d = new Date(unix * 1000), today = new Date();
    return d.toDateString() === today.toDateString() ? '' : DAYS[d.getDay()] + ' ';
}
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
    url: null,
    userAgent: null,
    token: 0,
    rect: GUIDE_PREVIEW,
    lastStart: 0,
    deferred: null,
    /* What the core last asked to play, [url, userAgent], also while that
     * waits below; null once stopped. */
    wanted: null,
    /* Held channel keys: a zap within 300 ms of the last one waits, and only
     * the newest is opened (opening is synchronous on the main thread). */
    play: function (url, userAgent) {
        var now = performance.now();
        this.wanted = [url, userAgent];
        clearTimeout(this.deferred);
        if (now - this.lastStart < 300) {
            var self = this;
            this.deferred = setTimeout(function () { self.lastStart = 0; self.play(url, userAgent); }, 300 - (now - this.lastStart));
            return;
        }
        this.lastStart = now;
        this.url = url;
        this.userAgent = userAgent;
        var token = ++this.token;
        var asked = this.askedAt || performance.now();
        this.askedAt = null;
        var timed = false;
        screensaver(false);
        if (!hasAvplay()) return this.playFallback(url);
        var av = webapis.avplay;
        try { av.stop(); } catch (e) { /* not playing */ }
        try { av.close(); } catch (e) { /* not open */ }
        try {
            av.open(url);
            if (userAgent) av.setStreamingProperty('USER_AGENT', userAgent);
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
                onerror: function (e) { if (token === player.token) player.failed(String(e)); },
                onstreamcompleted: function () { if (token === player.token) player.failed('The stream ended.'); }
            });
            av.prepareAsync(function () {
                if (token !== player.token) return;
                try { av.play(); } catch (e) { player.failed(String(e)); }
            }, function (e) {
                if (token === player.token) player.failed(String(e && (e.name || e.message) || e));
            });
        } catch (e) {
            this.failed(String(e && e.message || e));
        }
    },
    playFallback: function (url) {
        var v = $('fallback');
        v.style.display = 'block';
        this.place();
        v.src = url;
        var p = v.play();
        if (p && p.catch) p.catch(function (e) { player.failed(String(e)); });
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
    stop: function () {
        this.token++;
        this.url = null;
        this.wanted = null;
        clearTimeout(this.deferred);
        loading(false);
        screensaver(true);
        if (hasAvplay()) {
            try { webapis.avplay.stop(); } catch (e) { /* idle */ }
            try { webapis.avplay.close(); } catch (e) { /* closed */ }
        } else {
            $('fallback').removeAttribute('src');
        }
    },
    failed: function (why) {
        loading(false);
        this.stop();
        notice('This channel did not play. ' + why);
        send({ cmd: 'playFailed' });
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
/* `cover`: over the whole screen, opaque, also once the list shows (the
 * core stopped). */
var stopped = false;
function loading(on, text, cover) {
    var box = $('loading');
    box.classList.toggle('hidden', !on);
    if (text) $('loading-text').textContent = text;
    /* Before the list exists it covers the screen; afterwards it only
     * marks the video (the preview, or the full screen). */
    var over = !cover && !stopped && view && view.ready;
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
    /* A key older than a second was not answered by this view (one sent
     * before the list was ready): it is dropped, not counted. */
    while (keyTimes.length) {
        var at = keyTimes.shift();
        if (drawnAt - at < 1000) sample(samples, Math.round(drawnAt - at));
    }
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
        $('info-progress').firstChild.style.width = Math.round(progress(p.now) * 100) + '%';
        $('info-next').textContent = p.next ? 'Next  ' + programLine(p.next) : '';
    }
}

/* ---------- guide grid ---------- */
var guideRulerStart = null;
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
function minutes(secs) {
    var m = Math.max(1, Math.ceil(secs / 60));
    return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h' + (m % 60 ? ' ' + m % 60 + ' min' : '');
}
function guideHint(v, f) {
    var g = v.grid;
    if (v.reorder) return 'Moving: Up/Down one place · Left/Right: to the top or the end · OK: done';
    switch (g.pane) {
    case 'preview': return 'OK: full screen · Down: guide';
    case 'groups': return 'Up/Down: lists · OK: open · Left: the rail';
    case 'rail': return 'OK: open · Right: groups';
    default:
        var ok = !f ? '' : f.airing === 'live' ? 'OK: watch · ' : f.catchup ? 'OK: watch from the start · ' : '';
        var move = v.title === 'Favorites' && g.rows.total > 1 ? ' · Blue: move' : '';
        return ok + 'Channel Up/Down: page · Yellow: favorite' + move + ' · Back: groups';
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
    var now = Date.now() / 1000;
    $('guide-channel').textContent = f ? f.number + '  ' + f.name : '';
    $('guide-title').textContent = f ? (f.title || 'No information') + (f.favorite ? '  ★' : '') : '';
    $('guide-time').textContent = f ? day(f.start) + clock(f.start) + ' – ' + clock(f.end) : '';
    var live = f && f.airing === 'live';
    $('guide-progress').style.display = live ? 'inline-block' : 'none';
    if (live) $('guide-progress').firstChild.style.width = Math.round(progress(f) * 100) + '%';
    $('guide-left').textContent = !f ? '' : live ? minutes(f.end - now) + ' left'
        : f.airing === 'upcoming' ? 'in ' + minutes(f.start - now) : 'Ended';
    /* While a held key moves through the groups, the list is built once it
     * is released (the core's `pending`). */
    var count = v.pending ? '…' : g.rows.total ? (g.rows.focus + 1) + ' / ' + g.rows.total : 'No channels';
    $('guide-list').textContent = v.title + '  ·  ' + count;
    $('guide-hint').textContent = guideHint(v, f);

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

    var d = new Date();
    $('guide-clock').textContent = DAYS[d.getDay()] + ' ' + d.getDate() + '  ' + two(d.getHours()) + ':' + two(d.getMinutes());
    if (guideRulerStart !== g.start) {
        guideRulerStart = g.start;
        var marks = $('guide-marks');
        marks.textContent = '';
        var end = g.start + g.minutes * 60;
        for (var t = Math.ceil(g.start / 1800) * 1800; t < end; t += 1800) {
            var tick = document.createElement('span');
            tick.style.left = Math.round(GRID.left + (t - g.start) / 60 * perMinute) + 'px';
            tick.textContent = clock(t);
            marks.appendChild(tick);
        }
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
    if (typing && !setup.open && document.activeElement !== input) input.focus();
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
            li.children[3].textContent = day(item.start) + clock(item.start) + '–' + clock(item.end) + '  ' + item.name + (item.live ? '  ·  On now' : '');
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

/* The play bar (OK over full-screen video): the playing channel, its
 * program with times, and buttons. It hides after BAR_TIMEOUT ms without a
 * key: the core is told, as it owns the bar's state. */
var BAR_TIMEOUT = 7000;
var barTimer = null, barShown = false;
/* Counts the 7 s again: from when the bar shows, and from every key. */
function armBar() {
    clearTimeout(barTimer);
    if (barShown) barTimer = setTimeout(function () { sendKey({ cmd: 'key', key: 'barTimeout' }); }, BAR_TIMEOUT);
}
function renderBar(v) {
    var bar = v && v.bar, p = v && v.playing;
    $('bar').classList.toggle('hidden', !bar || !p);
    var shown = !!(bar && p);
    if (shown !== barShown) {
        barShown = shown;
        armBar();
    }
    if (!bar || !p) return;
    $('banner').classList.add('hidden');
    $('bar-channel').textContent = p.number + '  ' + p.name;
    $('bar-now').textContent = p.now ? p.now.title : 'No information';
    $('bar-start').textContent = p.now ? clock(p.now.start) : '';
    $('bar-end').textContent = p.now ? clock(p.now.end) : '';
    $('bar-progress').firstChild.style.width = Math.round(progress(p.now) * 100) + '%';
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
    $('banner-progress').firstChild.style.width = Math.round(progress(channel.now) * 100) + '%';
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
        keyAnswered();
        scheduleRender();
        if (m.ready && m.mode === 'full' && m.playing && !wasFull) {
            setTimeout(function () { showBanner(m.playing); }, 0);
        }
        break;
    case 'progress':
        if (view) { view.status = m.status; scheduleRender(); }
        break;
    case 'play':
        player.play(m.url, m.userAgent);
        if (view && view.mode === 'full') showBanner(m.channel);
        break;
    case 'notice': notice(m.text, m.kind === 'info'); break;
    case 'sourceRejected': loading(false); openSetup(); notice(m.text); break;
    case 'fatal':
        /* The core stopped: nothing it showed works any more. */
        player.stop();
        loading(true, m.text, true);
        stopped = true;
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
function load(source) {
    player.stop();
    resetSearch();
    keys.pending = [];
    send({ cmd: 'source', source: source, utcOffsetSecs: -new Date().getTimezoneOffset() * 60 });
}
/* Returns whether the key was the setup's own (so the page swallows it). */
function setupKey(code) {
    if (setup.editing) {
        /* The IME and the field own the keys; Done/OK, Cancel and Back leave it. */
        if (code === 13 || code === 65376 || code === 65385 || code === 10009) {
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
        if (savedSource()) closeSetup();
        return true;
    } else {
        return false;
    }
    drawSetup();
    return true;
}

/* ---------- keys ---------- */
var KEYS = { 37: 'left', 38: 'up', 39: 'right', 40: 'down', 13: 'ok', 10009: 'back',
    427: 'chup', 428: 'chdown', 33: 'pageup', 34: 'pagedown',
    406: 'blue', 458: 'guide', 10225: 'search' };
/* The TV keyboard's Done and Cancel. */
var IME_DONE = 65376, IME_CANCEL = 65385;
var EXIT = 10182;

/* Keys to the core: one at a time, the next sent when the view for the
 * previous one arrives, so a held key never queues work in the worker.
 * While one is in flight, a repeat replaces a repeat waiting behind it (a
 * released key stops at once); other keys wait in order. */
var keys = { inFlight: false, sentAt: 0, pending: [] };
function sendKey(cmd) {
    var last = keys.pending[keys.pending.length - 1];
    if (cmd.repeat && last && last.repeat) keys.pending[keys.pending.length - 1] = cmd;
    else keys.pending.push(cmd);
    /* Unanswered for a second (the core is still loading): go on anyway. */
    if (keys.inFlight && performance.now() - keys.sentAt < 1000) return;
    keyAnswered();
}
/* A view arrived: the next key waiting goes. */
function keyAnswered() {
    var next = keys.pending.shift();
    keys.inFlight = !!next;
    if (!next) return;
    keys.sentAt = performance.now();
    send(next);
}
/* Keys whose repeats are dropped: holding one acts once. */
var ONCE = { ok: true, back: true, blue: true, guide: true, search: true };

function exit() {
    player.stop();
    try { tizen.application.getCurrentApplication().exit(); } catch (e) { window.close(); }
}

document.addEventListener('keydown', function (e) {
    var code = e.keyCode;
    /* The remote's Exit key leaves the app, from any screen; Back never does. */
    if (code === EXIT) { e.preventDefault(); if (!e.repeat) exit(); return; }
    if (setup.open) { if (setupKey(code)) e.preventDefault(); return; }
    /* The color keys act once per press. */
    if (code >= 403 && code <= 405 && e.repeat) { e.preventDefault(); return; }
    if (code === 403) { openSetup(); return; } /* red */
    if (code === 404) { send({ cmd: 'refreshGuide' }); notice('Updating the TV guide…', true); return; } /* green */
    if (code === 405) { armBar(); sendKey({ cmd: 'key', key: 'favorite' }); return; } /* yellow */
    /* While the search field has the focus, the TV keyboard owns the keys;
     * Done, Cancel and Back leave it. */
    if (document.activeElement === $('search-input')) {
        var ime = code === IME_DONE || code === 13 ? 'done' : code === IME_CANCEL ? 'cancel' : code === 10009 ? 'back' : null;
        if (ime) {
            e.preventDefault();
            if (e.repeat) return;
            if (ime === 'done') flushSearch();
            keyTimes.push(performance.now());
            sendKey({ cmd: 'key', key: ime });
        }
        return;
    }
    var key = KEYS[code];
    if (!key) return;
    e.preventDefault();
    if (e.repeat && ONCE[key]) return;
    /* Left held in the guide opens the groups column (Android TV's Long
     * Left): its first repeat sends `groups` once, later repeats nothing.
     * Only a press that started on the guide screen does (a Left from full
     * screen opens the guide, and holding it must not go on). */
    if (key === 'left') {
        if (!e.repeat) leftFrom = view && view.mode;
        else {
            if (leftFrom === 'guide' && !leftHeld) {
                leftHeld = true;
                keyTimes.push(performance.now());
                sendKey({ cmd: 'key', key: 'groups' });
            }
            return;
        }
    }
    if (barShown) armBar();
    keyTimes.push(performance.now());
    /* Keys that may start a stream: the zap timing starts here. */
    var full = view && view.mode === 'full';
    if (key === 'ok' || key === 'chup' || key === 'chdown' || (full && (key === 'up' || key === 'down'))) {
        player.askedAt = performance.now();
    } else if (!full) {
        player.askedAt = null;
    }
    sendKey({ cmd: 'key', key: key, repeat: e.repeat });
    if (e.repeat) repeated = true;
});

var leftHeld = false, leftFrom = null;
/* A key was repeating: the core builds what it put off meanwhile (the
 * groups column's list). */
var repeated = false;
document.addEventListener('keyup', function (e) {
    if (e.keyCode === 37) leftHeld = false;
    if (repeated) {
        repeated = false;
        sendKey({ cmd: 'settle' });
    }
});

document.addEventListener('visibilitychange', function () {
    if (document.hidden) {
        /* What the core thinks plays, also a zap still waiting to open. */
        var wanted = player.wanted;
        player.stop();
        player.resume = wanted;
    } else if (player.resume) {
        player.play(player.resume[0], player.resume[1]);
        player.resume = null;
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
