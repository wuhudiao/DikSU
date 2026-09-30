
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use pinyin::ToPinyin;
use serde::Serialize;

use crate::defs;
#[cfg(target_os = "android")]
use crate::ksucalls;
#[cfg(target_os = "android")]
use crate::module;
#[cfg(target_os = "android")]
use crate::webui_spawn;

const INDEX_HTML: &str = include_str!("../assets/web/index.html");

const HIDE_MANAGER_FLAG: &str = "/data/adb/ksu/hide_manager";

const KSU_BRIDGE: &str = concat!(
    "<script>",
    include_str!("../assets/web/ksu-bridge.js"),
    "</script>"
);

#[derive(Serialize)]
struct KernelInfo {
    version: i32,
    uapi_version: u32,
    runtime_mode: String,
    is_lkm: bool,
    is_late_load: bool,
}

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
    web: bool,
    remove: bool,
    has_action: bool,
}

#[derive(Serialize)]
struct FeatureState {
    id: u32,
    name: String,
    description: String,
    value: u64,
    enabled: bool,
}

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

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

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

fn forbidden_response(stream: &mut TcpStream, is_navigation: bool) -> Result<()> {
    if is_navigation {
        return send_response(
            stream,
            403,
            "text/html; charset=utf-8",
            FORBIDDEN_HTML.as_bytes(),
        );
    }
    error_response(stream, 403, "forbidden")
}

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

    if std::path::Path::new(defs::MODULE_DIR).exists() {
        module::foreach_module(module::ModuleType::All, |path| {
            if let Some(info) = read_module(path, false) {
                if !module_map.contains_key(&info.id) {
                    module_map.insert(info.id.clone(), info);
                }
            }
            Ok(())
        })?;
    }

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
            let extra = "Cache-Control: no-store, must-revalidate\r\nPragma: no-cache\r\n";
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

#[cfg(target_os = "android")]
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);

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

#[cfg(target_os = "android")]
fn start_module_install_stream(stream: &mut TcpStream, path: &str) -> Result<()> {
    if path.trim().is_empty() {
        return error_response(stream, 400, "请填写模块 zip 的路径");
    }
    if !std::path::Path::new(path).is_file() {
        return error_response(stream, 404, &format!("找不到文件：{path}"));
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ksud"));
    let args = vec![
        "module".to_string(),
        "install".to_string(),
        path.to_string(),
    ];
    match webui_spawn::start(&exe.to_string_lossy(), &args, "/", &[]) {
        Ok(id) => json_response(stream, &ApiResponse::ok(serde_json::json!({ "id": id }))),
        Err(e) => error_response(stream, 500, &format!("{e:#}")),
    }
}

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

    log::info!("module spawn: {} {:?}", request.cmd, request.args);

    let env: Vec<(String, String)> = request.env.into_iter().collect();
    match webui_spawn::start(&request.cmd, &request.args, &request.cwd, &env) {
        Ok(id) => json_response(stream, &ApiResponse::ok(serde_json::json!({ "id": id }))),
        Err(e) => error_response(stream, 500, &format!("{e:#}")),
    }
}

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
        (
            1000,
            "隐藏管理器状态",
            "开启后管理器首页显示「未安装」；内核和网页端都不受影响",
        ),
    ];

    let mut result = Vec::new();
    for (id, name, desc) in &features {
        if *id == 999 {
            let enabled = ksucalls::get_default_umount_modules();
            result.push(FeatureState {
                id: *id,
                name: name.to_string(),
                description: desc.to_string(),
                value: if enabled { 1 } else { 0 },
                enabled,
            });
            continue;
        }
        if *id == 1000 {
            let hidden = std::path::Path::new(HIDE_MANAGER_FLAG).exists();
            result.push(FeatureState {
                id: *id,
                name: name.to_string(),
                description: desc.to_string(),
                value: if hidden { 1 } else { 0 },
                enabled: true,
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
#[cfg(target_os = "android")]
fn label_from_apk_path(apk_path: &str) -> Option<String> {
    let file = std::fs::File::open(apk_path).ok()?;
    let label = crate::apkparser::extract_label(file)?;
    (!label.is_empty()).then_some(label)
}

#[cfg(target_os = "android")]
fn get_app_labels_from_dex() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let dex_path = format!("{}/applabel.dex", crate::defs::BINARY_DIR);
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

static APP_ICON_CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<String, (String, String)>>,
> = std::sync::Mutex::new(None);

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

#[cfg(target_os = "android")]
fn get_app_icon_base64(pkg_name: &str) -> Option<(String, String)> {
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

    let result = icon_from_disk(pkg_name);
    if let Some(result) = result {
        if let Ok(mut cache_guard) = APP_ICON_CACHE.lock() {
            if let Some(cache) = cache_guard.as_mut() {
                cache.insert(pkg_name.to_string(), result.clone());
            }
        }
        return Some(result);
    }

    let dex_path = format!("{}/appicon.dex", crate::defs::BINARY_DIR);
    if let Ok(data) = crate::assets::get_asset_data("appicon.dex") {
        let _ = std::fs::create_dir_all(crate::defs::BINARY_DIR);
        let _ = std::fs::write(&dex_path, &data);
    }
    if !std::path::Path::new(&dex_path).exists() {
        return None;
    }

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

    {
        let mut cache_guard = APP_ICON_CACHE.lock().unwrap();
        if let Some(cache) = cache_guard.as_mut() {
            cache.insert(pkg_name.to_string(), result.clone());
        }
    }

    Some(result)
}

#[cfg(target_os = "android")]
fn icon_from_disk(pkg_name: &str) -> Option<(String, String)> {
    let dir = format!("{}/app_icons", crate::defs::WORKING_DIR);
    let png = std::path::Path::new(&dir).join(format!("{}.png", pkg_name.replace('/', "_")));
    let png_time = std::fs::metadata(&png).and_then(|m| m.modified()).ok()?;

    if let Some(apk) = cached_apk_path(pkg_name)
        && let Ok(apk_time) = std::fs::metadata(&apk).and_then(|m| m.modified())
        && apk_time > png_time
    {
        return None;
    }

    let bytes = std::fs::read(&png).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some((base64_encode(&bytes), "image/png".to_string()))
}

#[cfg(target_os = "android")]
fn cached_apk_path(pkg_name: &str) -> Option<String> {
    let guard = APPS_CACHE.lock().ok()?;
    let (built_at, rows) = guard.as_ref()?;
    if built_at.elapsed() >= APPS_CACHE_TTL {
        return None;
    }
    rows.iter()
        .find(|row| row.package == pkg_name)
        .map(|row| row.apk_path.clone())
        .filter(|path| !path.is_empty())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(not(target_os = "android"))]
fn get_app_icon_base64(_pkg_name: &str) -> Option<(String, String)> {
    None
}

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

#[cfg(target_os = "android")]
const SOFT_REBOOT_LOG: &str = "/data/adb/ksu/log/soft-reboot.log";

#[cfg(target_os = "android")]
fn ksud_path() -> std::path::PathBuf {
    let mut candidates = vec![
        std::path::PathBuf::from(defs::DAEMON_PATH),
        std::path::PathBuf::from(defs::DAEMON_LINK_PATH),
    ];
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe);
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("ksud"))
}

#[cfg(target_os = "android")]
fn soft_reboot_log_tail() -> String {
    let Ok(text) = std::fs::read_to_string(SOFT_REBOOT_LOG) else {
        return String::new();
    };
    let mut tail: Vec<&str> = text.lines().rev().take(4).collect();
    tail.reverse();
    format!(" · 日志末尾：{}", tail.join(" / "))
}

#[cfg(target_os = "android")]
fn soft_reboot_child() -> Result<()> {
    use std::process::{Command, Stdio};

    ksucalls::ensure_uapi_version_matched()
        .map_err(|e| anyhow::anyhow!("内核模块与 ksud 版本不一致，先重新越狱再试（{e}）"))?;

    let exe = ksud_path();
    let log_path = std::path::Path::new(SOFT_REBOOT_LOG);
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("打不开 {SOFT_REBOOT_LOG}"))?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let _ = writeln!(log, "\n=== web UI 请求软重启（{seconds}）===");

    let mut child = Command::new(&exe)
        .arg("soft-reboot")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| format!("启动 {} soft-reboot 失败", exe.display()))?;

    for _ in 0..40 {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                bail!(
                    "ksud soft-reboot 没起来（{status}）{}",
                    soft_reboot_log_tail()
                )
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e).context("等 ksud soft-reboot 失败"),
        }
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn reboot_device(mode: &str) {
    use std::process::Command;

    if mode == "recovery" {
        let _ = Command::new("/system/bin/input")
            .args(["keyevent", "26"])
            .status();
    }

    let reason = match mode {
        "userspace" | "recovery" | "bootloader" | "download" | "edl" => format!(" {mode}"),
        _ => String::new(),
    };
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
        if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
            let kernel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.insert(
                "kernelVersion".to_string(),
                serde_json::Value::String(kernel),
            );
        }

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

        if let Ok(output) = std::process::Command::new("getprop")
            .arg("ro.build.fingerprint")
            .output()
        {
            let fp = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.insert("fingerprint".to_string(), serde_json::Value::String(fp));
        }

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

#[cfg(target_os = "android")]
#[derive(Clone)]
struct AppRow {
    package: String,
    uid: i32,
    is_system: bool,
    label: String,
    version_code: i64,
    apk_path: String,
    size: u64,
}

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

#[cfg(target_os = "android")]
fn collect_apps() -> Vec<AppRow> {
    use std::collections::HashSet;

    let system: HashSet<String> = parse_pm_lines(&pm_query(&["-s", "-U"]))
        .into_iter()
        .map(|(name, _, _, _)| name)
        .collect();

    let mut listed = parse_pm_lines(&pm_query(&["-f", "-U", "--show-versioncode"]));
    if listed.is_empty() {
        listed = parse_pm_lines(&pm_query(&["-f", "-U"]));
    }

    let labels = app_labels(&listed);

    let rows: Vec<AppRow> = listed
        .into_iter()
        .filter(|(name, _, _, _)| !name.is_empty() && name != defs::DEFAULT_PACKAGE_NAME)
        .map(|(package, apk_path, uid, version_code)| {
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

    let mut keyed: Vec<(bool, String, AppRow)> = rows
        .into_iter()
        .map(|row| {
            let granted = ksucalls::is_app_granted(row.uid);
            let pinyin: String = row
                .label
                .as_str()
                .to_pinyin()
                .map(|p| p.map(|py| py.plain()).unwrap_or(""))
                .collect();
            (granted, pinyin.to_lowercase(), row)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, _, row)| row).collect()
}

#[cfg(target_os = "android")]
fn app_labels(
    listed: &[(String, String, i32, i64)],
) -> std::collections::HashMap<String, String> {
    use std::hash::{Hash, Hasher};

    let cache_path = format!("{}/app_labels.cache", crate::defs::WORKING_DIR);
    let locale = crate::utils::getprop("persist.sys.locale").unwrap_or_default();

    let mut material = String::with_capacity(locale.len() + listed.len() * 64);
    material.push_str(&locale);
    material.push('\n');
    for (package, apk, _, _) in listed {
        let (size, mtime) = std::fs::metadata(apk).map_or((0, 0), |md| {
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (md.len(), mtime)
        });
        material.push_str(&format!("{package}\t{apk}\t{size}\t{mtime}\n"));
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    material.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    if let Ok(text) = std::fs::read_to_string(&cache_path)
        && let Some((cached_key, body)) = text.split_once('\n')
        && cached_key == key
    {
        let mut map = std::collections::HashMap::new();
        for line in body.lines() {
            if let Some((package, label)) = line.split_once('\t') {
                map.insert(package.to_string(), label.to_string());
            }
        }
        if !map.is_empty() {
            return map;
        }
    }

    let labels = get_app_labels_from_dex();
    if !labels.is_empty() {
        let mut packages: Vec<&String> = labels.keys().collect();
        packages.sort();
        let mut out = String::with_capacity(key.len() + labels.len() * 32);
        out.push_str(&key);
        out.push('\n');
        for package in packages {
            let label = &labels[package];
            if label.contains('\n') {
                continue;
            }
            out.push_str(package);
            out.push('\t');
            out.push_str(label);
            out.push('\n');
        }
        let staging = format!("{cache_path}.new");
        if std::fs::write(&staging, out).is_ok() {
            let _ = std::fs::rename(&staging, &cache_path);
        }
    }
    labels
}

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

fn handle_request(stream: &mut TcpStream, req: &HttpRequest, peer: Option<u32>) -> Result<()> {
    let path = req.path.split('?').next().unwrap_or("/");

    let redeemed = query_param(req, "t").is_some_and(|ticket| redeem_ticket(&ticket, peer));
    if req.method != "OPTIONS" && !redeemed && !token_matches(&request_token(req)) {
        let token = request_token(req);
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
            current_token().len()
        );
        return forbidden_response(stream, path == "/" || path == "/index.html");
    }

    if req.method == "OPTIONS" {
        return send_response(stream, 204, "text/plain", &[]);
    }

    match (req.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => {
            let token = request_token(req);
            let mut extra =
                String::from("Cache-Control: no-store, must-revalidate\r\nPragma: no-cache\r\n");
            if redeemed || token_matches(&token) {
                extra.push_str(&format!(
                    "Set-Cookie: {TOKEN_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict\r\n",
                    current_token()
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

        ("GET", p) if p.starts_with("/modweb/") => serve_module_web(stream, p),

        ("GET", p) if p.starts_with("/assets/") || p.starts_with("/internal/") => {
            serve_module_asset_by_referer(stream, p, req)
        }

        ("GET", "/api/heartbeat") | ("POST", "/api/heartbeat") => {
            json_response(stream, &ApiResponse::ok("alive"))
        }

        ("GET", "/api/mint") | ("POST", "/api/mint") => {
            let uid = query_param(req, "uid").and_then(|v| v.trim().parse::<u32>().ok());
            let ticket = mint_ticket(uid);
            let port = LISTEN_PORT.load(Ordering::Relaxed);
            log::info!("Web UI: minted a one-time link for uid {uid:?}");
            json_response(
                stream,
                &serde_json::json!({
                    "url": format!("http://127.0.0.1:{port}/?t={ticket}"),
                    "ttl": TICKET_TTL.as_secs(),
                }),
            )
        }

        ("POST", "/api/close") | ("GET", "/api/close") => {
            log::info!("Web UI: page closed, rotating the token so the cookie dies with it");
            reset_token()?;
            json_response(stream, &ApiResponse::ok("closed"))
        }

        ("GET", "/api/watch") => watch_connection(stream),

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

        ("POST", "/api/fs/search") => fs_json(
            stream,
            json_body::<FsSearch>(req).and_then(|body| {
                let depth = if body.depth == 0 {
                    6
                } else {
                    body.depth.min(12)
                };
                let limit = if body.limit == 0 {
                    300
                } else {
                    body.limit.min(1000)
                };
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

        ("GET", "/api/fs/apkicon") => match require_param(req, "path") {
            Ok(path) => match apk_icon(&path) {
                Ok(bytes) => send_response_with(stream, 200, "image/png", &bytes, ""),
                Err(e) => error_response(stream, 404, &format!("{e:#}")),
            },
            Err(e) => error_response(stream, 400, &format!("{e:#}")),
        },

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

        ("GET", "/api/fs/quickrun") => json_response(
            stream,
            &ApiResponse::ok(crate::webui_files::load_quick_run()),
        ),

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
                let id = require_param(req, "id").unwrap_or_default();
                if id.trim().is_empty() {
                    error_response(stream, 400, "缺少参数 id")
                } else {
                    match module_info_json(id.as_str()) {
                        Ok(info) => json_response(stream, &ApiResponse::ok(info)),
                        Err(e) => error_response(stream, 500, &format!("{e:#}")),
                    }
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

        ("GET", "/api/module/packages") => {
            let kind = query_param(req, "type").unwrap_or_default();
            json_response(stream, &ApiResponse::ok(bridge_package_names(&kind)))
        }

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
                let started = std::time::Instant::now();
                if let Err(e) = std::fs::create_dir_all(defs::WORKING_DIR) {
                    return error_response(
                        stream,
                        500,
                        &format!("创建 {} 失败：{e}", defs::WORKING_DIR),
                    );
                }
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
                let _ = std::fs::remove_file(&tmp_path);
                result
            }
            #[cfg(not(target_os = "android"))]
            {
                error_response(stream, 500, "Not supported on this platform")
            }
        }

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

        ("POST", "/api/modules/install-stream") => {
            #[cfg(target_os = "android")]
            {
                match json_body::<ModulePath>(req) {
                    Ok(body) => start_module_install_stream(stream, &body.path),
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
                    return match ksucalls::set_default_umount_modules(value != 0) {
                        Ok(()) => json_response(stream, &ApiResponse::ok("Updated")),
                        Err(e) => error_response(stream, 500, &e.to_string()),
                    };
                }
                if id == 1000 {
                    let path = std::path::Path::new(HIDE_MANAGER_FLAG);
                    let done = if value != 0 {
                        std::fs::write(path, b"1\n")
                    } else {
                        match std::fs::remove_file(path) {
                            Ok(()) => Ok(()),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                            Err(e) => Err(e),
                        }
                    };
                    return match done {
                        Ok(()) => json_response(stream, &ApiResponse::ok("Updated")),
                        Err(e) => error_response(stream, 500, &e.to_string()),
                    };
                }
                match ksucalls::set_feature(id, value) {
                    Ok(()) => {
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

                if mode == "soft_reboot" {
                    return match soft_reboot_child() {
                        Ok(()) => json_response(stream, &ApiResponse::ok(reboot_label(mode))),
                        Err(e) => error_response(stream, 500, &format!("软重启没能启动：{e:#}")),
                    };
                }

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
                match run_hma_config(query_param(req, "scene").is_some()) {
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
fn run_hma_config(scene: bool) -> Result<String> {
    use std::io::Write;
    use std::process::Command;

    let script = r#"#!/system/bin/sh
PKG=org.frknkrc44.hma_oss
# 新版（future-*）把配置搬到了 /data/misc/hide_my_applist_<随机>/config.json：root/system 属主、
# 单行紧凑 JSON。应用私有目录里那份 files/config.json 是旧格式遗留，模块根本不读它。
CONFIG=$(ls /data/misc/hide_my_applist_*/config.json 2>/dev/null | head -1)

# Without this the config lookup below would report "还没生成配置文件" for an app that is not installed at all.
if ! pm list packages | grep -q "^package:$PKG$"; then
    echo "未安装 HMA（$PKG）"
    exit 1
fi

if [ -z "$CONFIG" ]; then
    echo "找不到 HMA-OSS 的配置文件（/data/misc/hide_my_applist_*/config.json）：先打开一次 HMA-OSS 再试"
    exit 1
fi

# 整份配置就一行。下面的处理全部以**文件**为单位：grep/sed 读文件、重定向写文件，绝不把整份
# JSON 当成某个命令的参数 —— Linux 给单个参数的上限是 128KB，装的应用一多，`printf '%s' "$LINE"`
# 这种写法就会直接 `Argument list too long`（群友手机上那条报错就是这么来的）。
TMPDIR=/data/adb/ksu
HEAD_F="$TMPDIR/hma_head.$$"
BODY_F="$TMPDIR/hma_body.$$"
NEW_F="$TMPDIR/hma_new.$$"
trap 'rm -f "$HEAD_F" "$BODY_F" "$BODY_F.2" "$NEW_F"' EXIT

# 一次遍历切成两半：scope 之前（含 `"scope":{`）写 head 文件，之后写 body 文件。
if ! awk -v head="$HEAD_F" -v body="$BODY_F" -v marker='"scope":{' '
    NR == 1 {
        i = index($0, marker)
        if (i == 0) exit 1
        printf "%s", substr($0, 1, i + 8) > head
        printf "%s", substr($0, i + 9) > body
        next
    }
    { exit 2 }
' "$CONFIG"; then
    echo "配置读不动（不是单行 JSON，或没有 scope）：$CONFIG"
    exit 1
fi

# 多余逗号：早先那版脚本在 scope 为空时写出过 {...,} 这种非法 JSON，HMA 自己也读不了它。
# 既然这次要重写这份文件，顺手修掉（改之前照样先备份）。
BROKEN=0
if grep -qF ',}' "$BODY_F" || grep -qF ',]' "$BODY_F"; then
    BROKEN=1
    sed 's/,}/}/g; s/,]/]/g' "$BODY_F" > "$BODY_F.2" && mv -f "$BODY_F.2" "$BODY_F"
    echo "现有配置里有多余逗号（HMA 读不了），这次写回一并修掉"
fi

# 应用预设全勾（HMA 现有的 7 个：无障碍应用、自定义 ROM、检测类、root 类、Shizuku/Dhizuku、
# 可疑应用、Xposed 模块）。设置预设默认勾"无障碍功能"和"开发者选项"。
ALL_APP_PRESETS='"accessibility_apps","custom_rom","detector_apps","root_apps","shizuku_dhizuku","sus_apps","xposed"'
APP_RULE_TEMPLATE='"%s":{"useWhitelist":false,"excludeSystemApps":true,"hideInstallationSource":false,"hideSystemInstallationSource":false,"excludeTargetInstallationSource":false,"invertActivityLaunchProtection":false,"excludeVoldIsolation":false,"restrictedZygotePermissions":[],"applyTemplates":[],"applyPresets":[%s],"applySettingTemplates":[],"applySettingsPresets":[%s],"extraAppList":[%s],"extraOppositeAppList":[]}'

# 每个应用上要勾的预设，对应 HMA「模板设置 → 选择预设」那几个勾：默认勾「无障碍功能」和
# 「开发者选项」。Scene 版去掉无障碍 —— Scene 靠无障碍服务读应用，只要范围内的应用被隐藏
# 了无障碍，它照样用不了，所以这一项在 Scene 版里整个不勾。
APPLY_SETTINGS_PRESETS='"accessibility","dev_options"'
if [ -n "$HMA_NO_ACCESSIBILITY" ]; then
    APPLY_SETTINGS_PRESETS='"dev_options"'
fi

EXCLUDED_PACKAGES="eu.darken.sdmse me.weishu.kernelsu bin.mt.plus.canary bin.mt.plus org.telegram.messenger org.telegram.group me.bmax.apatch"

# 管理器自己的包名由 ksud 传进来（分叉后不是 me.weishu.kernelsu 了），Scene 版再多塞一个。
# 单独判断再拼，是因为空串也会拼出一个空元素，而下面把它整个当正则用（sed 's/ /|/g'）：空元素
# 会让 grep -v 把所有包都排掉 —— 那是一个应用都写不进去，却还报成功。
for EXTRA in "$HMA_MANAGER_PKG" "$HMA_EXTRA_EXCLUDE"; do
    [ -n "$EXTRA" ] && EXCLUDED_PACKAGES="$EXCLUDED_PACKAGES $EXTRA"
done

# 网页端启动器（现在是那个计算器）的包名每次安装都会重新生成，安装时由管理器写在这个文件里
# —— 这是唯一可靠的来源。**不猜**：以前这里会按包名形状（com.<5 个小写字母>.<5 个小写字母>）
# 兜底猜一个，撞上了就会把那个不相干的应用从所有应用里额外隐藏掉；宁可这次不写 extraAppList。
LAUNCHER_PKG=$(head -1 /data/adb/ksu/calculator.pkg 2>/dev/null | tr -d '[:space:]')
if [ -n "$LAUNCHER_PKG" ] && ! pm list packages -3 | grep -q "^package:$LAUNCHER_PKG$"; then
    # 文件里记的那个包已经卸载了：当作没有启动器，不要退回猜。
    LAUNCHER_PKG=""
fi
if [ -n "$LAUNCHER_PKG" ]; then
    # 启动器自己不需要隐藏别的东西，所以不进隐藏范围；反过来要把它从别的应用里隐藏掉。
    EXCLUDED_PACKAGES="$EXCLUDED_PACKAGES $LAUNCHER_PKG"
    EXTRA_APP_LIST="\"$LAUNCHER_PKG\""
else
    EXTRA_APP_LIST=""
fi

EXCLUDE_REGEX=$(echo "$EXCLUDED_PACKAGES" | sed 's/ /|/g')
ALL_USER_PACKAGES=$(pm list packages -3 | sed 's/^package://' | grep -v -E "$EXCLUDE_REGEX")

# 没配过的应用补一条；已经配过的把预设补成"全选"——只改那两条预设数组，条目里别的字段
# （白名单模式、额外隐藏、模板）一个字节都不动，别的应用也完全不碰。
OLD_COUNT=$(grep -o '"useWhitelist"' "$BODY_F" | wc -l | tr -d ' ')
# scope 里已经有哪些包：一次 grep 拿全。千万别按包名在整行上做 shell 模式匹配——mksh 在 37KB
# 的单行上匹配一次要 0.2 秒，六十几条配置就是一分多钟，按钮点下去半天不跳转就是这么来的。
SCOPE_PKGS=$(grep -o '"[^"]*":{"useWhitelist"' "$BODY_F" | sed 's/":{"useWhitelist"$//; s/^"//' | tr '\n' ' ')
SPACED=" $SCOPE_PKGS "

# 新条目直接追加进 new 文件，不在变量里拼成一大串：那是同一个 128KB 限制的第二个坑。
ADDED=0
cat "$HEAD_F" > "$NEW_F"
for PKG_NAME in $ALL_USER_PACKAGES; do
    case "$SPACED" in
        *" $PKG_NAME "*) continue ;;
    esac
    [ "$ADDED" -gt 0 ] && printf ',' >> "$NEW_F"
    printf "$APP_RULE_TEMPLATE" "$PKG_NAME" "$ALL_APP_PRESETS" "$APPLY_SETTINGS_PRESETS" "$EXTRA_APP_LIST" >> "$NEW_F"
    ADDED=$((ADDED + 1))
done

# 已经配过的条目：把两条预设数组补成"全选"。sed 直接读 body 文件、结果写回同一个文件，
# 顺带把启动器写进 extraAppList。其余字段、别的应用都原样不动。
WANT_APP='"applyPresets":['"$ALL_APP_PRESETS"']'
UPGRADED=$(grep -o '"applyPresets":\[[^]]*\]' "$BODY_F" | grep -v -F -x "$WANT_APP" | wc -l | tr -d ' ')
CHANGED=0
sed -e "s#\"applyPresets\":\[[^]]*\]#$WANT_APP#g" \
    -e "s#\"applySettingsPresets\":\[[^]]*\]#\"applySettingsPresets\":[$APPLY_SETTINGS_PRESETS]#g" \
    "$BODY_F" > "$BODY_F.2"
if [ -n "$EXTRA_APP_LIST" ]; then
    sed "s#\"extraAppList\":\[[^]]*\]#\"extraAppList\":[$EXTRA_APP_LIST]#g" "$BODY_F.2" > "$BODY_F.2.b" &&
        mv -f "$BODY_F.2.b" "$BODY_F.2"
fi
cmp -s "$BODY_F" "$BODY_F.2" || CHANGED=1
mv -f "$BODY_F.2" "$BODY_F"
SKIPPED=$((OLD_COUNT - UPGRADED))

if [ "$ADDED" -gt 0 ] || [ "$UPGRADED" -gt 0 ] || [ "$BROKEN" -eq 1 ] || [ "$CHANGED" -eq 1 ]; then
    STAMP=$(date +%Y%m%d-%H%M%S)
    cp -f "$CONFIG" "$CONFIG.bak-$STAMP" || {
        echo "备份失败，没动原文件：$CONFIG"
        exit 1
    }

    # 新条目插在 "scope":{ 后面，其余（全局设置、别人配过的应用、模板）原样留着。
    # scope 本来就是空的时候（body 以 } 开头）后面不能再跟逗号，那会写出 {,... 这种非法 JSON。
    SEP=""
    if [ "$ADDED" -gt 0 ] && [ "$(head -c 1 "$BODY_F")" != "}" ]; then
        SEP=","
    fi
    printf '%s' "$SEP" >> "$NEW_F"
    cat "$BODY_F" >> "$NEW_F"

    # 写入前自检：条目数正好多出 ADDED 条、括号配平、没有空元素/多余逗号。不对就整份放弃，原文件不动。
    NEW_COUNT=$(grep -o '"useWhitelist"' "$NEW_F" 2>/dev/null | wc -l | tr -d ' ')
    OPEN=$(tr -cd '{' < "$NEW_F" | wc -c | tr -d ' ')
    CLOSE=$(tr -cd '}' < "$NEW_F" | wc -c | tr -d ' ')
    if [ "$NEW_COUNT" != "$((OLD_COUNT + ADDED))" ] || [ "$OPEN" != "$CLOSE" ] ||
        grep -qF ',}' "$NEW_F" || grep -qF ',]' "$NEW_F" || grep -qF ',,' "$NEW_F" ||
        grep -qF '{,' "$NEW_F"; then
        echo "自检没过（条目 $NEW_COUNT/$((OLD_COUNT + ADDED))，括号 $OPEN/$CLOSE），没动原文件"
        exit 1
    fi

    # 原地写入：inode 不动，system 属主、600 权限、SELinux 上下文都保持原样。
    cat "$NEW_F" > "$CONFIG"
fi

if [ -n "$LAUNCHER_PKG" ] && [ $((OLD_COUNT + ADDED)) -gt 0 ]; then
    # 启动器应该在每条配置的额外隐藏里各出现一次。少了就说明哪条没写全。
    LAUNCHER_MENTIONS=$(grep -o "$LAUNCHER_PKG" "$CONFIG" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$LAUNCHER_MENTIONS" -lt $((OLD_COUNT + ADDED)) ]; then
        echo "额外隐藏启动器没写全：$((OLD_COUNT + ADDED)) 条配置里只出现 $LAUNCHER_MENTIONS 次"
        exit 1
    fi
fi

# 模块把配置缓存在内存里，写完必须让它重读一次才算生效。新版把「重新加载配置文件」挂在
# HMA-OSS 首页那张状态卡的长按上（应用里的长按动作），所以这里替用户按一下。
input keyevent KEYCODE_WAKEUP > /dev/null 2>&1
am start -n "$PKG/.MainActivityLauncher" > /dev/null 2>&1

# 等它真的到前台再按。看前台窗口比 uiautomator 稳——那玩意儿时不时回一句
# "null root node returned by UiTestAutomationBridge"，一次都读不出来。
FOCUS=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
    sleep 1
    FOCUS=$(dumpsys window 2>/dev/null | grep -m1 mCurrentFocus)
    case "$FOCUS" in *"$PKG"*) break ;; esac
done
case "$FOCUS" in
    *"$PKG"*) ;;
    *)
        echo "没能把 HMA-OSS 拉到前台：请在它的首页长按那张蓝色状态卡手动重读一次"
        exit 1
        ;;
esac
sleep 2

# 卡片位置：能从无障碍树里读到就用真坐标，读不到就按屏幕比例估（首页第一块大卡片就在
# 标题下面，y 大概五分之一处）。uiautomator 写不进 /data/adb/ksu，临时文件放别处。
DUMP="/data/local/tmp/hma_ui.$$.xml"
# 它偶尔要跑十秒（有时还只回一句 "null root node"），掐个上限，读不到就用下面的比例兜底。
timeout 3 uiautomator dump "$DUMP" > /dev/null 2>&1
BOUNDS=""
if [ -f "$DUMP" ]; then
    BOUNDS=$(tr '>' '\n' < "$DUMP" | grep -m1 '模块已激活' | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p')
    rm -f "$DUMP"
fi
if [ -n "$BOUNDS" ]; then
    set -- $BOUNDS
    X=$(( ($1 + $3) / 2 ))
    Y=$(( ($2 + $4) / 2 ))
else
    W=$(wm size | sed -n 's/.*: *\([0-9]*\)x\([0-9]*\).*/\1/p' | tail -1)
    H=$(wm size | sed -n 's/.*: *\([0-9]*\)x\([0-9]*\).*/\2/p' | tail -1)
    X=$(( W / 2 ))
    Y=$(( H / 5 ))
fi

input swipe "$X" "$Y" "$X" "$Y" 900
sleep 1
# 退回浏览器：HMA-OSS 是这个脚本替用户拉起来的。
input keyevent 4

if [ -n "$LAUNCHER_PKG" ]; then
    echo "新增 $ADDED 个、补预设 $UPGRADED 个、本来就没问题 $SKIPPED 个；并从它们里额外隐藏启动器（$LAUNCHER_PKG）"
else
    echo "新增 $ADDED 个、补预设 $UPGRADED 个、本来就没问题 $SKIPPED 个（没找到网页端启动器，未做额外隐藏）"
fi
exit 0
"#;

    let script_path = "/data/adb/ksu/hma_config.sh";
    let mut file = std::fs::File::create(script_path)?;
    file.write_all(script.as_bytes())?;
    drop(file);

    let output = Command::new("sh")
        .arg(script_path)
        .env(
            "HMA_EXTRA_EXCLUDE",
            if scene { "com.omarea.vtools" } else { "" },
        )
        .env("HMA_MANAGER_PKG", defs::DEFAULT_PACKAGE_NAME)
        .env("HMA_NO_ACCESSIBILITY", if scene { "1" } else { "" })
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to execute script: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let result = format!("{stdout}\n{stderr}").trim().to_string();

    if !output.status.success() {
        return Err(anyhow::anyhow!("{result}"));
    }

    Ok(result)
}

static LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

static WATCHER: AtomicU8 = AtomicU8::new(WATCHER_NONE);

const WATCHER_NONE: u8 = 0;
const WATCHER_WAITING: u8 = 1;
const WATCHER_CONNECTED: u8 = 2;
const WATCHER_GONE: u8 = 3;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(200);

const BROWSER_POLL_INTERVAL: Duration = Duration::from_secs(2);

static LAST_BROWSER_CHECK: AtomicU64 = AtomicU64::new(0);

fn any_package_in_use(packages: &[String]) -> bool {
    if packages.is_empty() {
        return false;
    }
    let Ok(output) = std::process::Command::new("pidof")
        .args(packages)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(pid_is_in_use)
}

fn pid_is_in_use(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/oom_score_adj"))
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .is_some_and(|adj| adj < CACHED_APP_ADJ)
}

const CACHED_APP_ADJ: i32 = 900;

const PORT_RANGE_START: u16 = 20000;
const PORT_RANGE_END: u16 = 60000;
const PORT_BIND_ATTEMPTS: u32 = 32;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn touch() {
    LAST_ACTIVITY.store(now_millis(), Ordering::Relaxed);
}

fn idle_for() -> Duration {
    let last = LAST_ACTIVITY.load(Ordering::Relaxed);
    Duration::from_millis(now_millis().saturating_sub(last))
}

fn random_u16() -> u16 {
    let mut buf = [0u8; 2];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return u16::from_le_bytes(buf);
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    (nanos ^ std::process::id().rotate_left(16)) as u16
}

#[cfg(target_os = "android")]
fn load_persisted_port() -> Option<u16> {
    std::fs::read_to_string(defs::WEBUI_PORT_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
}

#[cfg(not(target_os = "android"))]
fn load_persisted_port() -> Option<u16> {
    None
}

#[cfg(target_os = "android")]
fn persist_port(port: u16) {
    use std::os::unix::fs::OpenOptionsExt;

    if let Err(e) = std::fs::create_dir_all(defs::WORKING_DIR) {
        log::warn!("failed to create {}: {e}", defs::WORKING_DIR);
        return;
    }
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(defs::WEBUI_PORT_PATH)
        .and_then(|mut f| f.write_all(port.to_string().as_bytes()));
    if let Err(e) = written {
        log::warn!("failed to persist webui port: {e}");
    }
}

#[cfg(not(target_os = "android"))]
fn persist_port(_port: u16) {}

static TOKEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

const TOKEN_COOKIE: &str = "ksu_webui";
const TOKEN_BYTES: usize = 16;
const TOKEN_MIN_LEN: usize = 4;
const TOKEN_MAX_LEN: usize = 32;

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

fn random_token() -> String {
    let mut buf = [0u8; TOKEN_BYTES];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return to_hex(&buf);
        }
    }

    let mut seed = now_millis() ^ u64::from(std::process::id());
    for chunk in buf.chunks_mut(8) {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    to_hex(&buf)
}

#[cfg(target_os = "android")]
fn persist_token(token: &str) {
    match std::fs::create_dir_all(defs::WORKING_DIR) {
        Err(e) => log::warn!("failed to create {}: {e}", defs::WORKING_DIR),
        Ok(()) => {
            use std::os::unix::fs::OpenOptionsExt;
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(defs::WEBUI_TOKEN_PATH)
                .and_then(|mut f| f.write_all(token.as_bytes()));
            if let Err(e) = written {
                log::warn!("failed to persist webui token: {e}");
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
fn persist_token(_token: &str) {}

pub fn reset_token() -> Result<String> {
    let token = random_token();
    persist_token(&token);
    log::info!("Web UI access token rotated");
    Ok(token)
}

pub fn set_token(token: &str) -> Result<String> {
    let trimmed = token.trim();
    if !is_acceptable_token(trimmed) {
        anyhow::bail!("令牌需要 {TOKEN_MIN_LEN}..={TOKEN_MAX_LEN} 个非空白字符");
    }
    persist_token(trimmed);
    if let Ok(mut slot) = TOKEN.lock() {
        *slot = trimmed.to_string();
    }
    log::info!("Web UI access token set");
    Ok(trimmed.to_string())
}

fn is_acceptable_token(token: &str) -> bool {
    let length = token.chars().count();
    (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&length) && !token.chars().any(char::is_whitespace)
}

#[cfg(target_os = "android")]
fn load_or_create_token() -> String {
    if let Ok(existing) = std::fs::read_to_string(defs::WEBUI_TOKEN_PATH) {
        let trimmed = existing.trim();
        if is_acceptable_token(trimmed) {
            return trimmed.to_string();
        }
    }

    let token = random_token();
    persist_token(&token);
    token
}

#[cfg(not(target_os = "android"))]
fn load_or_create_token() -> String {
    random_token()
}

fn read_token_file(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    is_acceptable_token(trimmed).then(|| trimmed.to_string())
}

fn token_matches(candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }

    let accepted = TOKEN
        .lock()
        .map(|expected| !expected.is_empty() && candidate == expected.as_str())
        .unwrap_or(false);
    if accepted {
        return true;
    }

    #[cfg(target_os = "android")]
    {
        if let Some(fresh) = read_token_file(std::path::Path::new(defs::WEBUI_TOKEN_PATH))
            && candidate == fresh
        {
            log::info!("Web UI token on disk changed; adopting the new one");
            if let Ok(mut slot) = TOKEN.lock() {
                *slot = fresh;
            }
            return true;
        }
    }

    false
}

fn current_token() -> String {
    TOKEN.lock().map(|t| t.clone()).unwrap_or_default()
}

static LISTEN_PORT: AtomicU16 = AtomicU16::new(0);

const TICKET_BYTES: usize = 32;
const TICKET_TTL: Duration = Duration::from_secs(60);

struct Ticket {
    hash: String,
    expires: u64,
    uid: Option<u32>,
}

static TICKETS: std::sync::Mutex<Vec<Ticket>> = std::sync::Mutex::new(Vec::new());

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return to_hex(&buf);
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut seed = nanos ^ u128::from(std::process::id());
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push_str(&to_hex(&[(seed >> 64) as u8]));
    }
    out
}

fn mint_ticket(uid: Option<u32>) -> String {
    let ticket = random_hex(TICKET_BYTES);
    let hash = sha256::digest(ticket.as_bytes());
    let now = now_millis();
    if let Ok(mut list) = TICKETS.lock() {
        list.retain(|t| t.expires > now);
        list.push(Ticket {
            hash,
            expires: now + TICKET_TTL.as_millis() as u64,
            uid,
        });
    }
    ticket
}

fn redeem_ticket(candidate: &str, uid: Option<u32>) -> bool {
    let hash = sha256::digest(candidate.as_bytes());
    let Ok(mut list) = TICKETS.lock() else {
        return false;
    };
    let now = now_millis();
    list.retain(|t| t.expires > now);
    let Some(at) = list.iter().position(|t| t.hash == hash) else {
        return false;
    };
    if let Some(wanted) = list[at].uid
        && uid != Some(wanted)
    {
        return false;
    }
    list.remove(at);
    true
}

#[cfg(unix)]
fn peer_uid(stream: &TcpStream) -> Option<u32> {
    let addr = stream.peer_addr().ok()?;
    if !addr.ip().is_loopback() {
        return None;
    }
    let port = addr.port();
    let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let Some((host, port_hex)) = fields[1].split_once(':') else {
            continue;
        };
        if !host.eq_ignore_ascii_case("0100007F") {
            continue;
        }
        if u16::from_str_radix(port_hex, 16).ok() != Some(port) {
            continue;
        }
        return fields[7].parse::<u32>().ok();
    }
    None
}

#[cfg(not(unix))]
fn peer_uid(_stream: &TcpStream) -> Option<u32> {
    None
}

fn request_token(req: &HttpRequest) -> String {
    if let Some((_, query)) = req.path.split_once('?') {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "k" {
                    return value.to_string();
                }
            }
        }
    }

    if let Some(cookie) = req.headers.get("cookie") {
        for pair in cookie.split(';') {
            if let Some((key, value)) = pair.trim().split_once('=') {
                if key == TOKEN_COOKIE {
                    return value.to_string();
                }
            }
        }
    }

    String::new()
}

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

fn safe_join(dir: &str, name: &str) -> Result<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        bail!("非法文件名：{name}");
    }
    Ok(std::path::Path::new(dir).join(name).display().to_string())
}

fn json_body<T: serde::de::DeserializeOwned>(req: &HttpRequest) -> Result<T> {
    serde_json::from_slice(&req.body).with_context(|| "请求体不是合法 JSON")
}

fn apk_info(req: &HttpRequest) -> Result<serde_json::Value> {
    let archive = query_param(req, "archive").unwrap_or_default();
    let package = if archive.is_empty() {
        crate::webui_files::apk_package(&require_param(req, "path")?)?
    } else {
        crate::webui_files::apk_package_in_archive(&archive, &require_param(req, "entry")?)?
    };
    Ok(serde_json::json!({ "package": package }))
}

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
    sources: Vec<String>,
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
    path: String,
    query: String,
    #[serde(default)]
    depth: usize,
    #[serde(default)]
    limit: usize,
}

#[derive(serde::Deserialize)]
struct FsInstall {
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
    which: String,
    level: String,
}

#[derive(serde::Deserialize)]
struct KeymintKeybox {
    path: String,
}

#[derive(serde::Deserialize)]
struct KeymintKeyboxContent {
    content: String,
}

#[derive(serde::Deserialize)]
struct KeymintRestart {
    what: String,
}

#[derive(serde::Deserialize)]
struct AppsExtract {
    package: String,
    dest: String,
    #[serde(default)]
    label: String,
}

#[derive(serde::Deserialize)]
struct FsArchiveWrite {
    archive: String,
    entry: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct FsArchiveDelete {
    archive: String,
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

#[cfg(target_os = "android")]
fn module_info_json(id: &str) -> Result<serde_json::Value> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        bail!("非法的模块 id：{id}");
    }

    let active = std::path::Path::new(crate::defs::MODULE_DIR).join(id);
    let pending = std::path::Path::new(crate::defs::MODULE_UPDATE_DIR).join(id);
    let active_ok = active.join("module.prop").exists();
    let pending_ok = pending.join("module.prop").exists();
    let is_pending = !active_ok && pending_ok;
    let base = if active_ok {
        active
    } else if pending_ok {
        pending
    } else {
        bail!("模块 {id} 不存在");
    };

    let mut fields = serde_json::Map::new();
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

pub fn bind_webui(explicit: Option<u16>) -> Result<(TcpListener, u16)> {
    if let Some(port) = explicit {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let bound = listener.local_addr()?.port();
        return Ok((listener, bound));
    }

    if let Some(port) = load_persisted_port() {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                log::info!("Web UI reusing persisted port {port}");
                return Ok((listener, port));
            }
            Err(e) => log::warn!("persisted Web UI port {port} unusable ({e}), picking a new one"),
        }
    }

    let span = u32::from(PORT_RANGE_END - PORT_RANGE_START) + 1;
    for _ in 0..PORT_BIND_ATTEMPTS {
        let candidate = PORT_RANGE_START + (u32::from(random_u16()) % span) as u16;
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", candidate)) {
            persist_port(candidate);
            log::info!("Web UI picked new random port {candidate}");
            return Ok((listener, candidate));
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    persist_port(port);
    log::warn!("no free random port in range, OS assigned {port}");
    Ok((listener, port))
}

fn handle_client(mut stream: TcpStream) {
    touch();

    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let peer = peer_uid(&stream);

    match parse_request(&mut stream) {
        Ok(req) => {
            if let Err(e) = handle_request(&mut stream, &req, peer) {
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

fn watch_connection(stream: &mut TcpStream) -> Result<()> {
    if WATCHER.load(Ordering::Relaxed) == WATCHER_NONE {
        return error_response(stream, 404, "Not watching");
    }
    WATCHER.store(WATCHER_CONNECTED, Ordering::Relaxed);
    log::info!("Web UI: launcher connected; serving until it goes away");

    let _ = stream.set_read_timeout(None);
    send_response(stream, 200, "text/plain", b"watching")?;

    let mut buf = [0u8; 256];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(IDLE_POLL_INTERVAL);
            }
            Err(_) => break,
        }
    }
    WATCHER.store(WATCHER_GONE, Ordering::Relaxed);
    log::info!("Web UI: launcher disconnected, shutting down");
    Ok(())
}

pub fn serve(listener: TcpListener, idle_timeout: Duration) -> Result<()> {
    serve_with(listener, idle_timeout, false, None)
}

pub fn serve_watched(listener: TcpListener, idle_timeout: Duration) -> Result<()> {
    serve_with(listener, idle_timeout, true, None)
}

#[cfg(target_os = "android")]
pub fn serve_for_browser(packages: &[String], port: Option<u16>) -> Result<()> {
    if packages.is_empty() {
        anyhow::bail!("--auto-browser 至少要给一个包名");
    }
    let watched = packages.join(",");
    if another_watcher_running() {
        for _ in 0..30 {
            thread::sleep(Duration::from_millis(100));
            if !another_watcher_running() {
                break;
            }
        }
        if another_watcher_running() {
            log::info!("Web UI: another watcher is already running, stepping aside");
            return Ok(());
        }
    }
    remember_watcher();

    let mut reported_waiting = false;
    loop {
        if !any_package_in_use(packages) {
            if !reported_waiting {
                log::info!("Web UI: waiting for {watched}");
                reported_waiting = true;
            }
            thread::sleep(BROWSER_POLL_INTERVAL);
            continue;
        }

        reported_waiting = false;
        let (listener, bound) = match bind_auto_port(port) {
            Ok(bound) => bound,
            Err(e) => {
                log::info!("Web UI: {e}");
                thread::sleep(BROWSER_POLL_INTERVAL);
                continue;
            }
        };
        log::info!("Web UI: {watched} is in use, listening on 127.0.0.1:{bound}");
        LAST_BROWSER_CHECK.store(now_millis(), Ordering::Relaxed);
        serve_with(listener, Duration::ZERO, false, Some(packages))?;
        log::info!("Web UI: {watched} is gone, port released");
    }
}

fn watcher_pid_path() -> String {
    format!("{}/webui.auto.pid", defs::WORKING_DIR.trim_end_matches('/'))
}

fn another_watcher_running() -> bool {
    let Ok(text) = std::fs::read_to_string(watcher_pid_path()) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    cmdline.windows(12).any(|window| window == b"auto-browser")
}

fn remember_watcher() {
    let _ = std::fs::write(watcher_pid_path(), std::process::id().to_string());
}

#[cfg(not(target_os = "android"))]
pub fn serve_for_browser(_packages: &[String], _port: Option<u16>) -> Result<()> {
    anyhow::bail!("--auto-browser 只在 Android 上可用")
}

#[cfg(target_os = "android")]
fn bind_auto_port(explicit: Option<u16>) -> Result<(TcpListener, u16)> {
    let Some(port) = explicit.or_else(load_persisted_port) else {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        persist_port(port);
        log::info!("Web UI: pinned to port {port}");
        return Ok((listener, port));
    };
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("端口 {port} 已被占用（可能已经有一个在跑）：{e}"))?;
    Ok((listener, port))
}

fn serve_with(
    listener: TcpListener,
    idle_timeout: Duration,
    watch: bool,
    browser: Option<&[String]>,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    touch();
    WATCHER.store(
        if watch { WATCHER_WAITING } else { WATCHER_NONE },
        Ordering::Relaxed,
    );

    if let Ok(mut slot) = TOKEN.lock() {
        *slot = load_or_create_token();
    }

    log::info!(
        "KernelSU Web UI listening on http://{}",
        listener.local_addr()?
    );
    LISTEN_PORT.store(listener.local_addr()?.port(), Ordering::Relaxed);

    loop {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut fd = libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let _ = unsafe { libc::poll(&mut fd, 1, IDLE_POLL_INTERVAL.as_millis() as i32) };
        }
        #[cfg(not(unix))]
        thread::sleep(IDLE_POLL_INTERVAL);
        match listener.accept() {
            Ok((stream, _)) => {
                thread::spawn(move || handle_client(stream));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if watch && WATCHER.load(Ordering::Relaxed) == WATCHER_GONE {
                    log::info!("Web UI: launcher is gone, shutting down");
                    break;
                }
                if let Some(packages) = browser {
                    let now = now_millis();
                    let since = now.saturating_sub(LAST_BROWSER_CHECK.load(Ordering::Relaxed));
                    if since >= BROWSER_POLL_INTERVAL.as_millis() as u64 {
                        LAST_BROWSER_CHECK.store(now, Ordering::Relaxed);
                        if !any_package_in_use(packages) {
                            break;
                        }
                    }
                }
                if !(watch && WATCHER.load(Ordering::Relaxed) == WATCHER_CONNECTED) {
                    let idle = idle_for();
                    if !idle_timeout.is_zero() && idle >= idle_timeout {
                        log::info!("Web UI idle for {}s, shutting down", idle.as_secs());
                        break;
                    }
                }
            }
            Err(e) => {
                log::warn!("Connection failed: {e}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    static SERVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serve_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut slot) = TOKEN.lock() {
            slot.clear();
        }
        guard
    }

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

    fn wait_for_token() -> String {
        for _ in 0..100 {
            let token = current_token();
            if !token.is_empty() {
                return token;
            }
            thread::sleep(Duration::from_millis(20));
        }
        String::new()
    }

    #[test]
    #[ignore]
    fn serve_for_browser() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(18765)).expect("bind");
        let handle = thread::spawn(move || serve(listener, Duration::from_secs(900)));
        let token = wait_for_token();
        println!("BROWSER-URL http://127.0.0.1:{port}/?k={token}");
        handle.join().expect("serve");
    }

    #[test]
    fn nothing_is_served_without_the_token() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(0)).expect("bind");
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(1200)));

        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

        let page_unauth = request_status(port, "/");
        let api_unauth = request_status(port, "/api/heartbeat");
        let page_auth = request_status(port, &format!("/?k={token}"));
        let api_auth = request_status(port, &format!("/api/heartbeat?k={token}"));

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

    fn request_full(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ =
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf).into_owned();
        text[..text.len().min(4000)].to_string()
    }

    #[test]
    fn fs_list_answers_over_http() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(0)).expect("bind");
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(2500)));
        let token = wait_for_token();

        let ok = request_full(port, &format!("/api/fs/list?path=.&k={token}"));
        assert!(
            ok.starts_with("HTTP/1.1 200"),
            "listing did not succeed: {ok}"
        );
        assert!(ok.contains("Cargo.toml"), "listing looks empty: {ok}");

        assert!(request_status(port, "/api/fs/list?path=.").starts_with("HTTP/1.1 403"));

        assert!(request_status(port, "/api/heartbeat").starts_with("HTTP/1.1 403"));
        assert!(
            !handle.is_finished(),
            "server died while serving file manager routes"
        );

        let _ = handle.join();
    }

    #[test]
    fn the_page_is_served_without_caching() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(0)).expect("bind");
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
        let token = random_token();
        assert!(token.len() >= TOKEN_MIN_LEN);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_ticket_can_only_be_redeemed_once() {
        let ticket = mint_ticket(None);
        assert!(redeem_ticket(&ticket, None), "first use must work");
        assert!(!redeem_ticket(&ticket, None), "second use must fail");
        assert!(!redeem_ticket("not-a-ticket", None));
    }

    #[test]
    fn a_wrong_uid_neither_redeems_nor_burns_a_ticket() {
        let ticket = mint_ticket(Some(10_000));
        assert!(!redeem_ticket(&ticket, Some(20_000)));
        assert!(!redeem_ticket(&ticket, None));
        assert!(redeem_ticket(&ticket, Some(10_000)));
        assert!(!redeem_ticket(&ticket, Some(10_000)));
    }

    #[test]
    fn reports_the_port_it_actually_bound() {
        let (listener, port) = bind_webui(Some(0)).expect("bind");
        assert_ne!(port, 0);
        assert_eq!(listener.local_addr().expect("addr").port(), port);
    }

    #[test]
    fn requests_reset_the_idle_clock() {
        touch();
        assert!(idle_for() < Duration::from_secs(5));

        thread::sleep(Duration::from_millis(50));
        assert!(idle_for() >= Duration::from_millis(40));

        touch();
        assert!(idle_for() < Duration::from_millis(40));
    }

    #[test]
    fn serve_returns_once_the_idle_timeout_elapses() {
        let _guard = serve_guard();
        let (listener, _port) = bind_webui(Some(0)).expect("bind");
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
        let (listener, port) = bind_webui(Some(0)).expect("bind");
        let handle = thread::spawn(move || serve(listener, Duration::from_millis(800)));

        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

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

        let stop_by = Instant::now() + Duration::from_secs(10);
        while Instant::now() < stop_by && !handle.is_finished() {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            handle.is_finished(),
            "server did not exit after pings stopped"
        );
    }

    #[test]
    fn a_watched_server_lives_as_long_as_the_launcher_holds_it() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(0)).expect("bind");
        let handle = thread::spawn(move || serve_watched(listener, Duration::from_millis(400)));
        let token = wait_for_token();
        assert!(!token.is_empty(), "serve never produced a token");

        let mut guard = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ = guard.write_all(
            format!("GET /api/watch?k={token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes(),
        );
        let mut answer = [0u8; 64];
        let read = guard.read(&mut answer).expect("the guard is answered");
        let head = String::from_utf8_lossy(&answer[..read]).to_string();
        assert!(head.contains("200"), "guard refused: {head}");

        thread::sleep(Duration::from_millis(1200));
        assert!(
            !handle.is_finished(),
            "server shut down while the launcher was still holding it"
        );
        assert!(
            request_status(port, &format!("/api/heartbeat?k={token}")).contains("200"),
            "server stopped answering while the launcher was holding it"
        );

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

    #[test]
    fn a_watched_server_gives_up_when_no_launcher_ever_connects() {
        let _guard = serve_guard();
        let (listener, _port) = bind_webui(Some(0)).expect("bind");
        let handle = thread::spawn(move || serve_watched(listener, Duration::from_millis(300)));

        let give_up = Instant::now() + Duration::from_secs(5);
        while Instant::now() < give_up && !handle.is_finished() {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            handle.is_finished(),
            "a watched server waited forever for a launcher that never came"
        );
    }

    fn request_all(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ =
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn a_stale_address_explains_itself_in_html() {
        let _guard = serve_guard();
        let (listener, port) = bind_webui(Some(0)).expect("bind");
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

    #[test]
    fn a_rotated_token_is_read_from_disk() {
        let dir = std::env::temp_dir().join(format!("ksu-token-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("webui.token");

        let _ = std::fs::remove_file(&path);
        assert!(
            read_token_file(&path).is_none(),
            "missing file returned a token"
        );

        std::fs::write(&path, "ab").expect("write");
        assert!(read_token_file(&path).is_none(), "short token was accepted");

        let token = "a".repeat(TOKEN_MIN_LEN);
        std::fs::write(&path, format!("{token}\n")).expect("write");
        assert_eq!(read_token_file(&path).as_deref(), Some(token.as_str()));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn base64_matches_the_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        let all: Vec<u8> = (0u8..=255).collect();
        assert_eq!(base64_encode(&all).len(), all.len().div_ceil(3) * 4);
    }
}
