/*
 * Patina's host page for its NSH interface on Samsung Tizen TVs: the
 * player component and the app's start. app-config.js (written by
 * build.sh) gives the module's version. The Worker's host services are in
 * patina-worker.js.
 *
 * `tv.player` is the platform's AVPlay component (NshTizen.avplay), which
 * plays a source's URL with its token, pause and seek, and reports state,
 * size, first render, position and timeline, behind one rule of Patina's
 * player contract it cannot keep: AVPlay sends no request headers of the
 * app's, so a source that needs them reports an error instead of playing
 * without them (iptv-player-mpv's `source.headers`).
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

function PatinaPlayer(ctx) {
    var emit = ctx.emit;
    ctx.emit = function (name, data) {
        note(window.__patinaPlayer.events, { name: name, state: data && data.state, code: data && data.code, token: data && data.token });
        return emit.apply(ctx, arguments);
    };
    var inner = NshTizen.avplay(ctx);
    var refused = null;
    function filtered(props) {
        var source = props.source;
        if (source && source.headers && Object.keys(source.headers).length) {
            if (refused !== source.token) {
                refused = source.token;
                ctx.emit('error', { code: 'headersUnsupported' });
            }
            var copy = {};
            for (var k in props) copy[k] = props[k];
            copy.source = null;
            return copy;
        }
        refused = null;
        return props;
    }
    return {
        props: function (changed, all, removed) {
            var source = all.source;
            note(window.__patinaPlayer.props, {
                changed: Object.keys(changed),
                source: !!source,
                token: source ? source.token : null,
                headers: source && source.headers ? Object.keys(source.headers).length : 0,
                paused: all.paused
            });
            if (inner.props) inner.props(filtered(changed), filtered(all), removed);
        },
        frame: function (rect) { if (inner.frame) inner.frame(rect); },
        command: function (name, args) { if (inner.command) inner.command(name, args); },
        destroy: function () { if (inner.destroy) inner.destroy(); }
    };
}

NshTizen.registerComponent('tv.player', PatinaPlayer);
NshTizen.start({
    container: document.getElementById('app'),
    module: 'app.wasm',
    version: window.NSH_MODULE_VERSION,
    worker: 'nsh-worker.js',
    scripts: ['patina-worker.js'],
    fonts: [],
    onExit: function () {
        try { tizen.application.getCurrentApplication().exit(); } catch (e) { /* not a TV */ }
    },
    onError: function (text) {
        if (window.console) console.error('Patina: ' + text);
    }
});
