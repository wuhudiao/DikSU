
(function() {
    // Logged with a recognisable tag: a module page that comes up blank is otherwise
    // indistinguishable from one whose init threw.
    window.addEventListener('error', function(e) {
        console.error('[KSU Bridge] ERROR: ' + e.message + ' at ' + e.filename + ':' + e.lineno);
    });
    window.addEventListener('unhandledrejection', function(e) {
        console.error('[KSU Bridge] UNHANDLED PROMISE: ' + (e.reason ? (e.reason.message || e.reason) : 'unknown'));
    });

    if (window.ksu) return;

    // ---- talking to the host page -------------------------------------------------
    // The ksud page owns the frame this runs in, so what a module asks of its host (close
    // me, go fullscreen, show a toast) travels as a message. The module cannot reach the
    // page directly: they are same-origin, but the page's own state is not its business.
    function toHost(payload) {
        try { window.parent.postMessage(payload, '*'); } catch (e) {}
    }

    // ---- the synchronous surface the APK exposes ----------------------------------
    // The APK's bridge is synchronous and string-based: `ksu.moduleInfo()` hands back JSON
    // *text*, and modules do `JSON.parse(ksu.moduleInfo())`. Returning a promise instead
    // throws on the first line of such a module's init, which is what stops its page from
    // starting at all. So these calls go out synchronously, and what comes back is a
    // String object: it stringifies to the JSON exactly like the APK's, and it also
    // carries `then`/`catch` so a module written against the promise-based API can await
    // it. The network cost is nil — this is a loopback request to the ksud process.
    function request(path, body) {
        try {
            var xhr = new XMLHttpRequest();
            xhr.open(body === undefined ? 'GET' : 'POST', path, false);
            if (body !== undefined) xhr.setRequestHeader('Content-Type', 'application/json');
            xhr.send(body === undefined ? null : body);
            if (xhr.status < 200 || xhr.status >= 300) {
                console.error('[KSU Bridge] ' + path + ' -> HTTP ' + xhr.status);
                return null;
            }
            var payload = JSON.parse(xhr.responseText);
            if (!payload || payload.success !== true) {
                console.error('[KSU Bridge] ' + path + ' -> ' + ((payload && payload.error) || 'failed'));
                return null;
            }
            return payload.data;
        } catch (e) {
            console.error('[KSU Bridge] ' + path + ' failed: ' + e);
            return null;
        }
    }

    // A String object rather than a primitive, so `JSON.parse(...)` behaves as it does with
    // the APK's return value while `.then`/`await` still yield the parsed form.
    function result(text, parse) {
        var value = new String(text === null || text === undefined ? 'null' : text);
        value.then = function(onOk, onErr) {
            return Promise.resolve()
                .then(function() { return parse ? JSON.parse(value.valueOf()) : value.valueOf(); })
                .then(onOk, onErr);
        };
        value.catch = function(onErr) { return value.then(null, onErr); };
        return value;
    }

    function jsonResult(text) { return result(text, true); }

    // `await ksu.exec(cmd)` has to settle on what the documented API says it settles on:
    // `{ errno, stdout, stderr }`. It used to settle on the bare stdout string, so a module
    // reading `result.errno` — the field the official `@kernelsu/webui` types declare, and how
    // a module asks whether its command actually succeeded — saw `undefined` and reported a
    // failure for a command that ran.
    function execResult(code, stdout, stderr) {
        var text = stdout === null || stdout === undefined ? '' : String(stdout);
        // What the promise settles on: a String object, so the documented field reads
        // (`result.errno`) and the older text reads (`result.trim()`) both work. It carries
        // no `then` of its own, or awaiting it would resolve it again, forever.
        var results = new String(text);
        results.errno = code;
        results.stdout = text;
        results.stderr = stderr === null || stderr === undefined ? '' : String(stderr);

        var value = new String(text);
        value.then = function(onOk, onErr) { return Promise.resolve(results).then(onOk, onErr); };
        value.catch = function(onErr) { return value.then(null, onErr); };
        return value;
    }

    // Hand a finished exec to whatever the caller passed: a callback function, or the name of
    // one on the page (what the APK's string-based bridge takes). Reports whether anyone was
    // told, so `exec(cmd)` without a callback can return the output instead.
    function deliverExec(callbackName, code, stdout, stderr) {
        if (typeof callbackName === 'function') {
            try { callbackName(code, stdout, stderr); } catch (e) { console.error('[KSU Bridge] exec callback threw', e); }
            return true;
        }
        if (typeof callbackName === 'string' && callbackName) {
            var cb = window[callbackName];
            if (typeof cb === 'function') {
                try { cb(code, stdout, stderr); } catch (e) { console.error('[KSU Bridge] exec callback threw', e); }
            } else {
                console.warn('[KSU Bridge] exec callback "' + callbackName + '" is not a function');
            }
            return true;
        }
        return false;
    }

    // Which module this page belongs to, from the path it was served at.
    function moduleId() {
        var rest = location.pathname.split('/modweb/')[1] || '';
        return decodeURIComponent(rest.split('/')[0]);
    }

    function parseMaybe(raw) {
        if (typeof raw !== 'string') return raw;
        try { return JSON.parse(raw); } catch (e) { return null; }
    }

    // Text that is meant as JSON rather than as a callback name. The APK's bridge takes
    // strings for both, so which one a second argument is has to be decided by its shape.
    function looksLikeJson(text) {
        var trimmed = String(text).trim();
        var first = trimmed.charAt(0);
        return (first === '{' || first === '[') && parseMaybe(trimmed) !== null;
    }

    // ---- small event plumbing, matching the APK's ChildProcess shape --------------
    function makeStream() {
        var handlers = {};
        return {
            on: function(event, cb) {
                (handlers[event] = handlers[event] || []).push(cb);
                return this;
            },
            emit: function(event, value) {
                (handlers[event] || []).slice().forEach(function(cb) {
                    try { cb(value); } catch (e) { console.error('[KSU Bridge] stream listener threw', e); }
                });
            }
        };
    }

    function makeChild() {
        var handlers = {};
        var child = {
            id: null,
            stdout: makeStream(),
            stderr: makeStream(),
            _killed: false,
            on: function(event, cb) {
                (handlers[event] = handlers[event] || []).push(cb);
                return child;
            },
            wait: function() { return child._done; },
            kill: function() {
                child._killed = true;
                if (child.id) request('/api/module/spawn/close', JSON.stringify({ id: child.id }));
            },
            // Resolves with the exit code. Deliberately not with `child` itself: resolving a
            // promise with a thenable object that resolves to itself never settles.
            then: function(onOk, onErr) { return child._done.then(onOk, onErr); }
        };
        child._done = new Promise(function(resolve) { child._resolve = resolve; });
        child._lifecycle = function(event, arg) {
            (handlers[event] || []).slice().forEach(function(cb) {
                try { cb(arg); } catch (e) { console.error('[KSU Bridge] listener threw', e); }
            });
        };
        return child;
    }

    // The APK emits onto a global the module names (`cb.stdout.emit('data', ...)`), so if
    // that object exists its emit takes the events too. Modules that keep the returned
    // child instead are served by the object above.
    function forward(name, channel, event, value) {
        if (typeof name !== 'string' || !name) return;
        var target = window[name];
        if (!target) return;
        try {
            var sink = channel ? target[channel] : target;
            if (sink && typeof sink.emit === 'function') sink.emit(event, value);
        } catch (e) {
            console.error('[KSU Bridge] forwarding ' + event + ' to ' + name + ' failed', e);
        }
    }

    var SPAWN_POLL_MS = 120;

    var ksu = {
        // ---- shell ---------------------------------------------------------------
        // exec(cmd) -> stdout, synchronously, like the APK.
        // exec(cmd, cb) / exec(cmd, options, cb) -> cb(code, stdout, stderr).
        exec: function(cmd, options, callbackName) {
            if (typeof options === 'function') { callbackName = options; options = null; }

            // The APK's bridge is `exec(cmd, callbackFunc)` — a plain *string* callback name in
            // the second position. Reading that as options made the call return a value and
            // never call back, so a module that waits on it (the shape the APK documents, and
            // what a hand-written `ksu.exec(cmd, 'cb')` does) saw its read never complete.
            if (typeof options === 'string' && callbackName === undefined && !looksLikeJson(options)) {
                callbackName = options;
                options = null;
            }

            var body = { cmd: String(cmd) };
            var opts = parseMaybe(options);
            if (opts && typeof opts === 'object') {
                if (opts.cwd) body.options = { cwd: String(opts.cwd) };
                if (opts.env) body.env = opts.env;
            }

            var data = request('/api/module/exec', JSON.stringify(body));
            var code = data && typeof data.code === 'number' ? data.code : -1;
            var stdout = (data && data.stdout) || '';
            // stdout and stderr survive a non-zero exit: `success` is the API's status, not
            // the command's, so `grep` finding nothing does not blank out its output.
            var stderr = data ? ((data.stderr) || '') : '命令未执行：ksud 服务不可用';

            if (deliverExec(callbackName, code, stdout, stderr)) return;
            return execResult(code, stdout, stderr);
        },

        // spawn(command, args, options, cb) -> child: stdout/stderr stream as they are
        // produced, then 'exit' (and 'error' when the exit code is non-zero), like the APK.
        spawn: function(command, args, options, callbackName) {
            // `args` and `options` arrive as JSON *text* when a module uses the official JS
            // API: the APK's bridge is string-based, so modules stringify them first.
            // Accepting only a real array silently dropped every argument — `['-c','cat …']`
            // ran as a bare command — which a module reads back as "nothing there".
            var argList = parseMaybe(args);
            if (argList !== null && !Array.isArray(argList) && typeof argList === 'object') {
                // `(command, options, cb)`: no argument list was passed.
                if (typeof options === 'string' && callbackName === undefined) callbackName = options;
                options = args;
                argList = [];
            }
            if (!Array.isArray(argList)) argList = [];

            if (typeof options === 'function') { callbackName = options; options = null; }
            if (typeof options === 'string' && callbackName === undefined && !looksLikeJson(options)) {
                callbackName = options;
                options = null;
            }

            var child = makeChild();
            var body = { cmd: String(command), args: argList.map(String) };
            var opts = parseMaybe(options);
            if (opts && typeof opts === 'object') {
                if (opts.cwd) body.cwd = String(opts.cwd);
                if (opts.env) body.env = opts.env;
            }

            var started = request('/api/module/spawn', JSON.stringify(body));
            if (!started || started.id === undefined) {
                var failure = new Error('无法启动命令：ksud 服务不可用');
                child._lifecycle('error', failure);
                forward(callbackName, null, 'error', failure);
                child._lifecycle('exit', -1);
                forward(callbackName, null, 'exit', -1);
                child._resolve(-1);
                return child;
            }
            child.id = started.id;

            var deliver = function(channel, text) {
                child[channel].emit('data', text);
                forward(callbackName, channel, 'data', text);
            };

            var finish = function(code) {
                child._lifecycle('exit', code);
                forward(callbackName, null, 'exit', code);
                if (code !== 0) {
                    var err = new Error('命令退出码 ' + code);
                    err.exitCode = code;
                    child._lifecycle('error', err);
                    forward(callbackName, null, 'error', err);
                }
                child._resolve(code);
            };

            // Async and single-flight: polling with a blocking request would freeze the page
            // it is meant to keep responsive.
            var tick = function() {
                if (child._killed) return;
                fetch('/api/module/spawn/output?id=' + child.id)
                    .then(function(r) { return r.json(); })
                    .then(function(payload) {
                        if (!payload || payload.success !== true) { finish(-1); return; }
                        var d = payload.data || {};
                        if (d.stdout) deliver('stdout', d.stdout);
                        if (d.stderr) deliver('stderr', d.stderr);
                        if (d.alive) setTimeout(tick, SPAWN_POLL_MS);
                        else finish(typeof d.code === 'number' ? d.code : -1);
                    })
                    .catch(function(e) {
                        // The server going away mid-command is reported as an error rather
                        // than left hanging, so the module can at least say so.
                        console.error('[KSU Bridge] spawn poll failed', e);
                        finish(-1);
                    });
            };
            tick();
            return child;
        },

        // ---- module and package information ---------------------------------------
        // All three return JSON text, as the APK does.
        moduleInfo: function() {
            var data = request('/api/module/info?id=' + encodeURIComponent(moduleId()));
            return jsonResult(JSON.stringify(data === null || data === undefined ? {} : data));
        },

        listPackages: function(type) {
            var data = request('/api/module/packages?type=' + encodeURIComponent(type || ''));
            return jsonResult(JSON.stringify(data || []));
        },

        // getPackagesInfo accepts what the APK accepts: a JSON array of package names.
        // An already-parsed array works too, since callers usually build it in JS.
        getPackagesInfo: function(namesJson) {
            var names = parseMaybe(namesJson);
            if (!Array.isArray(names)) names = [];
            names = names.map(function(entry) {
                if (typeof entry === 'string') return entry;
                return (entry && entry.packageName) || '';
            }).filter(Boolean);
            var data = request('/api/module/packagesinfo?names=' + names.map(encodeURIComponent).join(','));
            return jsonResult(JSON.stringify(data || []));
        },

        // ---- misc ------------------------------------------------------------------
        toast: function(msg) {
            console.log('[KSU Toast]', msg);
            toHost({ ksu: 'toast', message: String(msg) });
        },

        getProp: function(key) {
            var data = request('/api/module/prop?key=' + encodeURIComponent(key));
            return result(data === null || data === undefined ? '' : String(data), false);
        },

        setProp: function(key, value) {
            request('/api/module/prop', JSON.stringify({ key: String(key), value: String(value) }));
        },

        exit: function() { toHost({ ksu: 'exit' }); },

        // Host-side fullscreen: there is no window to hide out here, but the page can put
        // the frame the module runs in full screen.
        fullScreen: function(enable) { toHost({ ksu: 'fullScreen', enable: enable !== false }); },

        // Reported to the host so the frame's title follows the module's own page.
        setTitle: function(title) { toHost({ ksu: 'title', title: String(title) }); },

        // WebView-only abilities. Defined so calling one is a no-op rather than a crash.
        hideSystemUI: function() {},
        showSystemUI: function() {},
        enableEdgeToEdge: function() {},
        onAddElement: function() {},
        processOptions: function() {}
    };

    // Anything not implemented fails loudly instead of silently doing nothing: a module
    // that awaits a missing API should see why, not hang.
    window.ksu = new Proxy(ksu, {
        get: function(target, prop) {
            if (prop in target) return target[prop];
            console.warn('[KSU Bridge] ksu.' + String(prop) + ' is not implemented by the ksud web UI');
            return function() {
                return Promise.reject(new Error('ksu.' + String(prop) + ' is not implemented by the ksud web UI'));
            };
        }
    });

    // ---- pages that are taller than the frame -------------------------------------
    // A module whose CSS pins the page to the viewport — `height: 100vh` plus `overflow:
    // hidden` on html/body, which is what this module's style.css does — clips whatever does
    // not fit and leaves nothing scrollable. The content below the fold then cannot be
    // reached at all: no gesture moves it and the buttons in it cannot be tapped. The module
    // cannot be edited from out here, so the host un-pins the document instead, and only
    // while that is genuinely what is happening. Three conditions, all about the layout in
    // front of us rather than about any particular module:
    //   1. the document itself cannot scroll;
    //   2. something is really cut off below the viewport (measured with rects — a clipped
    //      body reports its own box as scrollHeight, so the reported height cannot be used);
    //   3. nothing inside scrolls either, so a page that ships its own scroller is left alone.
    // `height: auto` with `min-height: 100%` keeps a page that fits looking identical while
    // letting a clipped one grow and scroll: children sized with `height: 100%` fall back to
    // auto, and it is that fallback which removes the clipping.
    function unclipIfCutOff() {
        var de = document.documentElement;
        var body = document.body;
        if (!de || !body || de.scrollHeight > de.clientHeight + 1) return;

        var limit = window.innerHeight + 1;
        var cut = false;
        var elements = body.getElementsByTagName('*');
        for (var i = 0; i < elements.length && !cut; i++) {
            if (elements[i].getBoundingClientRect().bottom > limit) cut = true;
        }
        if (!cut) return;

        for (var j = 0; j < elements.length; j++) {
            var el = elements[j];
            if (el.scrollHeight <= el.clientHeight + 1) continue;
            var overflowY = window.getComputedStyle(el).overflowY;
            if (overflowY === 'auto' || overflowY === 'scroll') return;
        }

        var style = document.createElement('style');
        style.textContent =
            'html, body { height: auto !important; min-height: 100% !important; overflow: visible !important; }';
        document.head.appendChild(style);
    }

    // After `load`, so the page's own stylesheets and scripts have had their say about the
    // layout. The bridge is injected into the head, where `body` may not exist yet.
    if (document.readyState === 'complete') unclipIfCutOff();
    else window.addEventListener('load', unclipIfCutOff);
})();
