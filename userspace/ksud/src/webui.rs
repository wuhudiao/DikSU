//! Web UI HTTP server for KernelSU
//!
//! Provides a lightweight HTTP server on 127.0.0.1 that serves a web-based
//! management interface. This allows managing KernelSU without the Manager APK.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use pinyin::ToPinyin;
use serde::Serialize;

#[cfg(target_os = "android")]
use crate::defs;
#[cfg(target_os = "android")]
use crate::ksucalls;
#[cfg(target_os = "android")]
use crate::module;
#[cfg(target_os = "android")]
use crate::webui_spawn;

const INDEX_HTML: &str = include_str!("../assets/web/index.html");

/// The `ksu` object a module's WebUI expects, in the `<script>` block it is injected as.
///
/// A file of its own rather than a raw string in here: it is three hundred lines of JavaScript,
/// and a stray character in one is invisible until a module's page comes up blank.
const KSU_BRIDGE: &str = concat!(
    "<script>",
    include_str!("../assets/web/ksu-bridge.js"),
    "</script>"
);

/// Kernel information
#[derive(Serialize)]
struct KernelInfo {
    version: i32,
    uapi_version: u32,
    runtime_mode: String,
    is_lkm: bool,
    is_late_load: bool,
}

/// Module information
#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct ModuleInfo {
    id: String,
    name: String,
    version: String,
    version_code: String,
    author: String,
    description: String,
    enabled: bool,
    update_available: bool,
    /// Whether the module ships its own WebUI (has a webroot/ directory)
    web: bool,
    /// Whether the module is marked for removal (has remove file)
    remove: bool,
    /// Whether the module has an action.sh script
    has_action: bool,
}

/// Feature state
#[derive(Serialize)]
struct FeatureState {
    id: u32,
    name: String,
    description: String,
    value: u64,
    enabled: bool,
}

/// SU log entry
/// API response wrapper
#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Simple HTTP request
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Headers larger than this are not a real browser request.
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Modules are the largest thing uploaded here, and they run to a few tens of MB. The cap
/// exists so a bogus Content-Length cannot make the server allocate without bound.
const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

/// Why a request could not be read, and what to tell the client about it.
///
/// Answering matters more than it looks: if the connection is simply closed, the page's
/// `fetch` never settles and the UI shows nothing — indistinguishable from the server
/// having died. A status plus a message turns the same failure into a visible error.
enum ParseFailure {
    TooLarge(&'static str),
    Incomplete(&'static str),
    Closed,
}

impl ParseFailure {
    fn status(&self) -> u16 {
        match self {
            Self::TooLarge(_) => 413,
            Self::Incomplete(_) => 408,
            Self::Closed => 400,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::TooLarge(m) | Self::Incomplete(m) => m,
            Self::Closed => "连接已关闭，请求不完整",
        }
    }
}

fn parse_request(stream: &mut TcpStream) -> std::result::Result<HttpRequest, ParseFailure> {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];

    // Read headers
    let header_end = loop {
        if buf.len() >= MAX_HEADER_BYTES {
            return Err(ParseFailure::TooLarge("请求头过大"));
        }
        let n = stream.read(&mut chunk).map_err(|e| {
            log::debug!("reading request headers: {e}");
            ParseFailure::Closed
        })?;
        if n == 0 {
            return Err(ParseFailure::Closed);
        }
        buf.extend_from_slice(&chunk[..n]);

        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end - 4]);
    let mut lines = header_text.lines();

    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().unwrap_or(&"GET").to_string();
    let path = parts.get(1).unwrap_or(&"/").to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    if content_length > MAX_BODY_BYTES {
        return Err(ParseFailure::TooLarge("上传内容过大（上限 512MB）"));
    }

    let mut body = buf[header_end..].to_vec();

    // Read the rest of the body. Anything already buffered past the headers counts.
    while body.len() < content_length {
        let mut body_buf = [0u8; 64 * 1024];
        let n = stream.read(&mut body_buf).map_err(|e| {
            log::warn!(
                "reading request body ({}/{content_length} bytes): {e}",
                body.len()
            );
            ParseFailure::Incomplete("上传中断或超时，请重试")
        })?;
        if n == 0 {
            return Err(ParseFailure::Incomplete("上传中断，请求体不完整"));
        }
        body.extend_from_slice(&body_buf[..n]);
    }

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn send_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    send_response_with(stream, status, content_type, body, "")
}

/// Like [`send_response`], but appends `extra_headers` (which must end in CRLF when
/// non-empty) to the response.
///
/// No CORS headers are sent, deliberately: the frontend is served from this same origin so
/// nothing legitimate needs them, and their absence stops another page from *reading* our
/// responses even if it somehow issues a request.
fn send_response_with(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &str,
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };

    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
        status,
        status_text,
        content_type,
        body.len()
    );

    stream.write_all(response.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn json_response<T: Serialize>(stream: &mut TcpStream, data: &T) -> Result<()> {
    let body = serde_json::to_vec(data)?;
    send_response(stream, 200, "application/json; charset=utf-8", &body)
}

fn error_response(stream: &mut TcpStream, status: u16, msg: &str) -> Result<()> {
    let resp = ApiResponse::<()>::err(msg);
    let body = serde_json::to_vec(&resp)?;
    send_response(stream, status, "application/json; charset=utf-8", &body)
}

/// Shown when someone opens an address whose token is no longer the one in use.
///
/// A navigation gets HTML and the API keeps JSON: reached by opening a bookmarked or
/// restored old address, a bare JSON body on an otherwise blank page reads as "the UI is
/// broken", while this says what happened and what to do about it.
const FORBIDDEN_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>访问令牌已失效</title>
<style>
body{font-family:-apple-system,system-ui,sans-serif;margin:0;padding:32px 24px;background:#f5f5f7;color:#1c1c1e;line-height:1.7}
h1{font-size:18px;margin:0 0 14px}
p{margin:0 0 12px;font-size:14px;color:#3a3a3c}
strong{color:#1c1c1e}
</style></head>
<body>
<h1>这个地址的访问令牌已经失效</h1>
<p>网页端每次启动都会换成新的令牌，所以旧地址（书签、浏览器历史、上次留着的标签页）打不开是正常的。</p>
<p><strong>请回桌面点一下启动器图标</strong>，它会带着当前令牌重新打开网页端。</p>
<p>刚点过还是这一页的话，通常是另一个更早启动的服务还占着同一个端口 —— 再点一次启动器，它会换一个端口启动。</p>
</body></html>"#;

#[cfg(target_os = "android")]
fn get_kernel_info() -> Result<KernelInfo> {
    Ok(KernelInfo {
        version: ksucalls::get_version(),
        uapi_version: ksucalls::uapi_version(),
        runtime_mode: ksucalls::runtime_mode().to_string(),
        is_lkm: ksucalls::is_lkm(),
        is_late_load: ksucalls::is_late_load(),
    })
}

#[cfg(not(target_os = "android"))]
fn get_kernel_info() -> Result<KernelInfo> {
    Ok(KernelInfo {
        version: 0,
        uapi_version: 4,
        runtime_mode: "unknown".to_string(),
        is_lkm: false,
        is_late_load: false,
    })
}

#[cfg(target_os = "android")]
fn get_modules_list() -> Result<Vec<ModuleInfo>> {
    use std::collections::HashMap;

    let mut module_map: HashMap<String, ModuleInfo> = HashMap::new();

    // Helper to read a module directory into ModuleInfo
    let read_module = |path: &std::path::Path, is_update: bool| -> Option<ModuleInfo> {
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return None;
        }

        let prop = module::read_module_prop(path).unwrap_or_default();

        let enabled = !path.join(defs::DISABLE_FILE_NAME).exists()
            && !path.join(defs::REMOVE_FILE_NAME).exists()
            && !is_update;

        let web = path.join(defs::MODULE_WEB_DIR).join("index.html").exists();
        let remove = path.join(defs::REMOVE_FILE_NAME).exists();
        let has_action = path.join(defs::MODULE_ACTION_SH).exists();

        Some(ModuleInfo {
            id: id.clone(),
            name: prop.get("name").cloned().unwrap_or_else(|| id.clone()),
            version: prop.get("version").cloned().unwrap_or_default(),
            version_code: prop.get("versionCode").cloned().unwrap_or_default(),
            author: prop.get("author").cloned().unwrap_or_default(),
            description: prop.get("description").cloned().unwrap_or_default(),
            enabled,
            update_available: is_update,
            web,
            remove,
            has_action,
        })
    };

    // First, read modules_update/ (newly installed, pending reboot)
    if std::path::Path::new(defs::MODULE_UPDATE_DIR).exists() {
        if let Ok(dir) = std::fs::read_dir(defs::MODULE_UPDATE_DIR) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(info) = read_module(&path, true) {
                        module_map.insert(info.id.clone(), info);
                    }
                }
            }
        }
    }

    // Then, read modules/ (existing, active) - don't overwrite update versions
    if std::path::Path::new(defs::MODULE_DIR).exists() {
        module::foreach_module(module::ModuleType::All, |path| {
            if let Some(info) = read_module(path, false) {
                // Only insert if not already present from modules_update/
                if !module_map.contains_key(&info.id) {
                    module_map.insert(info.id.clone(), info);
                }
            }
            Ok(())
        })?;
    }

    // Convert to Vec and sort: update modules first, then by name
    let mut modules: Vec<ModuleInfo> = module_map.into_values().collect();
    modules.sort_by(|a, b| {
        b.update_available
            .cmp(&a.update_available)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(modules)
}

#[cfg(not(target_os = "android"))]
fn get_modules_list() -> Result<Vec<ModuleInfo>> {
    Ok(Vec::new())
}

/// Serve static files from a module's `webroot/` directory (its bundled WebUI)
#[cfg(target_os = "android")]
fn serve_module_web(stream: &mut TcpStream, url_path: &str) -> Result<()> {
    use std::path::Path;
    let rel = url_path.trim_start_matches("/modweb/");
    let (id, file_rel) = match rel.split_once('/') {
        Some((a, b)) => (a, b),
        None => (rel, "index.html"),
    };
    if id.is_empty() || id.contains("..") || id.contains('/') {
        return error_response(stream, 400, "Bad module id");
    }
    let base = Path::new(defs::MODULE_DIR)
        .join(id)
        .join(defs::MODULE_WEB_DIR);
    let mut file_path = base.clone();
    for comp in file_rel.split('/') {
        match comp {
            "" | "." => {}
            ".." => return error_response(stream, 403, "Forbidden"),
            c => file_path.push(c),
        }
    }
    if file_path.is_dir() {
        file_path = file_path.join("index.html");
    }
    if !file_path.starts_with(&base) {
        return error_response(stream, 403, "Forbidden");
    }
    match std::fs::read(&file_path) {
        Ok(data) => {
            let mime = mime_for_name(&file_path.to_string_lossy());
            // No caching, for the same reason the index page has none: a module's WebUI is
            // replaced when the module is updated, and a browser holding the old page (or an
            // old script inside it) makes the update look like it did not install.
            let extra = "Cache-Control: no-store, must-revalidate\r\nPragma: no-cache\r\n";
            // Inject ksu JS bridge into HTML pages (module webuis depend on window.ksu)
            if mime.starts_with("text/html") {
                let html = String::from_utf8_lossy(&data);
                let injected = inject_ksu_bridge(&html);
                return send_response_with(
                    stream,
                    200,
                    "text/html; charset=utf-8",
                    injected.as_bytes(),
                    extra,
                );
            }
            send_response_with(stream, 200, mime, &data, extra)
        }
        Err(_) => error_response(stream, 404, "Module web resource not found"),
    }
}

/// Inject a ksu JavaScript bridge into module webui HTML.
/// KernelSU module webuis expect window.ksu to be available (injected by the Manager app's WebView).
/// This bridge provides basic APIs via fetch to our backend.
fn inject_ksu_bridge(html: &str) -> String {
    let bridge = KSU_BRIDGE;

    if let Some(pos) = html.find("</head>") {
        format!("{}{}{}", &html[..pos], bridge, &html[pos..])
    } else if let Some(pos) = html.find("<body") {
        format!("{}{}{}", &html[..pos], bridge, &html[pos..])
    } else {
        format!("{}{}", bridge, html)
    }
}

/// Serve module assets referenced by absolute paths (e.g., /assets/...) via Referer header.
#[cfg(target_os = "android")]
fn serve_module_asset_by_referer(
    stream: &mut TcpStream,
    url_path: &str,
    req: &HttpRequest,
) -> Result<()> {
    use std::path::Path;

    let referer = match req.headers.get("referer") {
        Some(r) => r.clone(),
        None => return error_response(stream, 404, "No referer header"),
    };

    let module_id = {
        let parts: Vec<&str> = referer.split("/modweb/").collect();
        if parts.len() < 2 {
            return error_response(stream, 404, "Invalid referer");
        }
        parts[1].split('/').next().unwrap_or("").to_string()
    };

    if module_id.is_empty() || module_id.contains("..") {
        return error_response(stream, 400, "Bad module id in referer");
    }

    // Special case: /internal/insets.css
    if url_path == "/internal/insets.css" {
        let module_insets = Path::new(defs::MODULE_DIR)
            .join(&module_id)
            .join(defs::MODULE_WEB_DIR)
            .join("internal/insets.css");
        if module_insets.exists() {
            if let Ok(data) = std::fs::read(&module_insets) {
                return send_response(stream, 200, "text/css; charset=utf-8", &data);
            }
        }
        let default_css = b":root { --window-inset-top: 0px; --window-inset-bottom: 0px; }
";
        return send_response(stream, 200, "text/css; charset=utf-8", default_css);
    }

    let rel_path = url_path.trim_start_matches('/');
    let base = Path::new(defs::MODULE_DIR)
        .join(&module_id)
        .join(defs::MODULE_WEB_DIR);
    let mut file_path = base.clone();
    for comp in rel_path.split('/') {
        match comp {
            "" | "." => {}
            ".." => return error_response(stream, 403, "Forbidden"),
            c => file_path.push(c),
        }
    }

    if !file_path.starts_with(&base) {
        return error_response(stream, 403, "Forbidden");
    }

    match std::fs::read(&file_path) {
        Ok(data) => send_response(
            stream,
            200,
            mime_for_name(&file_path.to_string_lossy()),
            &data,
        ),
        Err(_) => error_response(
            stream,
            404,
            &format!("Module asset not found: {}", url_path),
        ),
    }
}

/// How long `ksu.exec` waits for a command before giving up on it.
///
/// The APK blocks the module's page until the command ends, with no ceiling at all, so a
/// command that never returns wedges the page with no way out. Reusing the spawn machinery
/// means the wait is bounded, the pipes are drained while it runs, and whatever the command
/// did produce is still returned.
#[cfg(target_os = "android")]
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);

/// Execute shell command for module webui bridge (`ksu.exec`).
///
/// `success` describes the API call, not the command — the command's own status is `code`.
/// Reporting the command's exit status as `success` made the JS side throw away stdout and
/// stderr for every command that exited non-zero (`grep`, `[ -f ]`, a failing script),
/// which reads from the module's side as "I cannot see my own state".
#[cfg(target_os = "android")]
fn handle_module_exec(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let body = String::from_utf8_lossy(&req.body);
    let request: ExecRequest = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return error_response(stream, 400, "Invalid JSON"),
    };
    if request.cmd.trim().is_empty() {
        return error_response(stream, 400, "Missing cmd");
    }
    // Logged because a module page that "cannot read" its own state is otherwise invisible
    // from this side: this records what it actually asked for, and what it got back.
    log::info!("module exec: {}", request.cmd);

    let env: Vec<(String, String)> = request.env.into_iter().collect();
    let options = request
        .options
        .unwrap_or_else(|| ExecOptions { cwd: default_cwd() });
    let id = webui_spawn::start(&request.cmd, &[], &options.cwd, &env)?;

    let mut stdout = String::new();
    let mut stderr = String::new();
    let deadline = std::time::Instant::now() + EXEC_TIMEOUT;

    let code = loop {
        let (out, err, alive, code) = webui_spawn::poll(id)?;
        stdout.push_str(&out);
        stderr.push_str(&err);
        if !alive {
            break code.unwrap_or(-1);
        }
        if std::time::Instant::now() >= deadline {
            // Closed rather than left running: the module has already been told it failed,
            // and a stray process would hold a slot in the job table.
            let _ = webui_spawn::close(id);
            stderr.push_str(&format!(
                "\n[ksud] 命令超过 {} 秒未结束，已终止\n",
                EXEC_TIMEOUT.as_secs()
            ));
            break -1;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let _ = webui_spawn::close(id);

    log::info!(
        "module exec done: code={code} stdout {}B stderr {}B",
        stdout.len(),
        stderr.len()
    );

    json_response(
        stream,
        &ApiResponse::ok(serde_json::json!({
            "code": code,
            "stdout": stdout,
            "stderr": stderr,
        })),
    )
}

/// Install a module zip from a path on the device.
///
/// The zip is installed from where it lies rather than being copied first: it is already
/// on the device, and `install_module` reads what it needs out of it.
#[cfg(target_os = "android")]
fn install_from_path(stream: &mut TcpStream, path: &str) -> Result<()> {
    if path.trim().is_empty() {
        return error_response(stream, 400, "请填写模块 zip 的路径");
    }
    let source = std::path::Path::new(path);
    if !source.is_file() {
        return error_response(stream, 404, &format!("找不到文件：{path}"));
    }

    let started = std::time::Instant::now();
    match module::install_module(path) {
        Ok(()) => json_response(
            stream,
            &ApiResponse::ok(format!(
                "安装完成 · 耗时 {}ms",
                started.elapsed().as_millis()
            )),
        ),
        Err(e) => error_response(stream, 500, &format!("{e:#}")),
    }
}

/// Start a background command for `ksu.spawn`.
#[cfg(target_os = "android")]
fn handle_module_spawn(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let body = String::from_utf8_lossy(&req.body);
    let request: SpawnRequest = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return error_response(stream, 400, "Invalid JSON"),
    };
    if request.cmd.trim().is_empty() {
        return error_response(stream, 400, "Missing cmd");
    }

    // Same reason as `exec`: the arguments a module passed are the thing that used to go
    // missing, so they are worth having in the log.
    log::info!("module spawn: {} {:?}", request.cmd, request.args);

    let env: Vec<(String, String)> = request.env.into_iter().collect();
    match webui_spawn::start(&request.cmd, &request.args, &request.cwd, &env) {
        Ok(id) => json_response(stream, &ApiResponse::ok(serde_json::json!({ "id": id }))),
        Err(e) => error_response(stream, 500, &format!("{e:#}")),
    }
}

/// Output produced by a `ksu.spawn` job since the previous poll.
#[cfg(target_os = "android")]
fn handle_module_spawn_output(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let id = match require_param(req, "id")
        .and_then(|v| v.parse::<u32>().with_context(|| "id 不是数字"))
    {
        Ok(id) => id,
        Err(e) => return error_response(stream, 400, &format!("{e:#}")),
    };
    match webui_spawn::poll(id) {
        Ok((stdout, stderr, alive, code)) => json_response(
            stream,
            &ApiResponse::ok(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "alive": alive,
                "code": code,
            })),
        ),
        Err(e) => error_response(stream, 404, &format!("{e:#}")),
    }
}

/// Stop a `ksu.spawn` job (`child.kill()`).
#[cfg(target_os = "android")]
fn handle_module_spawn_close(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let body = String::from_utf8_lossy(&req.body);
    let request: SpawnClose = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return error_response(stream, 400, "Invalid JSON"),
    };
    match webui_spawn::close(request.id) {
        Ok(()) => json_response(
            stream,
            &ApiResponse::ok(serde_json::json!({ "closed": request.id })),
        ),
        Err(e) => error_response(stream, 404, &format!("{e:#}")),
    }
}

/// Get system property for module webui bridge
#[cfg(target_os = "android")]
fn handle_module_prop_get(stream: &mut TcpStream, path: &str) -> Result<()> {
    use std::process::Command;
    let query = path.split('?').nth(1).unwrap_or("");
    let key = query
        .split('&')
        .find(|s| s.starts_with("key="))
        .map(|s| &s[4..])
        .unwrap_or("");
    if key.is_empty() {
        return error_response(stream, 400, "Missing key");
    }
    let output = match Command::new("getprop").arg(key).output() {
        Ok(o) => o,
        Err(_) => return error_response(stream, 500, "Failed to getprop"),
    };
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let result = serde_json::json!({"success": true, "data": value, "error": null});
    send_response(
        stream,
        200,
        "application/json",
        result.to_string().as_bytes(),
    )
}

/// Set system property for module webui bridge
#[cfg(target_os = "android")]
fn handle_module_prop_set(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    use std::process::Command;
    let body = String::from_utf8_lossy(&req.body);
    let data: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return error_response(stream, 400, "Invalid JSON"),
    };
    let key = match data.get("key").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return error_response(stream, 400, "Missing key"),
    };
    let value = match data.get("value").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return error_response(stream, 400, "Missing value"),
    };
    let _ = Command::new("setprop").arg(key).arg(value).output();
    let result = serde_json::json!({"success": true, "data": null, "error": null});
    send_response(
        stream,
        200,
        "application/json",
        result.to_string().as_bytes(),
    )
}

#[cfg(not(target_os = "android"))]
fn serve_module_web(stream: &mut TcpStream, _url_path: &str) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn handle_module_exec(stream: &mut TcpStream, _req: &HttpRequest) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn handle_module_spawn(stream: &mut TcpStream, _req: &HttpRequest) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn handle_module_spawn_output(stream: &mut TcpStream, _req: &HttpRequest) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn handle_module_spawn_close(stream: &mut TcpStream, _req: &HttpRequest) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn bridge_package_names(_kind: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "android"))]
fn bridge_packages_info(_names: &[String]) -> Vec<serde_json::Value> {
    Vec::new()
}

#[cfg(not(target_os = "android"))]
fn handle_module_prop_get(stream: &mut TcpStream, _path: &str) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn handle_module_prop_set(stream: &mut TcpStream, _req: &HttpRequest) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(not(target_os = "android"))]
fn serve_module_asset_by_referer(
    stream: &mut TcpStream,
    _url_path: &str,
    _req: &HttpRequest,
) -> Result<()> {
    error_response(stream, 500, "Not supported on this platform")
}

#[cfg(target_os = "android")]
fn get_features() -> Result<Vec<FeatureState>> {
    let features = [
        (
            0u32,
            "传统 SU 命令支持",
            "允许通过 /system/bin/su 获取 Root 权限",
        ),
        (1, "卸载模块（内核级）", "在内核给需要的应用卸载模块"),
        (4, "隐藏 SELinux 修改", "阻止应用检测 SELinux 修改"),
        (
            999,
            "默认卸载模块",
            "App Profile 中「卸载模块」的全局默认值，如果启用，会将未自定义 Profile 的应用移除所有模块针对系统的修改",
        ),
    ];

    let mut result = Vec::new();
    for (id, name, desc) in &features {
        if *id == 999 {
            // Default umount modules is not a kernel feature, it's an AppProfile setting
            let enabled = ksucalls::get_default_umount_modules().unwrap_or(false);
            result.push(FeatureState {
                id: *id,
                name: name.to_string(),
                description: desc.to_string(),
                value: if enabled { 1 } else { 0 },
                enabled,
            });
            continue;
        }
        match ksucalls::get_feature(*id) {
            Ok((value, enabled)) => {
                result.push(FeatureState {
                    id: *id,
                    name: name.to_string(),
                    description: desc.to_string(),
                    value,
                    enabled,
                });
            }
            Err(_) => {
                result.push(FeatureState {
                    id: *id,
                    name: name.to_string(),
                    description: desc.to_string(),
                    value: 0,
                    enabled: false,
                });
            }
        }
    }

    Ok(result)
}

#[cfg(not(target_os = "android"))]
fn get_features() -> Result<Vec<FeatureState>> {
    Ok(vec![
        FeatureState { id: 0, name: "SU 兼容性模式".to_string(), description: "提升应用兼容性，但可能降低安全性".to_string(), value: 0, enabled: false },
        FeatureState { id: 1, name: "内核自动卸载模块".to_string(), description: "重启后自动卸载所有已安装模块".to_string(), value: 1, enabled: true },
        FeatureState { id: 4, name: "SELinux 隐藏".to_string(), description: "对应用隐藏 SELinux 状态，提升兼容性".to_string(), value: 0, enabled: false },
        FeatureState { id: 999, name: "默认卸载模块".to_string(), description: "App Profile 中「卸载模块」的全局默认值，如果启用，会将未自定义 Profile 的应用移除所有模块针对系统的修改".to_string(), value: 1, enabled: true },
    ])
}

#[cfg(target_os = "android")]
/// Parse the human-readable label directly from an APK file (no extra process)
#[cfg(target_os = "android")]
fn label_from_apk_path(apk_path: &str) -> Option<String> {
    let file = std::fs::File::open(apk_path).ok()?;
    let label = crate::apkparser::extract_label(file)?;
    (!label.is_empty()).then_some(label)
}

/// Get all app display labels by running a small Java program via app_process.
/// This calls Android PackageManager.getApplicationLabel() directly, which is the
/// only reliable way to get localized labels. Returns packageName -> label map.
#[cfg(target_os = "android")]
fn get_app_labels_from_dex() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let dex_path = format!("{}/applabel.dex", crate::defs::BINARY_DIR);
    // Always extract latest dex from embedded assets (overwrite old version)
    if let Ok(data) = crate::assets::get_asset_data("applabel.dex") {
        let _ = std::fs::create_dir_all(crate::defs::BINARY_DIR);
        let _ = std::fs::write(&dex_path, &data);
    }
    if !std::path::Path::new(&dex_path).exists() {
        return map;
    }
    let classpath = format!("-Djava.class.path={}", dex_path);
    let output = match std::process::Command::new("app_process")
        .args([&classpath, "/system/bin", "AppLabelDumper"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return map,
    };
    if !output.status.success() {
        return map;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            let pkg = parts[0].to_string();
            let label = parts[2].to_string();
            if !label.is_empty() && !pkg.is_empty() {
                map.insert(pkg, label);
            }
        }
    }
    map
}

// Cache for app icons: package_name -> (base64, mime)
static APP_ICON_CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<String, (String, String)>>,
> = std::sync::Mutex::new(None);

/// A single APK file's own icon, read out of the package without installing it.
///
/// Same route as an installed app's icon (appicon.dex through app_process), because an adaptive
/// icon only exists once the system's PackageManager composes it: picking a PNG out of the archive
/// by hand gets the foreground layer alone, which looks small and off-centre. The result is cached
/// as a file keyed by path + size + mtime — a folder can hold dozens of APKs, and starting an
/// app_process per row is hundreds of milliseconds each.
#[cfg(target_os = "android")]
fn apk_icon(path: &str) -> anyhow::Result<Vec<u8>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let md = std::fs::symlink_metadata(path)?;
    if !md.is_file() {
        anyhow::bail!("不是一个文件：{path}");
    }

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    md.len().hash(&mut hasher);
    md.modified().ok().hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let dir = format!("{}cache/apk_icon", crate::defs::WORKING_DIR);
    let out = format!("{dir}/{key}.png");
    if let Ok(bytes) = std::fs::read(&out) {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }

    let dex_path = format!("{}/appicon.dex", crate::defs::BINARY_DIR);
    if let Ok(data) = crate::assets::get_asset_data("appicon.dex") {
        let _ = std::fs::create_dir_all(crate::defs::BINARY_DIR);
        let _ = std::fs::write(&dex_path, &data);
    }
    if !std::path::Path::new(&dex_path).exists() {
        anyhow::bail!("缺少 appicon.dex");
    }
    std::fs::create_dir_all(&dir)?;

    // The dumper writes the PNG itself, so there is no base64 to carry back.
    let output = std::process::Command::new("app_process")
        .args([
            &format!("-Djava.class.path={dex_path}"),
            "/system/bin",
            "AppIconDumper",
            path,
            &out,
        ])
        .output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("读取 APK 图标失败：{}", detail.trim());
    }

    let bytes = std::fs::read(&out)?;
    if bytes.is_empty() {
        anyhow::bail!("APK 图标是空的");
    }
    Ok(bytes)
}

#[cfg(not(target_os = "android"))]
fn apk_icon(_path: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("当前平台不支持读取 APK 图标")
}

/// Get app icon as (base64, mime) using app_process + PackageManager.getApplicationIcon()
#[cfg(target_os = "android")]
fn get_app_icon_base64(pkg_name: &str) -> Option<(String, String)> {
    // Check cache first
    {
        let mut cache_guard = APP_ICON_CACHE.lock().unwrap();
        if cache_guard.is_none() {
            *cache_guard = Some(std::collections::HashMap::new());
        }
        if let Some(cache) = cache_guard.as_ref() {
            if let Some(cached) = cache.get(pkg_name) {
                return Some(cached.clone());
            }
        }
    }

    // Ensure appicon.dex is extracted
    let dex_path = format!("{}/appicon.dex", crate::defs::BINARY_DIR);
    if let Ok(data) = crate::assets::get_asset_data("appicon.dex") {
        let _ = std::fs::create_dir_all(crate::defs::BINARY_DIR);
        let _ = std::fs::write(&dex_path, &data);
    }
    if !std::path::Path::new(&dex_path).exists() {
        return None;
    }

    // Run AppIconDumper via app_process
    let classpath = format!("-Djava.class.path={}", dex_path);
    let output_dir = format!("{}/app_icons", crate::defs::WORKING_DIR);
    let _ = std::fs::create_dir_all(&output_dir);
    let output_file = format!("{}/{}.png", output_dir, pkg_name.replace('/', "_"));

    let output = match std::process::Command::new("app_process")
        .args([
            &classpath,
            "/system/bin",
            "AppIconDumper",
            pkg_name,
            &output_file,
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut base64_data: Option<String> = None;

    for line in stdout.lines() {
        if let Some(b64) = line.strip_prefix("BASE64:") {
            base64_data = Some(b64.to_string());
            break;
        }
    }

    let b64 = base64_data?;
    if b64.is_empty() {
        return None;
    }

    let result = (b64, "image/png".to_string());

    // Store in cache
    {
        let mut cache_guard = APP_ICON_CACHE.lock().unwrap();
        if let Some(cache) = cache_guard.as_mut() {
            cache.insert(pkg_name.to_string(), result.clone());
        }
    }

    Some(result)
}

#[cfg(not(target_os = "android"))]
fn get_app_icon_base64(_pkg_name: &str) -> Option<(String, String)> {
    None
}

/// What the page shows while the device goes down.
#[cfg(target_os = "android")]
fn reboot_label(mode: &str) -> String {
    match mode {
        "soft_reboot" => "正在软重启",
        "userspace" => "正在重启用户空间",
        "recovery" => "正在重启到 Recovery",
        "bootloader" => "正在重启到 Bootloader",
        "download" => "正在重启到 Download",
        "edl" => "正在重启到 EDL",
        "shutdown" => "正在关机",
        _ => "正在重启",
    }
    .to_string()
}

/// Perform what the page asked for, using the same commands the Manager app does.
///
/// Two of these are not `reboot` at all:
/// - `soft_reboot` is KernelSU's emulated one (`stop` → post-fs-data → `start` → services),
///   which is also what keeps the jailbreak across the restart. It runs as its own
///   `ksud soft-reboot` process because that path daemonizes — doing it here would take the
///   web UI's own server down before it could answer.
/// - `userspace` restarts the user space only; the kernel keeps running.
#[cfg(target_os = "android")]
fn reboot_device(mode: &str) {
    use std::process::{Command, Stdio};

    if mode == "soft_reboot" {
        let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ksud"));
        match Command::new(exe)
            .arg("soft-reboot")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => log::info!("soft reboot requested from the web UI"),
            Err(e) => log::warn!("soft reboot could not start: {e:#}"),
        }
        return;
    }

    // Some ROMs answer a recovery reboot with a "factory data reset" prompt; a power key press
    // dismisses it. The Manager does this before its recovery reboot too.
    if mode == "recovery" {
        let _ = Command::new("/system/bin/input")
            .args(["keyevent", "26"])
            .status();
    }

    let reason = match mode {
        "userspace" | "recovery" | "bootloader" | "download" | "edl" => format!(" {mode}"),
        _ => String::new(),
    };
    // The pair the Manager uses: `svc power reboot` is the documented path, and toybox's
    // `reboot` covers the ROMs where `svc` is missing or refuses.
    let cmd = if mode == "shutdown" {
        "/system/bin/svc power shutdown || /system/bin/reboot -p".to_string()
    } else {
        format!("/system/bin/svc power reboot{reason} || /system/bin/reboot{reason}")
    };
    match Command::new("/system/bin/sh").arg("-c").arg(&cmd).status() {
        Ok(_) => log::info!("reboot requested from the web UI: {cmd}"),
        Err(e) => log::warn!("reboot command failed ({cmd}): {e:#}"),
    }
}

/// Whether the platform supports a userspace reboot.
///
/// The Manager asks `PowerManager.isRebootingUserspaceSupported()`; from here the same answer
/// comes out of the properties behind it. With neither set the option stays hidden, so a ROM
/// without the feature never gets a button that would quietly do a full reboot instead.
#[cfg(target_os = "android")]
fn userspace_reboot_supported() -> bool {
    [
        "ro.reboot.userspace_supported",
        "ro.init.userspace_reboot_supported",
    ]
    .iter()
    .any(|key| {
        std::process::Command::new("getprop")
            .arg(key)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "true")
            .unwrap_or(false)
    })
}

fn get_system_info() -> Result<HashMap<String, serde_json::Value>> {
    let mut info = HashMap::new();

    #[cfg(target_os = "android")]
    {
        // Kernel version
        if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
            let kernel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.insert(
                "kernelVersion".to_string(),
                serde_json::Value::String(kernel),
            );
        }

        // Device model - try brand-specific market names first
        let manufacturer = if let Ok(output) = std::process::Command::new("getprop")
            .arg("ro.product.manufacturer")
            .output()
        {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase()
        } else {
            String::new()
        };
        let market_props = match manufacturer.as_str() {
            m if m.contains("samsung") => vec!["ro.product.marketname"],
            m if m.contains("xiaomi") || m.contains("redmi") || m.contains("poco") => {
                vec!["ro.product.marketname"]
            }
            m if m.contains("oppo")
                || m.contains("oneplus")
                || m.contains("realme")
                || m.contains("oplus") =>
            {
                vec!["ro.vendor.oplus.market.name", "ro.product.marketname"]
            }
            m if m.contains("vivo") || m.contains("iqoo") => vec!["ro.vivo.market.name"],
            m if m.contains("honor") || m.contains("huawei") => vec!["ro.config.marketing_name"],
            _ => vec!["ro.product.marketname"],
        };
        let mut device_model = String::new();
        for prop in &market_props {
            if let Ok(output) = std::process::Command::new("getprop").arg(*prop).output() {
                let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !val.is_empty() && val != "unknown" {
                    device_model = val;
                    break;
                }
            }
        }
        if device_model.is_empty() {
            if let Ok(output) = std::process::Command::new("getprop")
                .arg("ro.product.model")
                .output()
            {
                device_model = String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
        info.insert(
            "deviceModel".to_string(),
            serde_json::Value::String(device_model),
        );

        // System fingerprint
        if let Ok(output) = std::process::Command::new("getprop")
            .arg("ro.build.fingerprint")
            .output()
        {
            let fp = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.insert("fingerprint".to_string(), serde_json::Value::String(fp));
        }

        // SELinux status - translate to Chinese
        if let Ok(output) = std::process::Command::new("getenforce").output() {
            let selinux_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let selinux = match selinux_raw.as_str() {
                "Enforcing" => "强制执行".to_string(),
                "Permissive" => "宽容".to_string(),
                "Disabled" => "已禁用".to_string(),
                _ => selinux_raw,
            };
            info.insert(
                "selinuxStatus".to_string(),
                serde_json::Value::String(selinux),
            );
        }

        // Seccomp status - check if seccomp is available and enabled
        // APK uses prctl(PR_GET_SECCOMP), we check /proc/sys/kernel/seccomp/actions_avail
        let seccomp = if std::path::Path::new("/proc/sys/kernel/seccomp/actions_avail").exists() {
            if let Ok(content) = std::fs::read_to_string("/proc/sys/kernel/seccomp/actions_avail") {
                if content.trim().is_empty() { "0" } else { "2" }
            } else {
                "2"
            }
        } else {
            "0"
        };
        info.insert(
            "seccompStatus".to_string(),
            serde_json::Value::String(seccomp.to_string()),
        );

        // Tells the reboot dialog whether to offer a userspace reboot at all.
        info.insert(
            "userspaceReboot".to_string(),
            serde_json::Value::Bool(userspace_reboot_supported()),
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        info.insert(
            "kernelVersion".to_string(),
            serde_json::Value::String("unknown".to_string()),
        );
        info.insert(
            "deviceModel".to_string(),
            serde_json::Value::String("unknown".to_string()),
        );
        info.insert(
            "fingerprint".to_string(),
            serde_json::Value::String("unknown".to_string()),
        );
        info.insert(
            "selinuxStatus".to_string(),
            serde_json::Value::String("Disabled".to_string()),
        );
        info.insert(
            "seccompStatus".to_string(),
            serde_json::Value::String("0".to_string()),
        );
    }

    Ok(info)
}

/// One installed package, as the module bridge needs it.
#[cfg(target_os = "android")]
#[derive(Clone)]
struct AppRow {
    package: String,
    uid: i32,
    is_system: bool,
    label: String,
    version_code: i64,
    /// The installed APK, and how big it is: what the app list shows and what gets extracted.
    apk_path: String,
    size: u64,
}

/// `pm list packages` with `args`, preferring the non-deprecated `cmd package` spelling.
///
/// Empty when neither binary answers: callers treat that as "no packages", because a
/// failure here must not take the whole web UI down with it.
#[cfg(target_os = "android")]
fn pm_query(args: &[&str]) -> String {
    let spellings: [(&str, &[&str]); 2] = [
        ("cmd", &["package", "list", "packages"]),
        ("pm", &["list", "packages"]),
    ];
    for (bin, prefix) in spellings {
        let argv: Vec<&str> = prefix.iter().copied().chain(args.iter().copied()).collect();
        if let Ok(output) = std::process::Command::new(bin).args(&argv).output() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.contains("package:") {
                return stdout;
            }
        }
    }
    String::new()
}

/// Pull the package name, APK path, uid and version code out of `pm list packages`
/// output, as `(name, apk_path, uid, version_code)`.
///
/// With `-f` a line is `package:<apk path>=<name>`; without it the path is empty.
#[cfg(target_os = "android")]
fn parse_pm_lines(stdout: &str) -> Vec<(String, String, i32, i64)> {
    fn field(line: &str, key: &str) -> Option<String> {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(key))
            .map(str::to_string)
    }

    let mut rows = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.trim().strip_prefix("package:") else {
            continue;
        };
        let Some(first) = rest.split_whitespace().next() else {
            continue;
        };
        // With `-f` the token is `<apk path>=<package>`; the path can contain '=' itself, so
        // the package name is what follows the last one. An empty path means `-f` was not
        // used, in which case the token is the package name already.
        let (apk_path, name) = match first.rsplit_once('=') {
            Some((path, name)) => (path.to_string(), name.to_string()),
            None => (String::new(), first.to_string()),
        };
        if name.is_empty() {
            continue;
        }
        let uid = field(rest, "uid:")
            .and_then(|v| v.parse().ok())
            .unwrap_or(-1);
        let version_code = field(rest, "versionCode:")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        rows.push((name, apk_path, uid, version_code));
    }
    rows
}

/// Build the package list: names and uids in one pass, system-ness from a second query.
#[cfg(target_os = "android")]
fn collect_apps() -> Vec<AppRow> {
    use std::collections::HashSet;

    let labels = get_app_labels_from_dex();

    let system: HashSet<String> = parse_pm_lines(&pm_query(&["-s", "-U"]))
        .into_iter()
        .map(|(name, _, _, _)| name)
        .collect();

    // Names, APK paths and uids come from one query. The version-code flag is not on every
    // Android version, so where it is not understood the same query runs without it — and a
    // failed query must never leave the list empty.
    let mut listed = parse_pm_lines(&pm_query(&["-f", "-U", "--show-versioncode"]));
    if listed.is_empty() {
        listed = parse_pm_lines(&pm_query(&["-f", "-U"]));
    }

    let mut rows: Vec<AppRow> = listed
        .into_iter()
        .filter(|(name, _, _, _)| !name.is_empty() && name != defs::DEFAULT_PACKAGE_NAME)
        .map(|(package, apk_path, uid, version_code)| {
            // Localised labels come from the dex dump; where that gives nothing, the APK's
            // own manifest is parsed. The package-name tail is the last resort.
            let label = labels
                .get(&package)
                .cloned()
                .or_else(|| label_from_apk_path(&apk_path))
                .unwrap_or_else(|| package.rsplit('.').next().unwrap_or(&package).to_string());
            AppRow {
                is_system: system.contains(&package),
                version_code,
                size: std::fs::metadata(&apk_path).map_or(0, |md| md.len()),
                apk_path,
                package,
                uid,
                label,
            }
        })
        .collect();

    // Authorised apps first, then by pinyin so Chinese names sort the way a reader expects.
    rows.sort_by(|a, b| {
        let a_granted = ksucalls::is_app_granted(a.uid);
        let b_granted = ksucalls::is_app_granted(b.uid);
        b_granted.cmp(&a_granted).then_with(|| {
            let a_pinyin: String = a
                .label
                .as_str()
                .to_pinyin()
                .map(|p| p.map(|py| py.plain()).unwrap_or(""))
                .collect();
            let b_pinyin: String = b
                .label
                .as_str()
                .to_pinyin()
                .map(|p| p.map(|py| py.plain()).unwrap_or(""))
                .collect();
            a_pinyin.to_lowercase().cmp(&b_pinyin.to_lowercase())
        })
    });
    rows
}

/// How long a built package list is reused.
///
/// Building it shells out to `pm` three times and runs `app_process` for the labels. A
/// module page asks for the list several times while it renders, and doing all that work
/// per request is what left those pages sitting on a spinner.
#[cfg(target_os = "android")]
const APPS_CACHE_TTL: Duration = Duration::from_secs(20);

#[cfg(target_os = "android")]
static APPS_CACHE: std::sync::Mutex<Option<(std::time::Instant, std::sync::Arc<Vec<AppRow>>)>> =
    std::sync::Mutex::new(None);

#[cfg(target_os = "android")]
fn apps_cached() -> std::sync::Arc<Vec<AppRow>> {
    let mut guard = APPS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((built_at, rows)) = guard.as_ref()
        && built_at.elapsed() < APPS_CACHE_TTL
    {
        return std::sync::Arc::clone(rows);
    }
    let rows = std::sync::Arc::new(collect_apps());
    *guard = Some((std::time::Instant::now(), std::sync::Arc::clone(&rows)));
    rows
}

/// The installed-app list behind the file manager's 提取APK panel.
///
/// Separate from [`get_apps_list`], which backs the root-grant screen: that one is third-party
/// only and has no place for a version or a size, while this one is what an app-info list and
/// a 提取安装包 action need — every package, with the APK it came from.
fn get_all_apps_list() -> Result<Vec<serde_json::Value>> {
    #[allow(unused_mut)]
    let mut apps = Vec::new();

    #[cfg(target_os = "android")]
    {
        for row in apps_cached().iter() {
            apps.push(serde_json::json!({
                "package": row.package,
                "label": row.label,
                "uid": row.uid,
                "versionCode": row.version_code,
                "size": row.size,
                "apkPath": row.apk_path,
                "isSystem": row.is_system,
            }));
        }
    }

    Ok(apps)
}

fn get_apps_list() -> Result<Vec<HashMap<String, serde_json::Value>>> {
    #[allow(unused_mut)]
    let mut apps = Vec::new();

    #[cfg(target_os = "android")]
    {
        for row in apps_cached().iter() {
            // This list backs the web UI's root-grant screen, which has no place for the
            // hundreds of system packages; the bridge endpoints serve those separately.
            if row.is_system {
                continue;
            }
            let mut app = HashMap::new();
            app.insert("uid".to_string(), serde_json::Value::Number(row.uid.into()));
            app.insert(
                "packageName".to_string(),
                serde_json::Value::String(row.package.clone()),
            );
            app.insert(
                "appName".to_string(),
                serde_json::Value::String(row.label.clone()),
            );
            app.insert(
                "granted".to_string(),
                serde_json::Value::Bool(ksucalls::is_app_granted(row.uid)),
            );
            app.insert(
                "version".to_string(),
                serde_json::Value::String(String::new()),
            );
            apps.push(app);
        }
    }

    Ok(apps)
}

/// Package names for `ksu.listPackages(type)`.
///
/// The APK returns a JSON array of bare package names, filtered by `system` / `user`.
#[cfg(target_os = "android")]
fn bridge_package_names(kind: &str) -> Vec<String> {
    apps_cached()
        .iter()
        .filter(|row| match kind.to_lowercase().as_str() {
            "system" => row.is_system,
            "user" => !row.is_system,
            _ => true,
        })
        .map(|row| row.package.clone())
        .collect()
}

/// Per-package details for `ksu.getPackagesInfo(names)`.
///
/// Keys follow the APK's `getPackagesInfo`. `versionName` is the one field with no cheap
/// source here — it comes from `dumpsys` or the APK manifest, both far too slow per
/// package — so it is empty rather than invented.
#[cfg(target_os = "android")]
fn bridge_packages_info(names: &[String]) -> Vec<serde_json::Value> {
    let apps = apps_cached();
    names
        .iter()
        .map(|name| match apps.iter().find(|row| &row.package == name) {
            Some(row) => serde_json::json!({
                "packageName": row.package,
                "versionName": "",
                "versionCode": row.version_code,
                "appLabel": row.label,
                "isSystem": row.is_system,
                "uid": row.uid,
            }),
            None => serde_json::json!({
                "packageName": name,
                "error": "Package not found or inaccessible",
            }),
        })
        .collect()
}

fn handle_request(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let path = req.path.split('?').next().unwrap_or("/");

    // Nothing is served without the token — not the API, not even the page itself. A URL
    // that leaks is therefore inert on its own: reaching it requires the cookie the server
    // hands out on a token-bearing visit. The launcher supplies the token; everything the
    // page then loads is same-origin and carries the cookie.
        // Worth knowing which source was tried: a stale cookie and a stale `?k=` address
        // look identical from the outside.
        let source = if token.is_empty() {
            "no token"
        } else if req.path.contains("?k=") {
            "address token"
        } else {
            "cookie token"
        };
        log::warn!(
            "Rejected {} {path}: {source} does not match ({})",
            req.method,
        );
    }

    // Handle CORS preflight
    if req.method == "OPTIONS" {
        return send_response(stream, 204, "text/plain", &[]);
    }

    match (req.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => {
            // Opening the page with ?k=<token> upgrades the browser to a cookie, so the UI
            // and anything it embeds (module WebUIs, their bridge calls) just work.
            // no-store matters: without it a browser will happily keep serving a cached copy
            // of this page after the APK (and therefore the embedded HTML) has been updated,
            // which makes a new build look like it did not install.
            let mut extra =
                String::from("Cache-Control: no-store, must-revalidate\r\nPragma: no-cache\r\n");
                extra.push_str(&format!(
                ));
            }
            send_response_with(
                stream,
                200,
                "text/html; charset=utf-8",
                INDEX_HTML.as_bytes(),
                &extra,
            )
        }

        // Serve a module's bundled WebUI from its webroot/ directory
        ("GET", p) if p.starts_with("/modweb/") => serve_module_web(stream, p),

        // Serve module assets with absolute paths (/assets/..., /internal/...) via Referer
        ("GET", p) if p.starts_with("/assets/") || p.starts_with("/internal/") => {
            serve_module_asset_by_referer(stream, p, req)
        }

        // Liveness ping from the frontend. Served purely so the page has something to
        // hit; the activity stamp itself is written by handle_client for every request.
        ("GET", "/api/heartbeat") | ("POST", "/api/heartbeat") => {
            json_response(stream, &ApiResponse::ok("alive"))
        }

        // 启动器的守护连接：连着就一直服务，断开就退出（后台被划掉时就是这样）。

        // ---- terminal -----------------------------------------------------------
        // A persistent `sh` with piped stdio: writing to stdin runs a command, and a
        // script's own `read` receives whatever the user types. There is no pty, so
        // full-screen programs will not work.
        ("POST", "/api/shell/open") => rooted(stream, req, |body: ShellOpen| {
            crate::webui_shell::open(&body.cwd)
                .map(|(id, pty)| serde_json::json!({ "id": id, "pty": pty }))
        }),

        ("POST", "/api/shell/input") => fs_json(
            stream,
            json_body::<ShellInput>(req)
                .and_then(|body| crate::webui_shell::write(body.id, &body.data))
                .map(|()| "ok"),
        ),

        ("GET", "/api/shell/output") => match require_param(req, "id")
            .and_then(|v| v.parse::<u32>().with_context(|| "id 不是数字"))
        {
            Ok(id) => fs_json(
                stream,
                crate::webui_shell::read(id)
                    .map(|(data, alive)| serde_json::json!({ "data": data, "alive": alive })),
            ),
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        ("POST", "/api/shell/close") => fs_json(
            stream,
            json_body::<ShellClose>(req)
                .and_then(|body| crate::webui_shell::close(body.id))
                .map(|()| "ok"),
        ),

        // ---- file manager -------------------------------------------------------
        // Reads take the path as a query parameter; writes take a JSON body. Every one of
        // these inherits root from ksud, so `require_root` is checked once per request to
        // fail loudly instead of half-working.
        ("GET", "/api/fs/roots") => {
            json_response(stream, &ApiResponse::ok(crate::webui_files::root_info()))
        }

        ("GET", "/api/fs/list") => match require_param(req, "path") {
            Ok(path) => fs_json(stream, crate::webui_files::list_dir(&path)),
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        ("GET", "/api/fs/stat") => match require_param(req, "path") {
            Ok(path) => fs_json(stream, crate::webui_files::stat(&path)),
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        // 搜索：在当前目录下按名字找文件/文件夹。深度和结果数在这里再收一次口 —— 页面可以要得
        // 很贪，但走的是手机的存储，0 表示"用默认"。
        ("POST", "/api/fs/search") => fs_json(
            stream,
            json_body::<FsSearch>(req).and_then(|body| {
                let depth = if body.depth == 0 { 6 } else { body.depth.min(12) };
                let limit = if body.limit == 0 { 300 } else { body.limit.min(1000) };
                crate::webui_files::search(&body.path, &body.query, depth, limit)
            }),
        ),

        ("GET", "/api/fs/read") => match require_param(req, "path") {
            Ok(path) => fs_json(
                stream,
                crate::webui_files::read_text(&path, crate::webui_files::MAX_TEXT_BYTES),
            ),
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        ("GET", "/api/fs/raw") => serve_file_bytes(stream, req),

        // 一个 APK 文件自己的图标（没安装也能读）。
        ("GET", "/api/fs/apkicon") => match require_param(req, "path") {
            Ok(path) => match apk_icon(&path) {
                Ok(bytes) => send_response_with(stream, 200, "image/png", &bytes, ""),
                Err(e) => error_response(stream, 404, &format!("{e:#}")),
            },
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        // 安装包自己的包名：从 AndroidManifest.xml 里读，装之前就能报出来。
        ("GET", "/api/fs/apkinfo") => fs_json(stream, apk_info(req)),

        ("POST", "/api/fs/mkdir") => rooted_ok(stream, req, |body: FsMkdir| {
            crate::webui_files::create_dir(&body.path)
        }),

        ("POST", "/api/fs/rename") => rooted_ok(stream, req, |body: FsRename| {
            crate::webui_files::rename(&body.from, &body.to)
        }),

        ("POST", "/api/fs/copy") => rooted(stream, req, |body: FsTransfer| {
            crate::webui_files::copy(&body.paths, &body.to, body.overwrite)
        }),

        ("POST", "/api/fs/move") => rooted(stream, req, |body: FsTransfer| {
            crate::webui_files::move_paths(&body.paths, &body.to, body.overwrite)
        }),

        ("POST", "/api/fs/delete") => rooted_ok(stream, req, |body: FsDelete| {
            crate::webui_files::remove(&body.paths, body.recursive)
        }),

        ("POST", "/api/fs/chmod") => rooted_ok(stream, req, |body: FsMode| {
            let mode = crate::webui_files::parse_mode(&body.mode)?;
            crate::webui_files::chmod(&body.paths, mode, body.recursive)
        }),

        ("POST", "/api/fs/chown") => rooted_ok(stream, req, |body: FsOwner| {
            crate::webui_files::chown(&body.paths, body.uid, body.gid, body.recursive)
        }),

        ("POST", "/api/fs/write") => rooted_ok(stream, req, |body: FsWrite| {
            crate::webui_files::write_text(&body.path, &body.content)
        }),

        ("GET", "/api/fs/archive") => match (require_param(req, "path"), query_param(req, "sub")) {
            (Ok(path), sub) => fs_json(
                stream,
                crate::webui_files::list_archive(&path, &sub.unwrap_or_default()),
            ),
            (Err(e), _) => error_response(stream, 400, &format!("{e:#}")),
        },

        // One member of an archive, for the previews the file manager shows: an image or a
        // video inside a zip has no path on the device to serve it from.
        ("GET", "/api/fs/archive/raw") => {
            match (require_param(req, "archive"), require_param(req, "entry")) {
                (Ok(archive), Ok(entry)) => match crate::webui_files::read_archive_entry(
                    &archive,
                    &entry,
                    crate::webui_files::MAX_RAW_BYTES,
                ) {
                    Ok(data) => send_response(stream, 200, mime_for_name(&entry), &data),
                    Err(e) => error_response(stream, 404, &format!("{e:#}")),
                },
                (Err(e), _) | (_, Err(e)) => error_response(stream, 400, &format!("{e:#}")),
            }
        }

        ("GET", "/api/fs/archive/read") => {
            match (require_param(req, "archive"), require_param(req, "entry")) {
                (Ok(archive), Ok(entry)) => fs_json(
                    stream,
                    crate::webui_files::read_archive_text(
                        &archive,
                        &entry,
                        crate::webui_files::MAX_TEXT_BYTES,
                    ),
                ),
                (Err(e), _) | (_, Err(e)) => error_response(stream, 400, &format!("{e:#}")),
            }
        }

        ("POST", "/api/fs/archive/write") => rooted_ok(stream, req, |body: FsArchiveWrite| {
            crate::webui_files::write_archive_entry(&body.archive, &body.entry, &body.content)
        }),

        ("POST", "/api/fs/archive/delete") => rooted(stream, req, |body: FsArchiveDelete| {
            crate::webui_files::delete_from_archive(&body.archive, &body.entries)
        }),

        ("POST", "/api/fs/extract") => rooted(stream, req, |body: FsExtract| {
            crate::webui_files::extract_archive(&body.archive, &body.dest, &body.entries)
        }),

        // The other direction of the same gesture: what is selected on one side goes into the
        // archive open on the other.
        ("POST", "/api/fs/archive/add") => rooted(stream, req, |body: FsArchiveAdd| {
            crate::webui_files::add_to_archive(&body.archive, &body.sources, &body.sub)
        }),

        ("POST", "/api/fs/archive/create") => rooted(stream, req, |body: FsArchiveAdd| {
            crate::webui_files::create_archive(&body.archive, &body.sources, &body.sub)
        }),


        ("GET", "/api/fs/jumps") => {
            json_response(stream, &ApiResponse::ok(crate::webui_files::load_jumps()))
        }

        ("POST", "/api/fs/jumps") => rooted_ok(stream, req, |body: JumpsBody| {
            crate::webui_files::save_jumps(&body.jumps)
        }),

        ("GET", "/api/fs/quickrun") => {
            json_response(stream, &ApiResponse::ok(crate::webui_files::load_quick_run()))
        }

        ("POST", "/api/fs/quickrun") => rooted_ok(stream, req, |body: QuickRunBody| {
            crate::webui_files::save_quick_run(&body.quick_run)
        }),

        ("GET", "/api/ui/theme") => {
            json_response(stream, &ApiResponse::ok(crate::webui_files::load_theme()))
        }

        ("POST", "/api/ui/theme") => rooted_ok(stream, req, |body: crate::webui_files::Theme| {
            crate::webui_files::save_theme(&body)
        }),

        ("GET", "/api/module/info") => {
            #[cfg(target_os = "android")]
            {
                match module_info_json(require_param(req, "id").unwrap_or_default().as_str()) {
                    Ok(info) => json_response(stream, &ApiResponse::ok(info)),
                    Err(e) => error_response(stream, 500, &format!("{e:#}")),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        // `ksu.listPackages(type)`: bare package names, as the APK returns them.
        ("GET", "/api/module/packages") => {
            let kind = query_param(req, "type").unwrap_or_default();
            json_response(stream, &ApiResponse::ok(bridge_package_names(&kind)))
        }

        // `ksu.getPackagesInfo(names)`: the APK takes a JSON array of names, so this takes
        // them comma separated (already percent-decoded by the query parser).
        ("GET", "/api/module/packagesinfo") => {
            let names: Vec<String> = query_param(req, "names")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            json_response(stream, &ApiResponse::ok(bridge_packages_info(&names)))
        }

        ("POST", "/api/fs/install") => rooted(stream, req, |body: FsInstall| {
            // An installer inside an archive has no path of its own on the device, so it is
            // staged out of the archive and installed from there.
            if body.archive.is_empty() {
                crate::webui_files::install_apk(&body.path)
            } else {
                crate::webui_files::install_apk_from_archive(&body.archive, &body.entry)
            }
        }),

        ("POST", "/api/fs/upload") => {
            let target = require_param(req, "path")
                .and_then(|dir| require_param(req, "name").and_then(|name| safe_join(&dir, &name)));
            match target {
                Ok(path) => fs_json(
                    stream,
                    crate::webui_files::require_root()
                        .and_then(|()| crate::webui_files::write_bytes(&path, &req.body))
                        .map(|()| path),
                ),
                Err(e) => error_response(stream, 400, &format!("{e:#}")),
            }
        }

        ("GET", "/api/info") => match get_kernel_info() {
            Ok(info) => json_response(stream, &ApiResponse::ok(info)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },
        ("GET", "/api/systeminfo") => match get_system_info() {
            Ok(info) => json_response(stream, &ApiResponse::ok(info)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },

        ("GET", "/api/modules") => match get_modules_list() {
            Ok(modules) => json_response(stream, &ApiResponse::ok(modules)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },

        // Module webui bridge APIs (for ksu JS object)
        ("POST", "/api/module/exec") => handle_module_exec(stream, req),
        ("POST", "/api/module/spawn") => handle_module_spawn(stream, req),
        ("GET", "/api/module/spawn/output") => handle_module_spawn_output(stream, req),
        ("POST", "/api/module/spawn/close") => handle_module_spawn_close(stream, req),
        ("GET", p) if p.starts_with("/api/module/prop") => handle_module_prop_get(stream, p),
        ("POST", "/api/module/prop") => handle_module_prop_set(stream, req),

        ("POST", "/api/modules/install") => {
            #[cfg(target_os = "android")]
            {
                if req.body.is_empty() {
                    return error_response(stream, 400, "没有收到文件内容，请重新选择模块 zip");
                }
                // Timed per stage: "installing is slow" is not actionable until it is known
                // whether the upload, the write or the install itself is the cost.
                let started = std::time::Instant::now();
                if let Err(e) = std::fs::create_dir_all(defs::WORKING_DIR) {
                    return error_response(
                        stream,
                        500,
                        &format!("创建 {} 失败：{e}", defs::WORKING_DIR),
                    );
                }
                // Named per request rather than a fixed path: two installs overlapping (or a
                // leftover file from an interrupted one) must not write over each other.
                let tmp_path = std::path::Path::new(defs::WORKING_DIR).join(format!(
                    ".tmp_module_{}_{}.zip",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                ));
                let tmp = tmp_path.display().to_string();
                if let Err(e) = std::fs::write(&tmp_path, &req.body) {
                    return error_response(stream, 500, &format!("Failed to save file: {e}"));
                }
                let wrote = started.elapsed().as_millis();
                let installing = std::time::Instant::now();
                let result = match module::install_module(&tmp) {
                    Ok(()) => {
                        let installed = installing.elapsed().as_millis();
                        json_response(
                            stream,
                            &ApiResponse::ok(format!(
                                "安装完成 · 上传 {}KB / 写盘 {wrote}ms / 安装 {installed}ms",
                                req.body.len() / 1024
                            )),
                        )
                    }
                    Err(e) => error_response(stream, 500, &e.to_string()),
                };
                // The zip has served its purpose either way; leaving it behind would let a
                // failed install waste tens of MB in the working directory.
                let _ = std::fs::remove_file(&tmp_path);
                result
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        // Install a zip that is already on the device.
        //
        // This is the flow the UI prefers, because it needs no upload and — the point — no
        // system file picker. On Android the picker backgrounds the browser, and a browser
        // killed while it is away comes back to a page whose server has already exited,
        // which is what a "blank page after tapping +" actually is.
        ("POST", "/api/modules/install-path") => {
            #[cfg(target_os = "android")]
            {
                match json_body::<ModulePath>(req) {
                    Ok(body) => install_from_path(stream, &body.path),
                    Err(e) => error_response(stream, 400, &format!("{e:#}")),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", p) if p.starts_with("/api/modules/") && p.ends_with("/enable") => {
            #[cfg(target_os = "android")]
            {
                let id = p
                    .trim_start_matches("/api/modules/")
                    .trim_end_matches("/enable");
                match module::enable_module(id) {
                    Ok(()) => json_response(stream, &ApiResponse::ok("Enabled")),
                    Err(e) => error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", p) if p.starts_with("/api/modules/") && p.ends_with("/disable") => {
            #[cfg(target_os = "android")]
            {
                let id = p
                    .trim_start_matches("/api/modules/")
                    .trim_end_matches("/disable");
                match module::disable_module(id) {
                    Ok(()) => json_response(stream, &ApiResponse::ok("Disabled")),
                    Err(e) => error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", p) if p.starts_with("/api/modules/") && p.ends_with("/action") => {
            #[cfg(target_os = "android")]
            {
                let id = p
                    .trim_start_matches("/api/modules/")
                    .trim_end_matches("/action");
                match module::run_action(id) {
                    Ok(()) => json_response(stream, &ApiResponse::ok("Action executed")),
                    Err(e) => error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", p) if p.starts_with("/api/modules/") && p.ends_with("/undo") => {
            #[cfg(target_os = "android")]
            {
                let id = p
                    .trim_start_matches("/api/modules/")
                    .trim_end_matches("/undo");
                // Undo uninstall: remove the "remove" file from both modules/ and modules_update/
                let mut removed = false;
                for dir in [defs::MODULE_DIR, defs::MODULE_UPDATE_DIR] {
                    let remove_file = std::path::Path::new(dir)
                        .join(id)
                        .join(defs::REMOVE_FILE_NAME);
                    if remove_file.exists() {
                        if std::fs::remove_file(&remove_file).is_ok() {
                            removed = true;
                        }
                    }
                }
                if removed {
                    json_response(stream, &ApiResponse::ok("Undo successful"))
                } else {
                    error_response(stream, 404, "Module not marked for removal")
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("DELETE", p) if p.starts_with("/api/modules/") => {
            #[cfg(target_os = "android")]
            {
                let id = p.trim_start_matches("/api/modules/");
                match module::uninstall_module(id) {
                    Ok(()) => json_response(stream, &ApiResponse::ok("Uninstalled")),
                    Err(e) => error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("GET", "/api/apps/icon") => {
            #[cfg(target_os = "android")]
            {
                // Parse package from query string (?package=xxx)
                let pkg = if let Some(query_start) = req.path.find('?') {
                    let query = &req.path[query_start + 1..];
                    let mut result = String::new();
                    for pair in query.split('&') {
                        if let Some((key, value)) = pair.split_once('=') {
                            if key == "package" {
                                result = value.to_string();
                                break;
                            }
                        }
                    }
                    result
                } else {
                    String::new()
                };
                if pkg.is_empty() {
                    return error_response(stream, 400, "Missing package parameter");
                }
                match get_app_icon_base64(&pkg) {
                    Some((icon, mime)) => {
                        let json = serde_json::json!({"success": true, "icon": icon, "mime": mime});
                        json_response(stream, &json)
                    }
                    None => {
                        let json = serde_json::json!({"success": false, "error": "Icon not found"});
                        json_response(stream, &json)
                    }
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        // What an app-info screen shows: the package's own fields, the schemes its APK is
        // signed with, whether a packer is visible, and where its files live.
        // ---- Oh My Keymint panel ------------------------------------------------
        // The module has no WebUI of its own; this one edits the files its documentation
        // names. Reads do not need root, writes do — and every write backs the file up first.
        ("GET", "/api/keymint/status") => match crate::webui_keymint::status() {
            Ok(status) => json_response(stream, &ApiResponse::ok(status)),
            Err(e) => error_response(stream, 500, &format!("{e:#}")),
        },

        ("POST", "/api/keymint/scoop") => rooted(stream, req, |body: KeymintScoop| {
            crate::webui_keymint::save_scoop(&body.packages)
        }),

        ("POST", "/api/keymint/fix-props") => rooted(stream, req, |body: KeymintFixProps| {
            crate::webui_keymint::set_fix_props(body.enable)
        }),

        ("POST", "/api/keymint/log-level") => rooted(stream, req, |body: KeymintLogLevel| {
            let which = crate::webui_keymint::Which::parse(&body.which)?;
            crate::webui_keymint::save_log_level(which, &body.level)
        }),

        ("POST", "/api/keymint/keybox") => rooted(stream, req, |body: KeymintKeybox| {
            crate::webui_keymint::apply_keybox(&body.path)
        }),

        ("POST", "/api/keymint/keybox-content") => {
            rooted(stream, req, |body: KeymintKeyboxContent| {
                crate::webui_keymint::apply_keybox_content(&body.content)
            })
        }

        ("POST", "/api/keymint/restart") => rooted(stream, req, |body: KeymintRestart| {
            crate::webui_keymint::restart(&body.what)
        }),

        ("GET", "/api/apps/all") => match get_all_apps_list() {
            Ok(apps) => json_response(stream, &ApiResponse::ok(apps)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },

        ("GET", "/api/apps/detail") => match require_param(req, "package") {
            Ok(package) => match crate::webui_apps::package_detail(&package) {
                Ok(detail) => json_response(stream, &ApiResponse::ok(detail)),
                Err(e) => error_response(stream, 500, &format!("{e:#}")),
            },
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

        // 提取安装包: the app's APKs are copied out, base and splits alike.
        ("POST", "/api/apps/extract") => rooted(stream, req, |body: AppsExtract| {
            crate::webui_apps::extract_package(&body.package, &body.dest, &body.label)
        }),

        ("GET", "/api/apps") => match get_apps_list() {
            Ok(apps) => json_response(stream, &ApiResponse::ok(apps)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },

        ("GET", "/api/features") => match get_features() {
            Ok(features) => json_response(stream, &ApiResponse::ok(features)),
            Err(e) => error_response(stream, 500, &e.to_string()),
        },

        ("POST", "/api/features") => {
            #[cfg(target_os = "android")]
            {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
                let id = body.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let value = body.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
                if id == 999 {
                    // Default umount modules is an AppProfile setting
                    return match ksucalls::set_default_umount_modules(value != 0) {
                        Ok(()) => json_response(stream, &ApiResponse::ok("Updated")),
                        Err(e) => error_response(stream, 500, &e.to_string()),
                    };
                }
                match ksucalls::set_feature(id, value) {
                    Ok(()) => {
                        // 持久化保存 feature 状态到配置文件，重启后不丢失
                        if let Err(e) = crate::feature::save_config() {
                            log::warn!("Failed to save feature config: {e}");
                        }
                        json_response(stream, &ApiResponse::ok("Updated"))
                    }
                    Err(e) => error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", "/api/reboot") => {
            #[cfg(target_os = "android")]
            {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
                let mode = body
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system");

                // Answered before anything else: the work below either replaces init or ends
                // this very process, so the page has to be told first or it waits forever.
                json_response(stream, &ApiResponse::ok(reboot_label(mode)))?;
                reboot_device(mode);
                Ok(())
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", "/api/apps/grant") => {
            #[cfg(target_os = "android")]
            {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
                let uid = body.get("uid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let package_name = body
                    .get("packageName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if uid <= 0 || package_name.is_empty() {
                    return error_response(stream, 400, "Invalid uid or packageName");
                }
                // Use ksucalls to grant root permission via kernel AppProfile
                match crate::ksucalls::grant_app(uid, package_name) {
                    Ok(_) => json_response(stream, &ApiResponse::ok("Granted successfully.")),
                    Err(e) => return error_response(stream, 500, &format!("Failed to grant: {e}")),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", "/api/apps/revoke") => {
            #[cfg(target_os = "android")]
            {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
                let uid = body.get("uid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                if uid <= 0 {
                    return error_response(stream, 400, "Invalid uid");
                }
                // Use ksucalls to revoke root permission via kernel AppProfile
                match crate::ksucalls::revoke_app(uid) {
                    Ok(_) => json_response(stream, &ApiResponse::ok("Revoked successfully.")),
                    Err(e) => {
                        return error_response(stream, 500, &format!("Failed to revoke: {e}"));
                    }
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("POST", "/api/tools/hma-config") => {
            #[cfg(target_os = "android")]
            {
                match run_hma_config() {
                    Ok(output) => json_response(stream, &ApiResponse::ok(output)),
                    Err(e) => return error_response(stream, 500, &e.to_string()),
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        _ => error_response(stream, 404, "Not found"),
    }
}

#[cfg(target_os = "android")]
fn run_hma_config() -> Result<String> {
    use std::io::Write;
    use std::process::Command;

    let script = r#"#!/system/bin/sh
PKG=org.frknkrc44.hma_oss
OUTPUT_FILE="/data/user/0/$PKG/files/config.json"

# Without this the mkdir below would build an app data directory for an app that is not installed.
if ! pm list packages | grep -q "^package:$PKG$"; then
    echo "未安装 HMA（$PKG）"
    exit 1
fi

# Stop HMA before writing. It keeps a copy of the config in memory and writes that copy back when
# it leaves the foreground, so an HMA that is open while this runs would replace what is written
# here with what it already had. Stopped, the file is the only copy, and the app reads it back the
# next time it starts.
am force-stop "$PKG"

GLOBAL_HEADER='{
  "configVersion": 93,
  "detailLog": false,
  "errorOnlyLog": false,
  "maxLogSize": 512,
  "forceMountData": true,
  "disableActivityLaunchProtection": false,
  "altAppDataIsolation": false,
  "altVoldAppDataIsolation": false,
  "skipSystemAppDataIsolation": true,
  "packageQueryWorkaround": false,
  "templates": {},
  "settingsTemplates": {},
  "scope": {'
APP_RULE_TEMPLATE='  "%s": {
    "useWhitelist": false,
    "excludeSystemApps": true,
    "hideInstallationSource": false,
    "hideSystemInstallationSource": false,
    "excludeTargetInstallationSource": false,
    "invertActivityLaunchProtection": false,
    "excludeVoldIsolation": false,
    "restrictedZygotePermissions": [],
    "applyTemplates": [],
    "applyPresets": ["detector_apps","root_apps","shizuku_dhizuku","sus_apps","xposed"],
    "applySettingTemplates": [],
    "applySettingsPresets": ["accessibility","dev_options"],
    "extraAppList": [%s],
    "extraOppositeAppList": []
  }'
EXCLUDED_PACKAGES="eu.darken.sdmse me.weishu.kernelsu bin.mt.plus.canary bin.mt.plus org.telegram.messenger org.telegram.group me.bmax.apatch"

# 网页端启动器的包名每次安装都会重新生成，所以按形状认它：每个变体都是
    # 启动器自己不需要隐藏别的东西，所以不进隐藏范围；反过来要把它从别的应用里隐藏掉。
else
fi

EXCLUDE_REGEX=$(echo "$EXCLUDED_PACKAGES" | sed 's/ /|/g')
ALL_USER_PACKAGES=$(pm list packages -3 | sed 's/^package://' | grep -v -E "$EXCLUDE_REGEX")
FIRST_ENTRY=1
COUNT=0
SCOPE_CONTENT=""
for PKG_NAME in $ALL_USER_PACKAGES; do
    COUNT=$((COUNT + 1))
    if [ $FIRST_ENTRY -eq 1 ]; then
        FIRST_ENTRY=0
    else
        SCOPE_CONTENT="$SCOPE_CONTENT,
    fi
done
FULL_CONFIG="$GLOBAL_HEADER
$SCOPE_CONTENT
}}"
mkdir -p "$(dirname "$OUTPUT_FILE")"
printf "%s" "$FULL_CONFIG" > "$OUTPUT_FILE"
am start --user 0 -n $PKG/.MainActivityLauncher > /dev/null 2>&1
sleep 1
input keyevent 4
sleep 1

# Read the file back and count. This is the step that turns a silent failure into a message: the
# config used to be written into a /data/data the app never looks at — the one inside the sandbox
# mount namespace ksud was started in — and the endpoint still answered success. HMA rewrites the
# file as a single line of its own when it saves, so entries are counted by occurrence, not by line.
ON_DISK=$(grep -o '"useWhitelist"' "$OUTPUT_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$ON_DISK" != "$COUNT" ]; then
    echo "配置没写进 HMA：应写入 $COUNT 条，$OUTPUT_FILE 里只有 $ON_DISK 条"
    exit 1
fi

    # 启动器应该在每个应用的额外隐藏里各出现一次。多出来就说明它又回到隐藏范围里了。
    if [ "$LAUNCHER_MENTIONS" != "$COUNT" ]; then
        echo "额外隐藏启动器没写全：$COUNT 个应用里只出现 $LAUNCHER_MENTIONS 次（应各一次）"
        exit 1
    fi
    exit 0
fi

echo "已写入 $COUNT 个应用（没找到网页端启动器，未做额外隐藏）"
exit 0
"#;

    let script_path = "/data/adb/ksu/hma_config.sh";
    let mut file = std::fs::File::create(script_path)?;
    file.write_all(script.as_bytes())?;
    drop(file);

    let output = Command::new("sh")
        .arg(script_path)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to execute script: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let result = format!("{stdout}\n{stderr}").trim().to_string();

    /* INFO: The script checks its own result and exits non-zero when the config did not land.
       Reporting that as an error instead of returning it as output is the whole point: a config
       that never reached HMA must not come back to the page as "配置完成". */
    if !output.status.success() {
        return Err(anyhow::anyhow!("{result}"));
    }

    Ok(result)
}

/// Epoch milliseconds of the most recent request. Drives the idle shutdown.
static LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

/// Whether the launcher's guard connection is expected, has arrived, or has gone.
///
/// `--watch` hands the server's lifetime to the launcher app: it holds one connection open for
/// as long as its background service lives, and the server exits the moment that connection
/// closes. Nothing else ends it — not the page going quiet, not the browser being closed.

#[cfg(not(target_os = "android"))]
    None
}

#[cfg(not(target_os = "android"))]

/// Shared secret every `/api/*` route requires. Filled in by [`serve`].
static TOKEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[cfg(not(target_os = "android"))]
fn persist_token(_token: &str) {}

#[cfg(not(target_os = "android"))]
fn load_or_create_token() -> String {
}

/// Percent-decode a query value. The frontend encodes paths with `encodeURIComponent`, so
/// UTF-8 file names arrive escaped.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(req: &HttpRequest, key: &str) -> Option<String> {
    let (_, query) = req.path.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn require_param(req: &HttpRequest, key: &str) -> Result<String> {
    query_param(req, key)
        .filter(|v| !v.is_empty())
        .with_context(|| format!("缺少参数 {key}"))
}

/// Join a client-supplied file name onto a directory, refusing anything that escapes it.
///
/// This is a root file manager, so browsing anywhere is intended — but an upload target
/// arriving as `../../x` is a bug, not a feature.
fn safe_join(dir: &str, name: &str) -> Result<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        bail!("非法文件名：{name}");
    }
    Ok(std::path::Path::new(dir).join(name).display().to_string())
}

fn json_body<T: serde::de::DeserializeOwned>(req: &HttpRequest) -> Result<T> {
    serde_json::from_slice(&req.body).with_context(|| "请求体不是合法 JSON")
}

/// The package name inside an APK, for the file manager's install prompt.
///
/// `archive`+`entry` names a member of a zip; anything else is a path on the device.
fn apk_info(req: &HttpRequest) -> Result<serde_json::Value> {
    let archive = query_param(req, "archive").unwrap_or_default();
    let package = if archive.is_empty() {
        crate::webui_files::apk_package(&require_param(req, "path")?)?
    } else {
        crate::webui_files::apk_package_in_archive(&archive, &require_param(req, "entry")?)?
    };
    Ok(serde_json::json!({ "package": package }))
}

/// The shape every mutating route shares: root, then the JSON body, then the call.
///
/// Spelled out once because the order is the point — a caller without root is refused before its
/// body is parsed — and a chain written out again in every arm is how one of them ends up doing
/// it the other way round. `R` is what the call returns; [`rooted_ok`] is this for the calls
/// whose only answer is `"ok"`.
fn rooted<T, R>(
    stream: &mut TcpStream,
    req: &HttpRequest,
    f: impl FnOnce(T) -> Result<R>,
) -> Result<()>
where
    T: serde::de::DeserializeOwned,
    R: Serialize,
{
    fs_json(
        stream,
        crate::webui_files::require_root()
            .and_then(|()| json_body::<T>(req))
            .and_then(f),
    )
}

/// [`rooted`] for the calls that answer nothing but `"ok"`.
fn rooted_ok<T>(
    stream: &mut TcpStream,
    req: &HttpRequest,
    f: impl FnOnce(T) -> Result<()>,
) -> Result<()>
where
    T: serde::de::DeserializeOwned,
{
    fs_json(
        stream,
        crate::webui_files::require_root()
            .and_then(|()| json_body::<T>(req))
            .and_then(f)
            .map(|()| "ok"),
    )
}

/// The MIME table for everything served off the device.
///
/// One table for both the file manager's previews and a module's own WebUI files: a `.js` or a
/// `.json` does not change what it is depending on which route served it, and two tables only
/// meant two places to forget an extension in.
fn mime_for_name(name: &str) -> &'static str {
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "txt" | "log" | "md" | "sh" | "conf" | "prop" | "rc" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Render a filesystem result, keeping the whole error chain.
///
/// `{:#}` matters here: it carries the errno (`Permission denied (os error 13)`) up to the
/// UI, so a failure caused by SELinux instead of by permissions is distinguishable.
fn fs_json<T: Serialize>(stream: &mut TcpStream, result: Result<T>) -> Result<()> {
    match result {
        Ok(value) => json_response(stream, &ApiResponse::ok(value)),
        Err(e) => error_response(stream, 500, &format!("{e:#}")),
    }
}

#[derive(serde::Deserialize)]
struct FsMkdir {
    path: String,
}

#[derive(serde::Deserialize)]
struct FsRename {
    from: String,
    to: String,
}

#[derive(serde::Deserialize)]
struct FsTransfer {
    paths: Vec<String>,
    to: String,
    #[serde(default)]
    overwrite: bool,
}

#[derive(serde::Deserialize)]
struct FsDelete {
    paths: Vec<String>,
    #[serde(default)]
    recursive: bool,
}

#[derive(serde::Deserialize)]
struct FsMode {
    paths: Vec<String>,
    mode: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(serde::Deserialize)]
struct FsOwner {
    paths: Vec<String>,
    uid: u32,
    gid: u32,
    #[serde(default)]
    recursive: bool,
}

#[derive(serde::Deserialize)]
struct FsWrite {
    path: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct FsExtract {
    archive: String,
    dest: String,
    #[serde(default)]
    entries: Vec<String>,
}

#[derive(serde::Deserialize)]
struct FsArchiveAdd {
    archive: String,
    /// Paths on the device: a file goes in as it is, a folder with everything under it.
    sources: Vec<String>,
    /// Folder inside the archive to put them under — the one the file manager is showing.
    #[serde(default)]
    sub: String,
}


#[derive(serde::Deserialize)]
struct JumpsBody {
    jumps: Vec<crate::webui_files::Jump>,
}

#[derive(serde::Deserialize)]
struct QuickRunBody {
    quick_run: Vec<crate::webui_files::QuickRun>,
}

#[derive(serde::Deserialize)]
struct FsSearch {
    /// The directory to search under — the one the file manager is looking at.
    path: String,
    /// What the name has to contain, case-insensitively.
    query: String,
    /// How deep to walk and how many answers to keep: 0 means the default.
    #[serde(default)]
    depth: usize,
    #[serde(default)]
    limit: usize,
}

#[derive(serde::Deserialize)]
struct FsInstall {
    /// A path on the device — or, for an installer inside an archive, the archive and the
    /// member inside it instead.
    #[serde(default)]
    path: String,
    #[serde(default)]
    archive: String,
    #[serde(default)]
    entry: String,
}

#[derive(serde::Deserialize)]
struct KeymintScoop {
    packages: Vec<String>,
}

#[derive(serde::Deserialize)]
struct KeymintFixProps {
    enable: bool,
}

#[derive(serde::Deserialize)]
struct KeymintLogLevel {
    /// `config` for keymint's level, `injector` for the injector's own.
    which: String,
    level: String,
}

#[derive(serde::Deserialize)]
struct KeymintKeybox {
    /// A keybox.xml the user picked with the file manager.
    path: String,
}

#[derive(serde::Deserialize)]
struct KeymintKeyboxContent {
    /// keybox.xml text the page fetched from a remote URL; the download lives in the browser,
    /// this endpoint only validates it and swaps it in.
    content: String,
}

#[derive(serde::Deserialize)]
struct KeymintRestart {
    /// `keymint`, `injector` or `all`.
    what: String,
}

#[derive(serde::Deserialize)]
struct AppsExtract {
    package: String,
    /// Where the APK (or the folder holding base and splits) is written.
    dest: String,
    /// The app's label, used for the file name when it is a usable one.
    #[serde(default)]
    label: String,
}

#[derive(serde::Deserialize)]
struct FsArchiveWrite {
    archive: String,
    /// The member to replace.
    entry: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct FsArchiveDelete {
    archive: String,
    /// Member names: a folder name takes everything under it with it.
    entries: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ShellOpen {
    #[serde(default)]
    cwd: String,
}

#[derive(serde::Deserialize)]
struct ShellInput {
    id: u32,
    data: String,
}

#[derive(serde::Deserialize)]
struct ShellClose {
    id: u32,
}

/// The options object the APK's `exec(cmd, options, cb)` accepts, kept under the same name.
#[derive(serde::Deserialize)]
struct ExecOptions {
    #[serde(default = "default_cwd")]
    cwd: String,
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    cmd: String,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    options: Option<ExecOptions>,
}

#[derive(serde::Deserialize)]
struct SpawnRequest {
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "default_cwd")]
    cwd: String,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct SpawnClose {
    id: u32,
}

#[derive(serde::Deserialize)]
struct ModulePath {
    path: String,
}

fn default_cwd() -> String {
    "/".to_string()
}

/// Parse a module's state into the shape the JS bridge's `moduleInfo()` returns.
///
/// The APK builds this from `ksud module list`'s entry for the module, with `moduleDir`
/// added, and those values are **all strings** — `enabled` arrives as `"true"`, not as a
/// boolean. Matching that exactly is the whole point: a module doing
/// `info.enabled === 'true'` (or reading `info.moduleDir` to find its own files) breaks on
/// anything else, and that is why the two sides used to disagree.
#[cfg(target_os = "android")]
fn module_info_json(id: &str) -> Result<serde_json::Value> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        bail!("非法的模块 id：{id}");
    }

    let active = std::path::Path::new(crate::defs::MODULE_DIR).join(id);
    let pending = std::path::Path::new(crate::defs::MODULE_UPDATE_DIR).join(id);
    let active_ok = active.join("module.prop").exists();
    let pending_ok = pending.join("module.prop").exists();
    // Decided up front: the answer is needed again below, after `pending` has been moved.
    let is_pending = !active_ok && pending_ok;
    let base = if active_ok {
        active
    } else if pending_ok {
        pending
    } else {
        bail!("模块 {id} 不存在");
    };

    let mut fields = serde_json::Map::new();
    // `moduleDir` first, like the APK.
    fields.insert(
        "moduleDir".to_string(),
        serde_json::Value::String(base.display().to_string()),
    );
    for (key, value) in module::read_module_prop(&base).unwrap_or_default() {
        fields.insert(key, serde_json::Value::String(value));
    }
    fields.insert("id".to_string(), serde_json::Value::String(id.to_string()));

    let flag = |yes: bool| serde_json::Value::String(yes.to_string());
    let removed = base.join(defs::REMOVE_FILE_NAME).exists();
    fields.insert(
        "enabled".to_string(),
        flag(!base.join(defs::DISABLE_FILE_NAME).exists() && !removed && !is_pending),
    );
    fields.insert("update".to_string(), flag(is_pending));
    fields.insert("remove".to_string(), flag(removed));
    fields.insert(
        "web".to_string(),
        flag(base.join(defs::MODULE_WEB_DIR).exists()),
    );
    fields.insert(
        "action".to_string(),
        flag(base.join(defs::MODULE_ACTION_SH).exists()),
    );
    fields.insert(
        "mount".to_string(),
        flag(base.join("system").exists() && !base.join("skip_mount").exists()),
    );

    Ok(serde_json::Value::Object(fields))
}

/// Serve a file's bytes, either inline (preview) or as a download.
/// Serve one file's bytes inline, for the file manager's previews.
fn serve_file_bytes(stream: &mut TcpStream, req: &HttpRequest) -> Result<()> {
    let path = match require_param(req, "path") {
        Ok(path) => path,
        Err(e) => return error_response(stream, 400, &format!("{e:#}")),
    };

    let bytes = match crate::webui_files::read_bytes(&path, crate::webui_files::MAX_RAW_BYTES) {
        Ok(bytes) => bytes,
        Err(e) => return error_response(stream, 500, &format!("{e:#}")),
    };

    let name = std::path::Path::new(&path)
        .file_name()
        .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().into_owned());

    send_response_with(stream, 200, mime_for_name(&name), &bytes, "")
}

fn handle_client(mut stream: TcpStream) {
    // Every request counts as activity, including ones that end in a 404.
    touch();

    // Set read timeout
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));

    match parse_request(&mut stream) {
        Ok(req) => {
            if let Err(e) = handle_request(&mut stream, &req) {
                log::warn!("Error handling request: {e}");
                let _ = error_response(&mut stream, 500, &e.to_string());
            }
        }
        Err(failure) => {
            log::debug!("Error parsing request: {}", failure.message());
            let _ = error_response(&mut stream, failure.status(), failure.message());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// The token lives in a process-wide static, so tests that start a server must not
    /// overlap or they would clobber each other's token.
    static SERVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serve_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The token is process-wide, so wipe whatever an earlier test left behind: without
        // this a test can read a stale token before its own server has installed one.
        if let Ok(mut slot) = TOKEN.lock() {
            slot.clear();
        }
        guard
    }

    /// Sends one request and returns its status line.
    fn request_status(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ =
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Waits for `serve` to install its token.
    fn wait_for_token() -> String {
        for _ in 0..100 {
            if !token.is_empty() {
                return token;
            }
            thread::sleep(Duration::from_millis(20));
        }
        String::new()
    }

    /// Serves the UI on a fixed port and stays up, so the file manager can be driven from a
    /// real browser on a dev machine — no device, no APK rebuild:
    ///
    ///     cargo test --bin ksud -- --ignored --nocapture serve_for_browser
    ///
    /// The printed token goes in the URL as `?k=`.
    #[test]
    #[ignore]
    fn serve_for_browser() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_secs(900)));
        let token = wait_for_token();
        println!("BROWSER-URL http://127.0.0.1:{port}/?k={token}");
        handle.join().expect("serve");
    }

    #[test]
    fn nothing_is_served_without_the_token() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(1200)));

        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

        let page_unauth = request_status(port, "/");
        let api_unauth = request_status(port, "/api/heartbeat");
        let page_auth = request_status(port, &format!("/?k={token}"));
        let api_auth = request_status(port, &format!("/api/heartbeat?k={token}"));

        // Finding the port must not be enough to even load the page: that is what lets a
        // leaked address be inert.
        assert!(
            page_unauth.starts_with("HTTP/1.1 403"),
            "index page was served without a token: {page_unauth:?}"
        );
        assert!(
            api_unauth.starts_with("HTTP/1.1 403"),
            "unauthenticated API call was not refused: {api_unauth:?}"
        );
        assert!(
            page_auth.starts_with("HTTP/1.1 200"),
            "authenticated page load was refused: {page_auth:?} (token len {})",
            token.len()
        );
        assert!(
            api_auth.starts_with("HTTP/1.1 200"),
            "authenticated API call was refused: {api_auth:?} (token len {})",
            token.len()
        );

        let _ = handle.join();
    }

    /// Full response for one request, truncated for readable assertion output.
    fn request_full(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ =
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf).into_owned();
        // Long enough to clear the headers and still show a JSON body.
        text[..text.len().min(4000)].to_string()
    }

    /// Drives the file manager route through the real HTTP path.
    ///
    /// The release profile is `panic = "abort"`, so a panic anywhere in a handler takes the
    /// whole server down and every later request fails at the transport layer. This exists to
    /// prove listing a directory does not do that.
    #[test]
    fn fs_list_answers_over_http() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(2500)));
        let token = wait_for_token();

        // '.' keeps the test clear of percent-encoding concerns.
        let ok = request_full(port, &format!("/api/fs/list?path=.&k={token}"));
        assert!(
            ok.starts_with("HTTP/1.1 200"),
            "listing did not succeed: {ok}"
        );
        assert!(ok.contains("Cargo.toml"), "listing looks empty: {ok}");

        // The same route is still refused without a token.
        assert!(request_status(port, "/api/fs/list?path=.").starts_with("HTTP/1.1 403"));

        // And the server is still alive after all that.
        assert!(request_status(port, "/api/heartbeat").starts_with("HTTP/1.1 403"));
        assert!(
            !handle.is_finished(),
            "server died while serving file manager routes"
        );

        let _ = handle.join();
    }

    /// A cached copy of this page would keep showing an old UI after the APK was updated,
    /// which is indistinguishable from "the new build did not install".
    #[test]
    fn the_page_is_served_without_caching() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(1500)));
        let token = wait_for_token();

        let reply = request_full(port, &format!("/?k={token}"));
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        let lower = reply.to_lowercase();
        assert!(
            lower.contains("cache-control: no-store"),
            "page may be cached: {reply}"
        );
        assert!(
            lower.contains("set-cookie: ksu_webui="),
            "token cookie missing: {reply}"
        );

        let _ = handle.join();
    }

    #[test]
    fn tokens_are_hex_and_long_enough_to_be_unguessable() {
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn reports_the_port_it_actually_bound() {
        // Port 0 asks the OS to choose; whatever the listener ends up on must be what
        // the caller is told, since that is what gets persisted and handed to the browser.
        assert_ne!(port, 0);
        assert_eq!(listener.local_addr().expect("addr").port(), port);
    }

    #[test]
    fn requests_reset_the_idle_clock() {
        touch();

        thread::sleep(Duration::from_millis(50));

        touch();
    }

    #[test]
    fn serve_returns_once_the_idle_timeout_elapses() {
        let _guard = serve_guard();
        let started = Instant::now();
        serve(listener, Duration::from_millis(300)).expect("serve");

        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(300),
            "returned before going idle"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "idle timeout never fired"
        );
    }

    #[test]
    fn an_open_request_keeps_the_server_alive() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(800)));

        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

        // Keep hitting it the way the page's heartbeat does, token and all; the server must
        // outlive its idle timeout for as long as the pings keep coming.
        let deadline = Instant::now() + Duration::from_millis(2500);
        while Instant::now() < deadline {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
                let _ = stream.write_all(
                    format!("GET /api/heartbeat?k={token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                        .as_bytes(),
                );
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf);
            }
            thread::sleep(Duration::from_millis(150));
        }

        assert!(
            !handle.is_finished(),
            "server shut down while still being pinged"
        );

        // Stop pinging and it should exit on its own shortly after.
        let stop_by = Instant::now() + Duration::from_secs(10);
        while Instant::now() < stop_by && !handle.is_finished() {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            handle.is_finished(),
            "server did not exit after pings stopped"
        );
    }

    /// The launcher's guard connection is the lifetime: while it is held the server serves, even
    /// with the page long gone, and the moment it drops the port is released.
    #[test]
    fn a_watched_server_lives_as_long_as_the_launcher_holds_it() {
        let _guard = serve_guard();
        // Short enough that the idle timeout would have taken the server down on its own.
        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

        let mut guard = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ = guard.write_all(
        );
        let mut answer = [0u8; 64];
        let read = guard.read(&mut answer).expect("the guard is answered");
        let head = String::from_utf8_lossy(&answer[..read]).to_string();
        assert!(head.contains("200"), "guard refused: {head}");

        // Nothing asks for anything for longer than the idle timeout: the browser is closed,
        // which used to be enough to end the server.
        thread::sleep(Duration::from_millis(1200));
        assert!(
            !handle.is_finished(),
            "server shut down while the launcher was still holding it"
        );
        assert!(
            request_status(port, &format!("/api/heartbeat?k={token}")).contains("200"),
            "server stopped answering while the launcher was holding it"
        );

        // Dropping it is what a swipe away, a stop, or a kill does to the app's end of it.
        drop(guard);
        let give_up = Instant::now() + Duration::from_secs(5);
        while Instant::now() < give_up && !handle.is_finished() {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            handle.is_finished(),
            "server outlived the launcher's guard connection"
        );
    }

    /// A launcher that dies before it ever connects must not leave a port open that nothing
    /// can close: until the guard arrives, the idle timeout is still the clock.
    #[test]
    fn a_watched_server_gives_up_when_no_launcher_ever_connects() {
        let _guard = serve_guard();

        let give_up = Instant::now() + Duration::from_secs(5);
        while Instant::now() < give_up && !handle.is_finished() {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            handle.is_finished(),
            "a watched server waited forever for a launcher that never came"
        );
    }

    /// The whole response, for tests that care about the body rather than the status.
    fn request_all(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ =
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// Opening an old address has to say what happened. A bare JSON body on a blank page
    /// is what makes a stale address look like a broken UI, and the API keeps JSON because
    /// the page parses it.
    #[test]
    fn a_stale_address_explains_itself_in_html() {
        let _guard = serve_guard();
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(1200)));
        assert!(!wait_for_token().is_empty(), "serve never produced a token");

        let page = request_all(port, "/");
        assert!(page.starts_with("HTTP/1.1 403"), "got: {page:?}");
        assert!(
            page.contains("text/html"),
            "a navigation should not get JSON: {page:?}"
        );
        assert!(
            page.contains("启动器"),
            "the page should say how to recover: {page:?}"
        );

        let api = request_all(port, "/api/heartbeat");
        assert!(api.starts_with("HTTP/1.1 403"), "got: {api:?}");
        assert!(
            api.contains("application/json"),
            "the API should stay JSON: {api:?}"
        );

        handle.join().ok();
    }

    /// A token rotated on disk while the server runs must be adopted, or a freshly handed
    /// out address is refused by the server that is still serving it.
    #[test]
    fn a_rotated_token_is_read_from_disk() {
        let dir = std::env::temp_dir().join(format!("ksu-token-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("webui.token");

        // Nothing there yet.
        let _ = std::fs::remove_file(&path);
        assert!(
            read_token_file(&path).is_none(),
            "missing file returned a token"
        );

        // Too short to be a token: treated as absent rather than accepted.
        std::fs::write(&path, "short").expect("write");
        assert!(read_token_file(&path).is_none(), "short token was accepted");

        // A real one comes back trimmed, whatever the trailing newline.
        std::fs::write(&path, format!("{token}\n")).expect("write");
        assert_eq!(read_token_file(&path).as_deref(), Some(token.as_str()));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
