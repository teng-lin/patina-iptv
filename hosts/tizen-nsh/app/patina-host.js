/*
 * Patina's host page for its NSH interface on Samsung Tizen TVs: the
 * player component and the app's start. app-config.js (written by
 * build.sh) gives the module's version. The Worker's host services are in
 * patina-worker.js.
 *
 * `tv.player` is the platform's AVPlay component (NshTizen.avplay), which
 * plays a source's URL with its token, pause and seek, and reports state,
 * size, picture loss, first render, position and timeline, behind one rule
 * of Patina's player contract it cannot keep: AVPlay sends no request
 * headers of the app's, so a source that needs them reports an error
 * instead of playing without them (iptv-player-mpv's `source.headers`).
 * User-Agent is a forbidden page header, so AVPlay sends its own.
 *
 * Plain ES2017 (Chromium 69).
 */
'use strict';

/* What the player did, for DevTools: never a source's address or
 * headers (they can carry an account's credentials). */
window.__patinaPlayer = { props: [], events: [] };
function note(list, entry) {
    entry.at = Math.round(performance.now());
    list.push(entry);
    if (list.length > 50) list.shift();
}

/* Headers a page cannot send whatever it does: the TV's own go instead. */
var BROWSER_OWNED = /^user-agent$/i;

function required(headers) {
    return Object.keys(headers || {}).filter(function (name) { return !BROWSER_OWNED.test(name); });
}

function PatinaPlayer(ctx) {
    var emit = ctx.emit;
    ctx.emit = function (name, data) {
        note(window.__patinaPlayer.events, { name: name, state: data && data.state, code: data && data.code, token: data && data.token });
        return emit.apply(ctx, arguments);
    };
    var inner = NshTizen.avplay(ctx);
    /* The token of the source last refused: one error per source. Only
     * the full props (`all`) say what the source is now; `changed` often
     * has no source at all (a pause, a seek), which says nothing about it. */
    var refused = null;
    function filtered(props, full) {
        var source = props.source;
        if (source && required(source.headers).length) {
            if (full && refused !== source.token) {
                refused = source.token;
                ctx.emit('error', { code: 'headersUnsupported' });
            }
            var copy = {};
            for (var k in props) copy[k] = props[k];
            copy.source = null;
            return copy;
        }
        if (full) refused = null;
        return props;
    }
    return {
        props: function (changed, all, removed) {
            var source = all.source;
            note(window.__patinaPlayer.props, {
                changed: Object.keys(changed),
                source: !!source,
                token: source ? source.token : null,
                headers: source ? Object.keys(source.headers || {}) : [],
                paused: all.paused
            });
            if (inner.props) inner.props(filtered(changed, false), filtered(all, true), removed);
        },
        frame: function (rect) { if (inner.frame) inner.frame(rect); },
        command: function (name, args) { if (inner.command) inner.command(name, args); },
        destroy: function () { if (inner.destroy) inner.destroy(); }
    };
}

function leave() {
    try { tizen.application.getCurrentApplication().exit(); } catch (e) { /* not a TV */ }
}

NshTizen.registerComponent('tv.player', PatinaPlayer);
NshTizen.start({
    container: document.getElementById('app'),
    module: 'app.wasm',
    version: window.NSH_MODULE_VERSION,
    worker: 'nsh-worker.js',
    scripts: ['patina-worker.js'],
    /* Patina's TV layout is made for 960 x 540 points (Android TV's 1080p
     * screen): drawn at 2 page pixels per point it has its TV size. */
    scale: 2,
    /* Every Back press is the app's, as on Android TV: two quick presses
     * step back twice, and iptv-nav tells a tap from a hold. (The
     * platform's default drops a Back within 600 ms of the last.) */
    backDebounceMs: 0,
    /* iptv-app's fonts (its `browser-package` feature leaves them out of
     * the module): loaded in the Worker and on the page before it starts. */
    fonts: [
        { family: 'Roboto', url: 'fonts/Roboto-Regular.woff2', weight: 400 },
        { family: 'Roboto', url: 'fonts/Roboto-Medium.woff2', weight: 500 },
        { family: 'Roboto Condensed', url: 'fonts/RobotoCondensed-Regular.woff2', weight: 400 }
    ],
    /* Back at the top level asks first (iptv-app's `confirm_leaving`,
     * Samsung's exit confirmation), and Yes sends {leave: true}. The
     * remote's Exit key is never registered: the system handles it. */
    onHost: function (message) {
        if (message && message.leave) leave();
    },
    onExit: leave,
    onError: function (text) {
        if (window.console) console.error('Patina: ' + text);
    }
});
