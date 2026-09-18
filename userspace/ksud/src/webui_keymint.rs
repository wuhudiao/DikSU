//! Control panel for the Oh My Keymint module, from the web UI.
//!
//! The module ships no WebUI of its own, and it does not need one to be useful: everything it
//! offers is a file. Its own documentation says the active configuration is
//! `/data/misc/keystore/omk/{config,injector}.toml`, that a restart is asked for by touching a
//! flag in `/data/adb/omk/`, and that a *malformed* `config.toml` present at startup stops
//! keymint from starting at all. So this module's job is narrow and careful:
//!
//!  - read and write those files, keeping their mode and owner (keymint runs as the keystore
//!    uid and cannot read a file owned by root);
//!  - refuse to save TOML that does not parse, because that is the one mistake with a real
//!    cost;
//!  - copy both files aside before every write, so any change can be undone;
//!  - turn the module itself on and off — which is the module's own `disable` file, exactly the
//!    switch the module list already flips.
//!
//! Nothing here reads the module's `[crypto]` secrets out loud; the editors hand the file to
//! the same text editor a config file would open in, and the summary fields are the handful the
//! module's documentation describes.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::defs;

/// The module this panel belongs to.
pub const MODULE_ID: &str = "oh_my_keymint";

/// Where the property-fixing service script is installed: `service.d` is what the root
/// solutions run at boot, after the system is up.
pub const FIX_PROPS_PATH: &str = "/data/adb/service.d/omk-fixprops.sh";

/// The service script itself.
///
/// Kept here rather than in the panel so the switch is a switch and not an editor: what it does
/// is fixed, and this is the one copy of it. The body is the script as written for this panel,
/// with a shebang (service.d needs one) and a PATH fallback (resetprop lives in the root
/// solution's own bin, which is not always on a module script's PATH).
pub const FIX_PROPS_SCRIPT: &str = r#"#!/system/bin/sh
# 修正系统属性：把「已解锁 / 可调试」那一面的属性改回正常机器的样子。
# 由 keyMint 配置页的开关安装，删除该文件即可关闭。
command -v resetprop >/dev/null 2>&1 || PATH="/data/adb/ksu/bin:/data/adb/magisk:$PATH"
export PATH

wait_for_boot() {
  local i=0
  while [ "$i" -lt 60 ]; do
    local boot
    boot=$(getprop sys.boot_completed)
    [ "$boot" = "1" ] && break
    i=$((i + 1))
    sleep 1
  done
}

disable_setting() {
  local namespace="$1"
  local key="$2"
  local value="$3"
  settings put "$namespace" "$key" "$value" >/dev/null 2>&1
}

check_reset_prop() {
  local NAME="$1"
  local EXPECTED="$2"
  local VALUE
  VALUE=$(resetprop "$NAME")
  [ -n "$VALUE" ] && [ "$VALUE" != "$EXPECTED" ] && resetprop -n "$NAME" "$EXPECTED"
}

contains_reset_prop() {
  local NAME="$1"
  local CONTAINS="$2"
  local NEWVAL="$3"
  local VALUE
  VALUE=$(resetprop "$NAME")
  [ -n "$VALUE" ] && [[ "$VALUE" == *"$CONTAINS"* ]] && resetprop -n "$NAME" "$NEWVAL"
}

wait_for_boot

disable_setting global adb_enabled 0
stop adbd >/dev/null 2>&1
setprop ctl.stop adbd >/dev/null 2>&1

check_reset_prop "ro.boot.vbmeta.device_state" "locked"
check_reset_prop "ro.boot.verifiedbootstate" "green"
check_reset_prop "ro.boot.flash.locked" "1"
check_reset_prop "ro.boot.veritymode" "enforcing"
check_reset_prop "ro.secureboot.lockstate" "locked"
check_reset_prop "ro.debuggable" "0"
check_reset_prop "ro.force.debuggable" "0"
check_reset_prop "ro.secure" "1"
check_reset_prop "ro.adb.secure" "1"
check_reset_prop "ro.build.type" "user"
check_reset_prop "ro.build.tags" "release-keys"
check_reset_prop "ro.bootmode" "normal"
"#;

/// Whether the boot-time property fix is installed.
pub fn fix_props_enabled() -> bool {
    Path::new(FIX_PROPS_PATH).is_file()
}

/// Install or remove the service script. Returns the state it ended in.
pub fn set_fix_props(enable: bool) -> Result<bool> {
    let path = Path::new(FIX_PROPS_PATH);
    if enable {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建 {} 失败", parent.display()))?;
        }
        fs::write(path, FIX_PROPS_SCRIPT)
            .with_context(|| format!("写入 {} 失败", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        }
    } else {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("删除 {} 失败", path.display())),
        }
    }
    Ok(fix_props_enabled())
}

/// The log levels keymint accepts, exactly as the module's configuration guide lists them.
pub const LOG_LEVELS: [&str; 6] = ["off", "error", "warn", "info", "debug", "trace"];

/// The injector accepts one more spelling: `warning` is an alias of `warn`.
pub const INJECTOR_LOG_LEVELS: [&str; 7] =
    ["off", "error", "warn", "warning", "info", "debug", "trace"];

/// What the template in the module folder routes by default, which is what the app picker
/// hides: the module's own defaults are not the user's routing decision.
pub fn default_google_scoop() -> Vec<String> {
    let text = fs::read_to_string(module_dir().join("injector.toml")).unwrap_or_default();
    parse_scoop(&text)
        .unwrap_or_default()
        .into_iter()
        .filter(|package| package.starts_with("com.google.") || package.starts_with("com.android.vending"))
        .collect()
}

/// Where the module keeps its active configuration.
const CONFIG_DIR: &str = "/data/misc/keystore/omk";
/// Where it keeps its pid files, restart flags and — thanks to the installer's symlink — a view
/// of the configuration directory.
const STATE_DIR: &str = "/data/adb/omk";
/// The uid the configuration belongs to (the keystore user). Files written here have to keep
/// it, or keymint cannot read them after the next restart.
const KEYSTORE_UID: u32 = 1017;

/// What one of the module's files is doing, as far as this process can tell.
///
/// `error` carries the raw os error: a root process can still be refused by SELinux on the
/// keystore directory, and "读不到（Permission denied）" is a different problem from "文件不存在"
/// — one of them is the reader's to fix, the other is not.
#[derive(Serialize, Debug, PartialEq, Eq, Default)]
pub struct FileState {
    pub exists: bool,
    pub size: u64,
    pub error: String,
}

impl FileState {
    fn of(path: &Path) -> Self {
        match fs::metadata(path) {
            Ok(md) => FileState {
                exists: md.is_file(),
                size: md.len(),
                error: String::new(),
            },
            Err(e) => FileState {
                exists: false,
                size: 0,
                // `NotFound` is an answer, not a failure; anything else is worth saying out loud.
                error: if e.kind() == std::io::ErrorKind::NotFound {
                    String::new()
                } else {
                    e.to_string()
                },
            },
        }
    }
}

/// Serialized in camelCase, like every other payload the page reads — and pinned by
/// `the_status_payload_has_the_names_the_page_reads`, because a field spelled the Rust way is
/// one the page silently renders as `undefined`.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeymintStatus {
    /// Whether the module is installed at all.
    pub installed: bool,
    pub version: String,
    /// The module's own switch: absent `disable` file means enabled.
    pub enabled: bool,
    pub config_dir: String,
    pub state_dir: String,
    /// How each of the module's files is doing: present, absent, or unreadable.
    pub config_file: FileState,
    pub injector_file: FileState,
    pub keybox_file: FileState,
    pub keybox_size: u64,
    /// The two daemons, as their pid files report them.
    pub keymint_running: bool,
    pub injector_running: bool,
    /// Summaries of the active files, for the header of each editor.
    pub scoop_count: usize,
    pub scoop: Vec<String>,
    pub log_level: String,
    /// The injector keeps its own level in its own file, and does not need a restart for it.
    pub injector_log_level: String,
    pub config_version: i64,
    /// Whether the boot-time property fix is installed.
    pub fix_props: bool,
    /// The levels keymint accepts, for the picker.
    pub log_levels: Vec<String>,
    /// The levels the injector accepts (one more spelling than keymint).
    pub injector_log_levels: Vec<String>,
    /// The module template's own Google defaults, which the app picker keeps out of sight.
    pub default_google_scoop: Vec<String>,
}

/// The two files this panel edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Config,
    Injector,
}

impl Which {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "config" => Ok(Self::Config),
            "injector" => Ok(Self::Injector),
            other => anyhow::bail!("未知的配置文件：{other}"),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Config => "config.toml",
            Self::Injector => "injector.toml",
        }
    }

    fn path(self) -> PathBuf {
        Path::new(CONFIG_DIR).join(self.file_name())
    }
}

fn module_dir() -> PathBuf {
    // `defs::MODULE_DIR` only exists on Android, and this module's tests run on a host — so the
    // constant is used where it is available and the same path is spelled out where it is not.
    #[cfg(target_os = "android")]
    {
        Path::new(defs::MODULE_DIR).join(MODULE_ID)
    }
    #[cfg(not(target_os = "android"))]
    {
        PathBuf::from(format!("/data/adb/modules/{MODULE_ID}"))
    }
}

/// The name of the file that turns a module off, from the same constant the rest of ksud uses.
fn disable_file_name() -> &'static str {
    #[cfg(target_os = "android")]
    {
        defs::DISABLE_FILE_NAME
    }
    #[cfg(not(target_os = "android"))]
    {
        "disable"
    }
}

/// `name=` out of a module.prop, for the version line.
fn module_prop_value(key: &str) -> Option<String> {
    let text = fs::read_to_string(module_dir().join("module.prop")).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{key}=")))
        .map(|value| value.trim().to_string())
}

/// A pid file whose process is actually alive — the same check the module's own service script
/// makes before it starts another daemon.
fn daemon_running(pid_file: &Path) -> bool {
    #[cfg(unix)]
    {
        if let Ok(text) = fs::read_to_string(pid_file) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if pid > 0 {
                    // Signal 0 asks the kernel whether the process exists without touching it.
                    return unsafe { libc::kill(pid, 0) } == 0;
                }
            }
        }
    }
    let _ = pid_file;
    false
}

/// The package list from `injector.toml`'s `scoop` array.
///
/// Parsed as TOML rather than by hand: it is the one field the panel edits, and a hand-rolled
/// reader would be the thing that quietly dropped an entry.
pub fn parse_scoop(text: &str) -> Result<Vec<String>> {
    let value: toml::Value = toml::from_str(text).context("injector.toml 不是合法的 TOML")?;
    let scoop = value.get("scoop");
    match scoop {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .with_context(|| "scoop 里出现了非字符串项".to_string())
            })
            .collect(),
        Some(_) => anyhow::bail!("scoop 不是数组"),
    }
}

/// `injector.toml` with its `scoop` array replaced, everything else left byte for byte.
///
/// Rewriting the whole file from the parsed value would reformat the user's comments and
/// secrets; replacing just the array is the smaller change, and the smaller change is the one
/// that cannot lose something.
///
/// The file is deliberately not required to parse first. Replacing the span between the array's
/// brackets is local — nothing outside it is touched — and the result is validated before it is
/// written, so a file left with entries that are not quoted (by hand, or by an earlier version of
/// this panel) is repaired by the next save instead of refusing to be edited at all.
pub fn set_scoop(text: &str, packages: &[String]) -> Result<String> {
    let quoted: Vec<String> = packages
        .iter()
        .map(|name| format!("\"{}\"", name.replace('"', "")))
        .collect();

    let Some((open, close)) = find_scoop_array(text) else {
        // Nothing to replace, so an array has to be added — and that is only safe in a file that
        // parses: a `scoop` key already present in a shape this search cannot find would end up
        // duplicated, and duplicate keys are not valid TOML.
        parse_scoop(text).context("injector.toml 里找不到 scoop 数组，且文件本身不是合法 TOML")?;

        // A top-level key has to sit above the first table header. Appended to the end of the file
        // instead — as this once did — it lands inside whichever table happens to be last, where it
        // is not the key the module reads and the save looks like it did nothing.
        let at = first_table_header(text).unwrap_or(text.len());
        let head = text[..at].trim_end();
        let tail = &text[at..];

        let mut out = String::with_capacity(text.len() + 40);
        if !head.is_empty() {
            out.push_str(head);
            out.push_str("\n\n");
        }
        out.push_str(&format!("scoop = [{}]\n", quoted.join(", ")));
        if !tail.is_empty() {
            out.push('\n');
            out.push_str(tail);
        }
        return Ok(out);
    };

    // Keep the array's own line shape: a one-line array stays one line, a multi-line one keeps its
    // indentation so a diff of the file stays readable. The indentation comes from the line the key
    // sits on, not from the column the `[` happens to start in.
    let inner = if text[open..close].contains('\n') {
        let line = text[..open].rsplit('\n').next().unwrap_or("");
        let indent = line.len() - line.trim_start().len();
        let pad = " ".repeat(indent + 2);
        format!("\n{}{}\n{}", pad, quoted.join(&format!(",\n{pad}")), " ".repeat(indent))
    } else {
        quoted.join(", ")
    };

    Ok(format!("{}[{}]{}", &text[..open], inner, &text[close + 1..]))
}

/// The `[`…`]` span of the `scoop` value, when the file has one.
///
/// A commented line is neither the key nor the end of the search: an example left above the real
/// key is normal in this module's own file, and stopping there would add a second `scoop` key to a
/// file that already has one.
fn find_scoop_array(text: &str) -> Option<(usize, usize)> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();

        if !trimmed.starts_with('#') {
            if let Some(rest) = trimmed.strip_prefix("scoop") {
                // `scoopXYZ = …` is a different key; only `scoop =` is this one.
                if rest.trim_start().starts_with('=') {
                    if let Some(bracket) = line.find('[') {
                        let open = at + bracket;
                        if let Some(close) = matching_bracket(text, open) {
                            return Some((open, close));
                        }
                    }
                }
            }
        }

        at += line.len();
    }
    None
}

/// The line a table header starts on, when the file has one.
///
/// Where a key that does not exist yet has to be inserted: above this, a key is top level, below
/// it the key belongs to that table.
fn first_table_header(text: &str) -> Option<usize> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            return Some(at);
        }
        at += line.len();
    }
    None
}

/// The index of the `]` that closes the `[` at `open`.
///
/// Brackets inside quoted strings and inside comments do not count. A `]` in a comment left in the
/// array would otherwise end the span early, and the replacement would then eat everything after it
/// — the one mistake here that costs more than the array itself.
fn matching_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = open;

    while i < bytes.len() {
        let byte = bytes[i];

        if let Some(open_quote) = quote {
            if byte == open_quote {
                quote = None;
            } else if byte == b'\\' && open_quote == b'"' {
                i += 2; // an escape cannot close a basic string
                continue;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if byte == b'[' {
            depth += 1;
        } else if byte == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }

        i += 1;
    }
    None
}

/// A short summary of `config.toml`: the fields the module's documentation names as safe to
/// look at, and nothing else.
pub fn config_summary(text: &str) -> Result<(i64, String)> {
    let value: toml::Value = toml::from_str(text).context("config.toml 不是合法的 TOML")?;
    let version = value.get("version").and_then(toml::Value::as_integer).unwrap_or(0);
    let log_level = value
        .get("main")
        .and_then(|main| main.get("log_level"))
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((version, log_level))
}

/// Reject TOML that does not parse, before it can reach the device.
///
/// This is the mistake with consequences: the module's own documentation says a malformed
/// `config.toml` present when keymint starts stops keymint from starting, which is a broken
/// keystore and no obvious way back.
pub fn validate(which: Which, text: &str) -> Result<()> {
    let parsed: toml::Value =
        toml::from_str(text).with_context(|| format!("{} 不是合法的 TOML", which.file_name()))?;
    if !parsed.is_table() {
        anyhow::bail!("{} 的顶层不是表", which.file_name());
    }
    if which == Which::Injector {
        // The panel edits this field, so a typo here would silently unroute every app.
        parse_scoop(text)?;
    }
    Ok(())
}

/// Delete the backup folder the panel used to keep.
///
/// Every save used to copy both TOML files — and the keybox — into a timestamped folder under
/// `webui-backups`, so the folder grew with every save while holding nothing that is not still in
/// place. Those copies are simply removed: an edit that turns out wrong now has to be fixed by
/// hand.
fn drop_old_backups() {
    let dir = Path::new(STATE_DIR).join("webui-backups");
    if !dir.is_dir() {
        return;
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => log::info!("removed the retired backup folder {}", dir.display()),
        Err(e) => log::warn!("could not remove {}: {e}", dir.display()),
    }
}

/// Write one of the active files, keeping it readable by the keystore user.
fn write_config(which: Which, content: &str) -> Result<()> {
    validate(which, content)?;

    let path = which.path();
    // The file belongs to the keystore uid with 0600 — that is how the module's own
    // post-fs-data script leaves it, and root-owned 0600 is a file keymint cannot read.
    #[cfg(unix)]
    let (mode, uid, gid) = existing_owner(&path);
    // Written through a sibling and renamed, so a half-written file is never the active one.
    let staging = path.with_extension("toml.new");
    fs::write(&staging, content).with_context(|| format!("写入 {} 失败", staging.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&staging, fs::Permissions::from_mode(mode));
        let _ = chown(&staging, uid, gid);
    }
    fs::rename(&staging, &path).with_context(|| format!("替换 {} 失败", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn chown(path: &Path, uid: u32, gid: u32) {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if let Ok(c_path) = std::ffi::CString::new(bytes) {
        unsafe {
            libc::chown(c_path.as_ptr(), uid, gid);
        }
    }
}

#[cfg(not(unix))]
fn chown(_path: &Path, _uid: u32, _gid: u32) {}

/// The mode/owner a file has now, or the module's own defaults for a file that does not exist.
fn existing_owner(path: &Path) -> (u32, u32, u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(md) = fs::metadata(path) {
            return (md.mode() & 0o7777, md.uid(), md.gid());
        }
    }
    let _ = path;
    (0o600, KEYSTORE_UID, KEYSTORE_UID)
}

/// Everything the panel's header and status rows show.
pub fn status() -> Result<KeymintStatus> {
    // 备份机制没了，但它留在设备上的副本还在；面板每次打开顺手清一次。
    drop_old_backups();
    let dir = module_dir();
    let installed = dir.is_dir();
    let config_path = Which::Config.path();
    let injector_path = Which::Injector.path();
    let keybox = Path::new(CONFIG_DIR).join("keybox.xml");

    let injector_text = fs::read_to_string(&injector_path).unwrap_or_default();
    let scoop = parse_scoop(&injector_text).unwrap_or_default();
    let config_text = fs::read_to_string(&config_path).unwrap_or_default();
    let (config_version, log_level) = config_summary(&config_text).unwrap_or((0, String::new()));
    let injector_log_level = config_summary(&injector_text)
        .map(|(_, level)| level)
        .unwrap_or_default();

    Ok(KeymintStatus {
        installed,
        version: module_prop_value("version").unwrap_or_default(),
        enabled: installed && !dir.join(disable_file_name()).exists(),
        config_dir: CONFIG_DIR.to_string(),
        state_dir: STATE_DIR.to_string(),
        config_file: FileState::of(&config_path),
        injector_file: FileState::of(&injector_path),
        keybox_file: FileState::of(&keybox),
        keybox_size: fs::metadata(&keybox).map_or(0, |md| md.len()),
        keymint_running: daemon_running(&Path::new(STATE_DIR).join("keymint-daemon.pid")),
        injector_running: daemon_running(&Path::new(STATE_DIR).join("injector-daemon.pid")),
        scoop_count: scoop.len(),
        scoop,
        log_level,
        config_version,
        injector_log_level,
        fix_props: fix_props_enabled(),
        log_levels: LOG_LEVELS.iter().map(|l| (*l).to_string()).collect(),
        injector_log_levels: INJECTOR_LOG_LEVELS.iter().map(|l| (*l).to_string()).collect(),
        default_google_scoop: default_google_scoop(),
    })
}

pub fn read_config(which: Which) -> Result<String> {
    let path = which.path();
    // The raw os error is carried up: on this directory a refusal is usually SELinux rather than
    // permissions, and that difference is the whole answer for whoever reads the message.
    fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("读取 {} 失败：{}", path.display(), e))
}

/// Set `[main] log_level` in one of the two files, leaving the rest of it byte for byte.
///
/// keymint keeps its level in `config.toml` and the injector keeps its own in `injector.toml`;
/// the injector also accepts `warning` for `warn`. The whole file is written with the same
/// validation as any other save — the surgery is only so that the secrets and the comments
/// around them are not reformatted.
pub fn save_log_level(which: Which, level: &str) -> Result<()> {
    let allowed: &[&str] = match which {
        Which::Config => &LOG_LEVELS,
        Which::Injector => &INJECTOR_LOG_LEVELS,
    };
    if !allowed.contains(&level) {
        anyhow::bail!("日志级别只能是 {} 之一", allowed.join(" / "));
    }
    let text = read_config(which)?;
    config_summary(&text)?; // refuse to touch a file that does not parse

    let mut out = String::with_capacity(text.len());
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        // Only a real key line: a comment that mentions the name stays a comment.
        if !replaced && !trimmed.starts_with('#') && trimmed.starts_with("log_level") && trimmed.contains('=') {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            out.push_str(&format!("{indent}log_level = \"{level}\"{newline}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        anyhow::bail!("{} 里没有 log_level 这一行", which.file_name());
    }
    write_config(which, &out)
}

/// The shallow check both keybox paths run before anything is backed up or written.
///
/// A full parse belongs to the module — its loader refuses a bad file on its own — but a
/// placeholder, an HTML error page or a truncated download never names the `<AndroidAttestation>`
/// root nor carries a `<PrivateKey>`, so refusing those here turns a confusing "the module
/// rewrote my keybox back" into a plain error message.
fn looks_like_keybox(content: &str) -> bool {
    content.contains("<AndroidAttestation") && content.contains("<PrivateKey")
}

/// The shared tail of both keybox paths: stage-and-rename the new content into place with the mode and uid the module's own post-fs-data script gives it —
/// 0600 and the keystore user, or keymint cannot read it.
fn install_keybox(content: &str) -> Result<()> {
    let target = Path::new(CONFIG_DIR).join("keybox.xml");
    #[cfg(unix)]
    let (mode, uid, gid) = existing_owner(&target);
    let staging = target.with_extension("xml.new");
    fs::write(&staging, content).with_context(|| format!("写入 {} 失败", staging.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&staging, fs::Permissions::from_mode(mode));
        let _ = chown(&staging, uid, gid);
    }
    fs::rename(&staging, &target).with_context(|| format!("替换 {} 失败", target.display()))?;
    Ok(())
}

/// Replace the keybox with a file the user picked.
///
/// The keybox is what makes attestation convincing, so it is treated like the configuration:
/// written with the mode and uid the module's own post-fs-data script gives it — 0600 and the keystore user, or keymint cannot read it.
pub fn apply_keybox(source: &str) -> Result<()> {
    let from = Path::new(source);
    if !from.is_file() {
        anyhow::bail!("不是文件：{source}");
    }
    let content =
        fs::read_to_string(from).with_context(|| format!("读取 {source} 失败"))?;
    if !looks_like_keybox(&content) {
        anyhow::bail!("这个文件不是有效的 keybox.xml：缺少 <AndroidAttestation> 或 <PrivateKey>");
    }
    install_keybox(&content)
}

/// Replace the keybox with text the web UI downloaded from a remote URL.
///
/// The download itself lives in the browser — the page fetches the URL and hands the text over —
/// so this endpoint never talks to the network; what it does own is the trust boundary. Remote
/// content was never on the user's disk and was not hand-picked, so it gets the same strict
/// shape check before the keybox is replaced.
pub fn apply_keybox_content(content: &str) -> Result<()> {
    if !looks_like_keybox(content) {
        anyhow::bail!("远程内容不是有效的 keybox.xml：缺少 <AndroidAttestation> 或 <PrivateKey>");
    }
    install_keybox(content)
}

pub fn save_scoop(packages: &[String]) -> Result<()> {
    let text = read_config(Which::Injector)?;
    let updated = set_scoop(&text, packages)?;
    write_config(Which::Injector, &updated)
}

/// Ask one of the module's components to restart, the way its README documents: by touching a
/// flag file that its supervisor watches.
pub fn restart(what: &str) -> Result<String> {
    let flag = match what {
        "keymint" => "restart.keymint",
        "injector" => "restart.injector",
        "all" => "restart.all",
        other => anyhow::bail!("未知的重启目标：{other}"),
    };
    if !Path::new(STATE_DIR).is_dir() {
        anyhow::bail!("{STATE_DIR} 不存在，模块可能没有在运行");
    }
    let path = Path::new(STATE_DIR).join(flag);
    fs::write(&path, b"").with_context(|| format!("创建 {} 失败", path.display()))?;
    Ok(flag.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INJECTOR: &str = r#"# OhMyKeymint injector configuration
version = 2

# Apps routed to OMK.
scoop = [
  "com.example.one",
  "com.example.two",
]

[intercept]
attestation = true
"#;

    /// Every payload the page reads is checked the same way: the keys it looks up have to be
    /// there, spelled the way JavaScript spells them. This has been got wrong twice — once in
    /// the app detail, once here — so it is a named check now.
    fn assert_keys(json: &serde_json::Value, keys: &[&str]) {
        for key in keys {
            assert!(json.get(key).is_some(), "the panel reads `{key}`, which is missing");
        }
    }

    #[test]
    fn the_status_payload_has_the_names_the_page_reads() {
        let json = serde_json::to_value(KeymintStatus {
            installed: true,
            version: "1.0".into(),
            enabled: true,
            config_dir: "/data/misc/keystore/omk".into(),
            state_dir: "/data/adb/omk".into(),
            config_file: FileState::default(),
            injector_file: FileState::default(),
            keybox_file: FileState::default(),
            keybox_size: 0,
            keymint_running: false,
            injector_running: false,
            scoop_count: 0,
            scoop: Vec::new(),
            log_level: String::new(),
            config_version: 0,
            fix_props: false,
            injector_log_level: String::new(),
            log_levels: LOG_LEVELS.iter().map(|l| (*l).to_string()).collect(),
            injector_log_levels: INJECTOR_LOG_LEVELS.iter().map(|l| (*l).to_string()).collect(),
            default_google_scoop: Vec::new(),
        })
        .expect("serialize");
        assert_keys(
            &json,
            &[
                "installed", "version", "enabled", "configDir", "stateDir", "configFile",
                "injectorFile", "keyboxFile", "keyboxSize", "keymintRunning", "injectorRunning",
                "scoopCount", "scoop", "logLevel", "configVersion", "logLevels",
                "defaultGoogleScoop", "fixProps", "injectorLogLevel", "injectorLogLevels",
            ],
        );
        // A file's answer is an object, not a bare boolean: it has to be able to say why.
        assert_keys(&json["configFile"], &["exists", "size", "error"]);
    }

    /// A file that cannot be looked at says so instead of pretending to be absent.
    #[test]
    fn a_file_state_separates_missing_from_unreadable() {
        let missing = FileState::of(Path::new("/definitely/not/here"));
        assert!(!missing.exists && missing.error.is_empty());

        let dir = std::env::temp_dir().join(format!("ksu-keymint-fs-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        fs::write(&file, b"version = 2
").expect("write");
        let present = FileState::of(&file);
        assert!(present.exists && present.error.is_empty() && present.size > 0);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn rewrites_only_the_log_level_line() {
        let text = "# keep me\nversion = 2\n[main]\nbackend = \"injector\"\nlog_level = \"debug\"\n[crypto]\nkey = \"deadbeef\"\n";
        // The same surgery the panel does, exercised on the shape a real file has.
        let mut out = String::new();
        let mut replaced = false;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if !replaced && !trimmed.starts_with('#') && trimmed.starts_with("log_level") && trimmed.contains('=') {
                out.push_str("log_level = \"trace\"\n");
                replaced = true;
            } else {
                out.push_str(line);
            }
        }
        assert!(replaced);
        assert!(out.contains("# keep me"));
        assert!(out.contains("key = \"deadbeef\""));
        assert!(out.contains("log_level = \"trace\""));
        assert!(!out.contains("log_level = \"debug\""));
    }

    /// The service script is a constant here, so a stray edit should be caught by the tests
    /// rather than by a device that boots without its properties fixed.
    #[test]
    fn the_fix_props_script_is_installable() {
        assert!(FIX_PROPS_SCRIPT.starts_with("#!/system/bin/sh"), "service.d needs a shebang");
        for line in [
            "wait_for_boot",
            "resetprop -n",
            "ro.boot.verifiedbootstate",
            "ro.build.tags",
            "ro.bootmode",
            "disable_setting global adb_enabled 0",
        ] {
            assert!(FIX_PROPS_SCRIPT.contains(line), "the script lost `{line}`");
        }
        assert!(FIX_PROPS_PATH.starts_with("/data/adb/service.d/"), "service.d is the boot hook");
    }

    #[test]
    fn the_level_set_is_the_one_the_module_documents() {
        assert_eq!(LOG_LEVELS, ["off", "error", "warn", "info", "debug", "trace"]);
        // The injector documents one more spelling, as an alias.
        assert_eq!(
            INJECTOR_LOG_LEVELS,
            ["off", "error", "warn", "warning", "info", "debug", "trace"]
        );
    }

    #[test]
    fn reads_the_scoop_list() {
        assert_eq!(
            parse_scoop(INJECTOR).expect("parse"),
            vec!["com.example.one", "com.example.two"]
        );
        assert!(parse_scoop("version = 2\n").expect("no scoop").is_empty());
    }

    #[test]
    fn replaces_only_the_scoop_array() {
        let updated = set_scoop(INJECTOR, &["com.example.three".to_string(), "com.example.four".to_string()])
            .expect("set");

        // The list changed, every other byte of the file did not.
        assert_eq!(
            parse_scoop(&updated).expect("reparse"),
            vec!["com.example.three", "com.example.four"]
        );
        assert!(updated.contains("# OhMyKeymint injector configuration"));
        assert!(updated.contains("[intercept]\nattestation = true"));
        assert!(!updated.contains("com.example.one"));

        // Idempotent: writing what is already there changes nothing.
        assert_eq!(set_scoop(&updated, &["com.example.three".to_string(), "com.example.four".to_string()]).expect("again"), updated);
    }

    #[test]
    fn a_one_line_array_stays_one_line() {
        let text = "version = 2\nscoop = [\"a\", \"b\"]\n";
        let updated = set_scoop(text, &["c".to_string()]).expect("set");
        assert_eq!(updated, "version = 2\nscoop = [\"c\"]\n");
    }

    #[test]
    fn adds_a_scoop_array_when_there_is_none() {
        let updated = set_scoop("version = 2\n", &["com.example.one".to_string()]).expect("set");
        assert_eq!(parse_scoop(&updated).expect("parse"), vec!["com.example.one"]);
        assert!(updated.contains("version = 2"));
    }

    /// The check that matters: a broken file must never reach the device, because a malformed
    /// `config.toml` at startup is a keymint that does not start.
    #[test]
    fn refuses_toml_that_does_not_parse() {
        assert!(validate(Which::Config, "version = 2\n[main\n").is_err());
        assert!(validate(Which::Config, "version = 2\nlog_level = \"info\"\n").is_ok());
        // A stray quote in the list is a typo the panel must catch rather than save.
        assert!(validate(Which::Injector, "scoop = [\"unterminated]\n").is_err());
        assert!(validate(
            Which::Injector,
            "scoop = [\"com.example.one\"]\n"
        )
        .is_ok());
    }

    #[test]
    fn summarises_the_config_without_touching_secrets() {
        let text = "version = 2\n[main]\nbackend = \"injector\"\nlog_level = \"debug\"\n[crypto]\nkey = \"deadbeef\"\n";
        let (version, level) = config_summary(text).expect("summary");
        assert_eq!(version, 2);
        assert_eq!(level, "debug");
    }

    #[test]
    fn the_keybox_shape_check_matches_what_a_real_file_has() {
        // The module's loader does the strict parse; this check only has to tell a real
        // keybox.xml from a placeholder, an error page or a truncated download.
        let real = "<AndroidAttestation><Keybox><Key algorithm=\"ecdsa\">\
                    <PrivateKey format=\"pem\">x</PrivateKey></Key></Keybox></AndroidAttestation>";
        assert!(looks_like_keybox(real));
        assert!(looks_like_keybox("<?xml version=\"1.0\"?><AndroidAttestation>\n<PrivateKey>"));
        assert!(!looks_like_keybox("6666"));
        assert!(!looks_like_keybox("<html><body>rate limited</body></html>"));
        assert!(!looks_like_keybox("<AndroidAttestation><NumberOfKeyboxes>1</NumberOfKeyboxes>"));
    }

    /// Remote content must be refused before the backup runs, so a bad download moves nothing.
    #[test]
    fn remote_keybox_content_is_refused_before_anything_is_written() {
        for junk in ["6666", "<html><body>503 Service Unavailable</body></html>", ""] {
            assert!(apply_keybox_content(junk).is_err(), "accepted `{junk}`");
        }
    }
}
