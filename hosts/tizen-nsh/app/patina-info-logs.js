/*
 * Development builds only (build.sh with NSH_INFO_LOGS=1 appends this to
 * patina-worker.js): the app's info logs reach the page's __nsh.logs too,
 * not only its warnings and errors. Patina's logs name no credentials or
 * addresses, but they do name sources and counts: never ship this.
 */
'use strict';
(function () {
    var engine = null;
    NshTizenWorker.onInstance(function (exports, e) { engine = e; });
    NshTizenWorker.addImports('nsh_tz', {
        log: function (level, ptr, len) {
            if (engine) engine.out({ t: 'log', level: level, text: engine.str(ptr, len) });
        }
    });
})();
