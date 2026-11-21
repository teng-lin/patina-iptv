/*
 * The page: renders the core's views, sends remote keys to it and plays
 * what it asks for with the TV's native player (AVPlay). The state is the
 * core's (worker.js runs it); this file keeps only presentation state.
 * Plain ES2017: the oldest supported engine is Chromium 69 (Tizen 5.5).
 */
'use strict';

var STORAGE_KEY = 'patina.source';
var PREVIEW = [1144, 48, 752, 423]; /* #side + #preview-frame, in 1920x1080 */
var FULL = [0, 0, 1920, 1080];
var ROWS = 9;
var ENTRIES = 11;

/*
 * Timings of this launch, kept in local storage (last 5 launches) so they
 * can be read after a normal launch, without a debugger attached.
 */
var metrics = { at: Date.now(), keys: [], zaps: [] };
function mark(name) {
    if (metrics[name] === undefined) metrics[name] = Math.round(performance.now());
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

/* ---------- player ---------- */
var player = {
    url: null,
    userAgent: null,
    token: 0,
    rect: PREVIEW,
    play: function (url, userAgent) {
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
            av.setDisplayRect(this.rect[0], this.rect[1], this.rect[2], this.rect[3]);
            av.setDisplayMethod('PLAYER_DISPLAY_MODE_LETTER_BOX');
            av.setListener({
                onbufferingstart: function () { if (token === player.token) loading(true, 'Buffering…'); },
                onbufferingcomplete: function () { if (token === player.token) loading(false); },
                oncurrentplaytime: function () {
                    if (timed || token !== player.token) return;
                    timed = true;
                    metrics.zaps.push(Math.round(performance.now() - asked));
                    saveMetrics();
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
            try { webapis.avplay.setDisplayRect(r[0], r[1], r[2], r[3]); } catch (e) { /* not open yet */ }
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
function loading(on, text) {
    $('loading').classList.toggle('hidden', !on);
    if (text) $('loading-text').textContent = text;
    /* Buffering over the list only darkens the preview, never the list. */
    $('loading').style.background = view && view.ready ? 'transparent' : '#141C21';
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
    if (!v) return;
    var drawnAt = performance.now();
    while (keyTimes.length) metrics.keys.push(Math.round(drawnAt - keyTimes.shift()));
    if (v.ready) mark('channelsShown');
    if (v.guide) mark('guideShown');
    if (!v.ready) {
        loading(true, statusText(v.status) || 'Loading…');
        return;
    }
    if (!$('loading').classList.contains('hidden') && $('loading').style.background !== 'transparent') loading(false);
    var full = v.mode === 'full';
    document.body.classList.toggle('full', full);
    player.setRect(full ? FULL : PREVIEW);
    $('status').textContent = statusText(v.status);

    var groups = $('group-rows').children;
    for (var g = 0; g < groups.length; g++) {
        var item = v.groups.items[g];
        groups[g].style.visibility = item ? 'visible' : 'hidden';
        if (!item) continue;
        groups[g].textContent = item.name;
        groups[g].classList.toggle('focus', v.groups.start + g === v.groups.focus);
        groups[g].classList.toggle('shown', item.shown);
    }
    $('groups').classList.toggle('active', v.pane === 'groups');
    $('channels').classList.toggle('active', v.pane === 'channels');
    $('list-title').textContent = v.title;
    $('list-count').textContent = v.rows.total ? (v.rows.focus + 1) + ' / ' + v.rows.total : 'No channels';

    var rows = $('channel-rows').children;
    for (var r = 0; r < rows.length; r++) {
        var c = v.rows.items[r];
        rows[r].style.visibility = c ? 'visible' : 'hidden';
        if (!c) continue;
        var li = rows[r];
        li.children[0].textContent = c.number;
        var img = li.children[1];
        var logo = c.logo || '';
        if (img.getAttribute('data-src') !== logo) {
            img.setAttribute('data-src', logo);
            img.style.visibility = logo ? 'visible' : 'hidden';
            if (logo) img.src = logo; else img.removeAttribute('src');
        }
        li.children[2].textContent = c.name;
        li.children[3].textContent = c.now ? clock(c.now.start) + '  ' + c.now.title : '';
        li.classList.toggle('focus', v.rows.start + r === v.rows.focus);
        li.classList.toggle('playing', c.playing);
    }

    var p = v.playing;
    $('info').style.visibility = p ? 'visible' : 'hidden';
    if (p) {
        $('info-channel').textContent = p.number + '  ' + p.name;
        $('info-now').textContent = programLine(p.now);
        $('info-progress').firstChild.style.width = Math.round(progress(p.now) * 100) + '%';
        $('info-next').textContent = p.next ? 'Next  ' + programLine(p.next) : '';
    }
    if (!full) $('banner').classList.add('hidden');
}
function scheduleRender() {
    if (drawn) return;
    drawn = true;
    requestAnimationFrame(render);
}

function showBanner(channel) {
    if (!view || view.mode !== 'full') return;
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
    case 'exit': $('confirm').classList.remove('hidden'); break;
    case 'fatal': loading(true, m.text); break;
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
    send({ cmd: 'source', source: source, utcOffsetSecs: -new Date().getTimezoneOffset() * 60 });
}
function setupKey(code) {
    if (setup.editing) {
        /* The IME owns the keys; Done/OK, Cancel and Back leave the field. */
        if (code === 13 || code === 65376 || code === 65385 || code === 10009) {
            setup.editing.blur();
            setup.editing = null;
        }
        return;
    }
    var fields = setupFields();
    var field = fields[setup.index];
    if (code === 38) setup.index--;
    else if (code === 40) setup.index++;
    else if (code === 13) {
        if (field.hasAttribute('data-kind-toggle')) setup.kind = setup.kind === 'xtream' ? 'm3u' : 'xtream';
        else if (field.hasAttribute('data-save')) {
            var source = setup.kind === 'xtream'
                ? { kind: 'xtream', server: $('server').value, username: $('username').value, password: $('password').value }
                : { kind: 'm3u', url: $('m3u').value };
            localStorage.setItem(STORAGE_KEY, JSON.stringify(source));
            closeSetup();
            load(source);
            return;
        } else {
            setup.editing = field.querySelector('input');
            setup.editing.focus();
            return;
        }
    } else if (code === 10009) {
        if (savedSource()) closeSetup(); else exit();
        return;
    }
    drawSetup();
}

/* ---------- keys ---------- */
var KEYS = { 37: 'left', 38: 'up', 39: 'right', 40: 'down', 13: 'ok', 10009: 'back',
    427: 'chup', 428: 'chdown', 33: 'pageup', 34: 'pagedown' };

function exit() {
    player.stop();
    try { tizen.application.getCurrentApplication().exit(); } catch (e) { window.close(); }
}

document.addEventListener('keydown', function (e) {
    var code = e.keyCode;
    if (setup.open) { setupKey(code); e.preventDefault(); return; }
    if (!$('confirm').classList.contains('hidden')) {
        e.preventDefault();
        if (code === 13 || code === 10009) exit();
        else $('confirm').classList.add('hidden');
        return;
    }
    if (code === 403) { openSetup(); return; } /* red */
    if (code === 404) { send({ cmd: 'refreshGuide' }); notice('Updating the TV guide…', true); return; } /* green */
    if (code === 405) { send({ cmd: 'key', key: 'favorite' }); return; } /* yellow */
    var key = KEYS[code];
    if (!key) return;
    e.preventDefault();
    keyTimes.push(performance.now());
    /* Keys that may start a stream: the zap timing starts here. */
    var full = view && view.mode === 'full';
    if (key === 'ok' || key === 'chup' || key === 'chdown' || (full && (key === 'up' || key === 'down'))) {
        player.askedAt = performance.now();
    } else if (!full) {
        player.askedAt = null;
    }
    send({ cmd: 'key', key: key });
});

document.addEventListener('visibilitychange', function () {
    if (document.hidden) {
        var url = player.url, ua = player.userAgent;
        player.stop();
        player.resume = url ? [url, ua] : null;
    } else if (player.resume) {
        player.play(player.resume[0], player.resume[1]);
        player.resume = null;
    }
});

(function start() {
    mark('scriptStart');
    if (hasTizen()) {
        ['ChannelUp', 'ChannelDown', 'ColorF0Red', 'ColorF1Green', 'ColorF2Yellow'].forEach(function (k) {
            try { tizen.tvinputdevice.registerKey(k); } catch (e) { /* the remote may lack it */ }
        });
    }
    makeRows($('group-rows'), ENTRIES, false);
    makeRows($('channel-rows'), ROWS, true);
    var source = savedSource();
    if (source) { loading(true, 'Connecting to your provider…'); load(source); } else openSetup();
    /* Program progress and "now" move on with the clock. */
    setInterval(function () { if (view && view.ready && !setup.open) send({ cmd: 'view' }); }, 30000);
})();
