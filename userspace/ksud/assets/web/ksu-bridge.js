
(function() {
    window.addEventListener('error', function(e) {
        console.error('[KSU Bridge] ERROR: ' + e.message + ' at ' + e.filename + ':' + e.lineno);
    });
    window.addEventListener('unhandledrejection', function(e) {
        console.error('[KSU Bridge] UNHANDLED PROMISE: ' + (e.reason ? (e.reason.message || e.reason) : 'unknown'));
    });

    if (window.ksu) return;

    function toHost(payload) {
        try { window.parent.postMessage(payload, '*'); } catch (e) {}
    }

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

    function execResult(code, stdout, stderr) {
        var text = stdout === null || stdout === undefined ? '' : String(stdout);
        var results = new String(text);
        results.errno = code;
        results.stdout = text;
        results.stderr = stderr === null || stderr === undefined ? '' : String(stderr);

        var value = new String(text);
        value.then = function(onOk, onErr) { return Promise.resolve(results).then(onOk, onErr); };
        value.catch = function(onErr) { return value.then(null, onErr); };
        return value;
    }

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

    function moduleId() {
        var rest = location.pathname.split('/modweb/')[1] || '';
        return decodeURIComponent(rest.split('/')[0]);
    }

    function parseMaybe(raw) {
        if (typeof raw !== 'string') return raw;
        try { return JSON.parse(raw); } catch (e) { return null; }
    }

    function looksLikeJson(text) {
        var trimmed = String(text).trim();
        var first = trimmed.charAt(0);
        return (first === '{' || first === '[') && parseMaybe(trimmed) !== null;
    }

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
        exec: function(cmd, options, callbackName) {
            if (typeof options === 'function') { callbackName = options; options = null; }

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
            var stderr = data ? ((data.stderr) || '') : '命令未执行：ksud 服务不可用';

            if (deliverExec(callbackName, code, stdout, stderr)) return;
            return execResult(code, stdout, stderr);
        },

        spawn: function(command, args, options, callbackName) {
            var argList = parseMaybe(args);
            if (argList !== null && !Array.isArray(argList) && typeof argList === 'object') {
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
                        console.error('[KSU Bridge] spawn poll failed', e);
                        finish(-1);
                    });
            };
            tick();
            return child;
        },

        moduleInfo: function() {
            var data = request('/api/module/info?id=' + encodeURIComponent(moduleId()));
            return jsonResult(JSON.stringify(data === null || data === undefined ? {} : data));
        },

        listPackages: function(type) {
            var data = request('/api/module/packages?type=' + encodeURIComponent(type || ''));
            return jsonResult(JSON.stringify(data || []));
        },

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

        fullScreen: function(enable) { toHost({ ksu: 'fullScreen', enable: enable !== false }); },

        setTitle: function(title) { toHost({ ksu: 'title', title: String(title) }); },

        hideSystemUI: function() {},
        showSystemUI: function() {},
        enableEdgeToEdge: function() {},
        onAddElement: function() {},
        processOptions: function() {}
    };

    window.ksu = new Proxy(ksu, {
        get: function(target, prop) {
            if (prop in target) return target[prop];
            console.warn('[KSU Bridge] ksu.' + String(prop) + ' is not implemented by the ksud web UI');
            return function() {
                return Promise.reject(new Error('ksu.' + String(prop) + ' is not implemented by the ksud web UI'));
            };
        }
    });

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

    if (document.readyState === 'complete') unclipIfCutOff();
    else window.addEventListener('load', unclipIfCutOff);
})();
