//! Filesystem operations behind the web UI's file manager.
//!
//! Everything here runs with whatever privileges ksud has. The launcher starts it through
//! `su`, so that is uid 0 and paths like `/data/adb` are reachable — but root is not a
//! licence to ignore SELinux, which can still refuse a path. Errors therefore keep the raw
//! errno (callers format them with `{:#}` so the whole anyhow chain reaches the UI) instead
//! of being flattened into a vague "operation failed".

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// Text files larger than this are refused by the editor.
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// Files we will inline for preview.
pub const MAX_RAW_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize)]
pub struct RootInfo {
    pub uid: u32,
    pub euid: u32,
    pub is_root: bool,
    /// SELinux context of this process, e.g. `u:r:kernelsu:s0`.
    pub selinux: String,
    pub roots: Vec<Root>,
}

#[derive(Serialize)]
pub struct Root {
    pub label: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub size: u64,
    /// Permission bits only, e.g. 0o777.
    pub mode: u32,
    /// Human-readable form of [`Entry::mode`], e.g. `rwxr-xr-x`.
    pub mode_text: String,
    pub uid: u32,
    pub gid: u32,
    pub mtime: i64,
}

fn io_context(op: &str, path: &Path) -> String {
    format!("{op} {} 失败", path.display())
}

#[cfg(unix)]
fn current_ids() -> (u32, u32) {
    unsafe { (libc::getuid(), libc::geteuid()) }
}

#[cfg(not(unix))]
fn current_ids() -> (u32, u32) {
    // No uid concept here; report a non-root sentinel rather than pretending to be root.
    (u32::MAX, u32::MAX)
}

#[cfg(unix)]
fn mode_of(md: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    md.mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(_md: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn uid_of(md: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    md.uid()
}

#[cfg(not(unix))]
fn uid_of(_md: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn gid_of(md: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    md.gid()
}

#[cfg(not(unix))]
fn gid_of(_md: &fs::Metadata) -> u32 {
    0
}

fn mtime_of(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// `0o755` -> `rwxr-xr-x`.
fn mode_text(mode: u32) -> String {
    let mut out = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        out.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        out.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        out.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    out
}

/// Parse the octal permissions the user typed: `777`, `0755`, `0o644`.
pub fn parse_mode(text: &str) -> Result<u32> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix("0o")
        .or_else(|| trimmed.strip_prefix("0O"))
        .unwrap_or(trimmed);

    if digits.is_empty() || digits.len() > 4 {
        bail!("权限应为 3~4 位八进制数字，例如 777 或 0755");
    }

    let mut value = 0u32;
    for ch in digits.chars() {
        let digit = ch
            .to_digit(8)
            .with_context(|| format!("'{text}' 不是合法的八进制权限"))?;
        value = value * 8 + digit;
    }

    if value > 0o7777 {
        bail!("权限超出范围：最大 7777");
    }
    Ok(value)
}

fn read_selinux_context() -> String {
    fs::read_to_string("/proc/self/attr/current")
        .map(|s| s.trim_end_matches('\0').trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Identity of the process plus a few convenient starting points.
pub fn root_info() -> RootInfo {
    let (uid, euid) = current_ids();
    RootInfo {
        uid,
        euid,
        is_root: euid == 0,
        selinux: read_selinux_context(),
        roots: default_roots(),
    }
}

/// The two starting points the file manager offers. Anything else is added by the user and
/// kept in the browser, so no extra config file has to live on the device.
fn default_roots() -> Vec<Root> {
    // `label`, `path`, and what has to exist for the entry to be worth offering: the APK
    // extract folder is created by the first extraction, so it is offered from the moment
    // `/data/adb` is there — otherwise the panel would write somewhere nothing could jump to.
    const CANDIDATES: [(&str, &str, &str); 2] = [
        ("内部存储", "/sdcard", "/sdcard"),
        ("APK提取路径", "/data/adb/app", "/data/adb"),
    ];

    CANDIDATES
        .iter()
        .filter(|(_, _, probe)| Path::new(probe).exists())
        .map(|(label, path, _)| Root {
            label: (*label).to_string(),
            path: (*path).to_string(),
        })
        .collect()
}

/// Refuse to touch anything unless the process really is root.
///
/// Without this a non-root ksud would silently return half-empty listings and confusing
/// permission errors; failing loudly says exactly what is wrong.
pub fn require_root() -> Result<()> {
    let (_, euid) = current_ids();
    if euid != 0 {
    }
    Ok(())
}

fn entry_for(path: &Path) -> Result<Entry> {
    let link_md = fs::symlink_metadata(path).with_context(|| io_context("读取属性", path))?;
    let is_symlink = link_md.file_type().is_symlink();
    let symlink_target = if is_symlink {
        fs::read_link(path).ok().map(|p| p.display().to_string())
    } else {
        None
    };
    // Follow the link for the type and size the user cares about; a broken link falls back
    // to what lstat reported.
    let md = fs::metadata(path).unwrap_or(link_md);

    Ok(Entry {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        path: path.display().to_string(),
        is_dir: md.is_dir(),
        is_symlink,
        symlink_target,
        size: md.len(),
        mode: mode_of(&md),
        mode_text: mode_text(mode_of(&md)),
        uid: uid_of(&md),
        gid: gid_of(&md),
        mtime: mtime_of(&md),
    })
}

/// Directory contents, directories first then case-insensitive by name.
///
/// Unreadable entries are skipped with a warning rather than failing the whole listing —
/// `/proc`, `/sys` and friends are full of those and a file manager should still show them.
pub fn list_dir(path: &str) -> Result<Vec<Entry>> {
    let dir = Path::new(path);
    let read = fs::read_dir(dir).with_context(|| io_context("读取目录", dir))?;

    let mut entries = Vec::new();
    for item in read {
        match item {
            Ok(item) => entries.push(entry_for(&item.path())?),
            Err(e) => log::warn!("skipping unreadable entry under {path}: {e}"),
        }
    }

    // Directories first, then files; newest first inside each group.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub fn stat(path: &str) -> Result<Entry> {
    entry_for(Path::new(path))
}

/// Names under `root` containing `query`, case-insensitively — the file manager's search.
///
/// Two things make a phone search different from a desktop one, and both are the caller's to
/// decide: how deep to walk and how many answers to keep. `/sdcard` is large enough that an
/// unbounded walk is a hang rather than a search, so the caller passes limits and this obeys them
/// (the page sends its own, the route caps them again). Symlinked directories are listed but not
/// descended into: a link back up the tree would otherwise be walked forever.
pub fn search(root: &str, query: &str, max_depth: usize, max_results: usize) -> Result<Vec<Entry>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        bail!("请输入要搜索的名字");
    }
    let start = Path::new(root);
    if !start.is_dir() {
        bail!("只会在目录里搜索：{}", start.display());
    }

    let mut found: Vec<Entry> = Vec::new();
    // Depth-first with an explicit stack, so a directory deeper than the call stack cannot crash it.
    let mut stack = vec![(start.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let read = match fs::read_dir(&dir) {
            Ok(read) => read,
            // An unreadable directory is skipped rather than failing the whole search, the same way
            // a listing skips it: /proc and /sys are full of those.
            Err(e) => {
                log::warn!("search: skipping {}: {e}", dir.display());
                continue;
            }
        };
        for item in read.flatten() {
            let path = item.path();
            if item.file_name().to_string_lossy().to_lowercase().contains(&needle) {
                match entry_for(&path) {
                    Ok(entry) => found.push(entry),
                    Err(e) => log::warn!("search: skipping {}: {e}", path.display()),
                }
            }
            if found.len() >= max_results {
                break;
            }
            if depth < max_depth {
                // `symlink_metadata` so a link to a directory is not followed.
                let is_dir = fs::symlink_metadata(&path)
                    .map(|md| md.is_dir() && !md.file_type().is_symlink())
                    .unwrap_or(false);
                if is_dir {
                    stack.push((path, depth + 1));
                }
            }
        }
        if found.len() >= max_results {
            break;
        }
    }

    // Directories first, then newest first — the order a listing reads in, so the same name in two
    // places is easy to tell apart.
    found.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

pub fn create_dir(path: &str) -> Result<()> {
    let dir = Path::new(path);
    if dir.exists() {
        bail!("已存在：{}", dir.display());
    }
    fs::create_dir_all(dir).with_context(|| io_context("新建目录", dir))
}

pub fn rename(from: &str, to: &str) -> Result<()> {
    let src = Path::new(from);
    let dst = Path::new(to);
    if dst.exists() {
        bail!("目标已存在：{}", dst.display());
    }
    fs::rename(src, dst)
        .with_context(|| format!("重命名 {} -> {} 失败", src.display(), dst.display()))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::other("symlinks unsupported on this platform"))
}

fn copy_tree(from: &Path, to: &Path, overwrite: bool) -> Result<()> {
    let md = fs::symlink_metadata(from)?;

    if md.file_type().is_symlink() {
        // Recreate the link rather than copying whatever it happens to point at.
        let target = fs::read_link(from)?;
        if to.exists() {
            if !overwrite {
                bail!("目标已存在：{}", to.display());
            }
            let _ = fs::remove_file(to);
        }
        return symlink(&target, to).with_context(|| io_context("复制软链接", from));
    }

    if md.is_dir() {
        if !to.exists() {
            fs::create_dir_all(to)?;
        }
        for item in fs::read_dir(from)? {
            let item = item?;
            copy_tree(&item.path(), &to.join(item.file_name()), overwrite)?;
        }
        return Ok(());
    }

    if to.exists() && !overwrite {
        bail!("目标已存在：{}", to.display());
    }
    fs::copy(from, to)?;
    Ok(())
}

/// Validate one source/destination pair, returning the path it will land on.
fn plan_move(from: &str, dest_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let src = PathBuf::from(from);
    let name = src
        .file_name()
        .with_context(|| format!("无法确定文件名：{from}"))?;
    let dst = dest_dir.join(name);

    if dst == src {
        bail!("源和目标是同一个位置：{from}");
    }
    if src.is_dir() && dst.starts_with(&src) {
        bail!("不能把目录放进它自己的子目录：{from}");
    }
    Ok((src, dst))
}

pub fn copy(sources: &[String], dest_dir: &str, overwrite: bool) -> Result<Vec<String>> {
    let dest = Path::new(dest_dir);
    if !dest.is_dir() {
        bail!("目标不是目录：{dest_dir}");
    }

    let mut done = Vec::new();
    for source in sources {
        let (src, dst) = plan_move(source, dest)?;
        if dst.exists() && !overwrite {
            bail!("目标已存在：{}", dst.display());
        }
        copy_tree(&src, &dst, overwrite).with_context(|| io_context("复制", &src))?;
        done.push(dst.display().to_string());
    }
    Ok(done)
}

#[cfg(unix)]
fn is_cross_device(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(not(unix))]
fn is_cross_device(_e: &io::Error) -> bool {
    // Without errno to check, assume the copy fallback is worth trying.
    true
}

pub fn move_paths(sources: &[String], dest_dir: &str, overwrite: bool) -> Result<Vec<String>> {
    let dest = Path::new(dest_dir);
    if !dest.is_dir() {
        bail!("目标不是目录：{dest_dir}");
    }

    let mut done = Vec::new();
    for source in sources {
        let (src, dst) = plan_move(source, dest)?;
        if dst.exists() {
            if !overwrite {
                bail!("目标已存在：{}", dst.display());
            }
            // Cleared first: rename cannot land on an existing directory, and replacing a
            // file with a directory (or the other way round) needs the old one gone.
            remove_one(&dst, true).with_context(|| io_context("替换", &dst))?;
        }

        match fs::rename(&src, &dst) {
            Ok(()) => {}
            Err(e) if is_cross_device(&e) => {
                // Different filesystems: rename cannot cross them, so copy then delete.
                copy_tree(&src, &dst, overwrite)
                    .with_context(|| io_context("移动（跨分区复制）", &src))?;
                remove_one(&src, true).with_context(|| io_context("移动（删除源）", &src))?;
            }
            Err(e) => {
                return Err(e).with_context(|| io_context("移动", &src));
            }
        }
        done.push(dst.display().to_string());
    }
    Ok(done)
}

fn remove_one(path: &Path, recursive: bool) -> Result<()> {
    let md = fs::symlink_metadata(path)?;
    if md.is_dir() && !md.file_type().is_symlink() {
        if recursive {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_dir(path)?;
        }
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn remove(paths: &[String], recursive: bool) -> Result<()> {
    for path in paths {
        remove_one(Path::new(path), recursive)
            .with_context(|| io_context("删除", Path::new(path)))?;
    }
    Ok(())
}

fn for_each_in_tree<F>(path: &Path, recursive: bool, apply: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    apply(path)?;
    if recursive && path.is_dir() {
        for item in fs::read_dir(path)? {
            let item = item?;
            for_each_in_tree(&item.path(), true, apply)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn c_path(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow::anyhow!("路径含 NUL 字节：{}", path.display()))
}

#[cfg(unix)]
pub fn chmod(paths: &[String], mode: u32, recursive: bool) -> Result<()> {
    for path in paths {
        for_each_in_tree(Path::new(path), recursive, &mut |target| {
            let c = c_path(target)?;
            if unsafe { libc::chmod(c.as_ptr(), mode as libc::mode_t) } != 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| io_context("修改权限", target));
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn chmod(_paths: &[String], _mode: u32, _recursive: bool) -> Result<()> {
    bail!("当前平台不支持修改权限")
}

#[cfg(unix)]
pub fn chown(paths: &[String], uid: u32, gid: u32, recursive: bool) -> Result<()> {
    for path in paths {
        for_each_in_tree(Path::new(path), recursive, &mut |target| {
            let c = c_path(target)?;
            if unsafe { libc::chown(c.as_ptr(), uid, gid) } != 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| io_context("修改所有者", target));
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn chown(_paths: &[String], _uid: u32, _gid: u32, _recursive: bool) -> Result<()> {
    bail!("当前平台不支持修改所有者")
}

/// Read a text file for the editor, refusing anything that looks binary.
pub fn read_text(path: &str, max_bytes: u64) -> Result<String> {
    let p = Path::new(path);
    let size = fs::metadata(p)
        .with_context(|| io_context("读取", p))?
        .len();
    if size > max_bytes {
        bail!("文件 {size} 字节，超过编辑器 {max_bytes} 字节上限");
    }

    let bytes = fs::read(p).with_context(|| io_context("读取", p))?;
    decode_text(bytes, max_bytes)
}

/// Read a text member out of an archive, for the same editor a loose file opens in.
pub fn read_archive_text(archive: &str, entry: &str, max_bytes: u64) -> Result<String> {
    // The member reader refuses anything above the cap before reading it, so a huge member is
    // an error rather than a request that fills memory.
    let bytes = read_archive_entry(archive, entry, max_bytes)?;
    decode_text(bytes, max_bytes)
}

/// Replace one member of an archive with new bytes: the editor saving a file it opened from
/// inside a zip. The rewrite is the same one the add and delete gestures use, so a failure
/// leaves the archive as it was.
pub fn write_archive_entry(archive: &str, entry: &str, content: &str) -> Result<()> {
    let target = existing_archive(archive)?;
    let name = entry.trim_start_matches('/').trim_end_matches('/');
    if name.is_empty() {
        bail!("没有指定要写入的条目");
    }
    rewrite_archive(
        target,
        &[name.to_string()],
        &[(
            name.to_string(),
            Content::Bytes(content.as_bytes().to_vec()),
        )],
    )?;
    Ok(())
}

/// The bytes an editor is allowed to open, or the reason it is not.
fn decode_text(bytes: Vec<u8>, max_bytes: u64) -> Result<String> {
    if bytes.len() as u64 > max_bytes {
        bail!("文件 {} 字节，超过编辑器 {max_bytes} 字节上限", bytes.len());
    }
    if bytes.iter().take(8192).any(|b| *b == 0) {
        bail!("这看起来是二进制文件，无法用文本编辑器打开");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn write_text(path: &str, content: &str) -> Result<()> {
    let p = Path::new(path);
    fs::write(p, content.as_bytes()).with_context(|| io_context("写入", p))
}

pub fn read_bytes(path: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let p = Path::new(path);
    let size = fs::metadata(p)
        .with_context(|| io_context("读取", p))?
        .len();
    if size > max_bytes {
        bail!("文件 {size} 字节，超过 {max_bytes} 上限");
    }
    fs::read(p).with_context(|| io_context("读取", p))
}

pub fn write_bytes(path: &str, data: &[u8]) -> Result<()> {
    let p = Path::new(path);
    fs::write(p, data).with_context(|| io_context("写入", p))
}

// ---- archives (zip) ----

#[derive(Serialize)]
pub struct ArchiveEntry {
    pub name: String,
    /// Path inside the archive; doubles as the key used when extracting a subset.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub compressed: u64,
    /// Seconds since the epoch, as the entry records it. A folder inside a zip has no entry of
    /// its own unless it was archived empty, so it borrows the newest time among its members.
    pub mtime: i64,
}

/// Extensions the archive viewer handles.
pub fn is_archive(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Zip entries carry no guaranteed encoding: the flag says UTF-8, and archives written on
/// Chinese Windows store GBK. Decoding both is what keeps file names readable.
fn zip_name<R: std::io::Read>(entry: &zip::read::ZipFile<'_, R>) -> String {
    let raw = entry.name_raw();
    if let Ok(text) = std::str::from_utf8(raw) {
        return text.to_string();
    }
    let (text, _, _) = encoding_rs::GBK.decode(raw);
    text.into_owned()
}

/// Reject anything that would escape the destination when an entry is written out.
fn sanitize_member(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn normalize_sub(sub: &str) -> String {
    let trimmed = sub.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// When an entry was last written, in seconds since the epoch.
///
/// Zip times are MS-DOS style: *local* time, two-second resolution, no zone recorded. Reading
/// them as local is what makes an archive entry's time match the same file's time on the
/// filesystem listing beside it.
fn zip_entry_mtime<R: std::io::Read>(entry: &zip::read::ZipFile<'_, R>) -> i64 {
    use chrono::TimeZone as _;

    // Absent when the entry carries no timestamp at all, which is allowed.
    let Some(stamp) = entry.last_modified() else {
        return 0;
    };
    let naive = chrono::NaiveDate::from_ymd_opt(
        i32::from(stamp.year()),
        u32::from(stamp.month()),
        u32::from(stamp.day()),
    )
    .and_then(|date| {
        date.and_hms_opt(
            u32::from(stamp.hour()),
            u32::from(stamp.minute()),
            u32::from(stamp.second()),
        )
    });
    naive
        .and_then(|naive| chrono::Local.from_local_datetime(&naive).earliest())
        .map_or(0, |local| local.timestamp())
}

/// Immediate children of `sub` inside the archive, directories first.
pub fn list_archive(path: &str, sub: &str) -> Result<Vec<ArchiveEntry>> {
    let file = fs::File::open(path).with_context(|| io_context("打开压缩包", Path::new(path)))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("不是可读的 zip 压缩包：{path}"))?;

    let prefix = normalize_sub(sub);
    let mut dirs: Vec<ArchiveEntry> = Vec::new();
    let mut files: Vec<ArchiveEntry> = Vec::new();
    // Newest member time per synthesized folder, filled in as its members go by.
    let mut dir_times: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let full = zip_name(&entry);
        let Some(rest) = full.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let mtime = zip_entry_mtime(&entry);

        if let Some((dir, _)) = rest.split_once('/') {
            let dir_path = format!("{prefix}{dir}/");
            let newest = dir_times.entry(dir_path.clone()).or_insert(mtime);
            *newest = (*newest).max(mtime);
            if !dirs.iter().any(|d| d.path == dir_path) {
                dirs.push(ArchiveEntry {
                    name: dir.to_string(),
                    path: dir_path,
                    is_dir: true,
                    size: 0,
                    compressed: 0,
                    mtime,
                });
            }
        } else {
            files.push(ArchiveEntry {
                name: rest.to_string(),
                path: full.clone(),
                is_dir: false,
                size: entry.size(),
                compressed: entry.compressed_size(),
                mtime,
            });
        }
    }

    for dir in &mut dirs {
        if let Some(newest) = dir_times.get(&dir.path) {
            dir.mtime = *newest;
        }
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    dirs.extend(files);
    Ok(dirs)
}

/// Extract the whole archive, or just `entries` (a name matches itself and everything under
/// it), into `dest_dir`. Returns the paths written.
pub fn extract_archive(path: &str, dest_dir: &str, entries: &[String]) -> Result<Vec<String>> {
    let dest = Path::new(dest_dir);
    if !dest.is_dir() {
        bail!("目标不是目录：{dest_dir}");
    }

    let file = fs::File::open(path).with_context(|| io_context("打开压缩包", Path::new(path)))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("不是可读的 zip 压缩包：{path}"))?;

    let mut written = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let full = zip_name(&entry);

        if !entries.is_empty() {
            let wanted = entries.iter().any(|want| {
                let want = want.trim_end_matches('/');
                full == want || full.starts_with(&format!("{want}/"))
            });
            if !wanted {
                continue;
            }
        }

        let Some(relative) = sanitize_member(&full) else {
            log::warn!("skipping unsafe archive member: {full}");
            continue;
        };
        let target = dest.join(relative);

        if full.ends_with('/') {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&target).with_context(|| io_context("写入", &target))?;
        std::io::copy(&mut entry, &mut out).with_context(|| io_context("解压", &target))?;
        written.push(target.display().to_string());
    }

    if written.is_empty() {
        bail!("压缩包里没有匹配的内容");
    }
    Ok(written)
}

/// Add files and folders to an existing zip, replacing any entry that ends up with the same
/// name. Entries land under `sub` — the folder the file manager is showing inside the archive
/// — and a folder is added together with everything under it. Returns the names written.
///
/// The archive is rebuilt rather than edited: a zip's central directory sits at the end and
/// every offset in it points at content before that, so there is nowhere to put a member. The
/// rebuild goes to a sibling file and is renamed over the original only once it is complete,
/// which also means a failure halfway leaves the user's archive untouched.
pub fn add_to_archive(archive: &str, sources: &[String], sub: &str) -> Result<Vec<String>> {
    use std::collections::HashMap;

    let target = existing_archive(archive)?;
    if sources.is_empty() {
        bail!("没有选中要加入的内容");
    }

    let prefix = normalize_sub(sub);
    let mut adding: Vec<(String, Content)> = Vec::new();
    let mut taken: HashMap<String, u32> = HashMap::new();
    for source in sources {
        let path = PathBuf::from(source);
        if !path.exists() {
            bail!("不存在：{source}");
        }
        let Some(base) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            bail!("无法确定名称：{source}");
        };
        // Two selections can resolve to one member name (`a/logo.png` and `b/logo.png`).
        // Keeping the first and numbering the rest is what a desktop archiver does, and it
        // beats either refusing the whole batch or quietly dropping one of them.
        let mut name = format!("{prefix}{base}");
        let repeat = taken.entry(name.clone()).or_insert(0);
        *repeat += 1;
        if *repeat > 1 {
            name = format!("{prefix}{base} ({repeat})");
        }

        if path.is_dir() {
            collect_tree(&path, &name, &mut adding)?;
        } else {
            adding.push((name, Content::FromFile(path)));
        }
    }
    if adding.is_empty() {
        bail!("选中的目录是空的");
    }

    // What is being added also decides what leaves: same name, new contents, the way an
    // archiver with "overwrite" behaves.
    let shadowed: Vec<String> = adding.iter().map(|(name, _)| name.clone()).collect();
    Ok(rewrite_archive(target, &shadowed, &adding)?.added)
}

/// Remove members from an archive. A name matches itself and everything under it, so deleting
/// a folder inside the archive takes its contents with it. Returns the names dropped.
/// A new archive holding the given selections.
///
/// The member naming, the numbering of clashing names and the walking of directories are all
/// `add_to_archive`; the only difference is that the target does not exist yet, so an empty archive
/// is written first and the work handed over. An existing target is refused rather than merged
/// into: this is the 「压缩」 action, and quietly adding to someone's archive is not what it says.
pub fn create_archive(archive: &str, sources: &[String], sub: &str) -> Result<Vec<String>> {
    let path = PathBuf::from(archive);
    if !is_archive(archive) {
        bail!("压缩包名字要以 .zip 结尾：{archive}");
    }
    if path.exists() {
        bail!("已存在同名文件：{archive}");
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!("目录不存在：{}", parent.display());
        }
    }

    {
        let file = fs::File::create(&path).with_context(|| format!("创建 {archive} 失败"))?;
        zip::ZipWriter::new(file)
            .finish()
            .with_context(|| format!("写入 {archive} 失败"))?;
    }

    match add_to_archive(archive, sources, sub) {
        Ok(added) => Ok(added),
        Err(e) => {
            // An archive that exists only because this ran, and holds nothing, is worse than none:
            // the selections are still on disk, so the empty shell goes away again.
            let _ = fs::remove_file(&path);
            Err(e)
        }
    }
}

pub fn delete_from_archive(archive: &str, entries: &[String]) -> Result<Vec<String>> {
    let target = existing_archive(archive)?;
    if entries.is_empty() {
        bail!("没有选中要删除的内容");
    }
    let dropped = rewrite_archive(target, entries, &[])?.dropped;
    if dropped.is_empty() {
        bail!("压缩包里没有匹配的内容");
    }
    Ok(dropped)
}

/// Read one member of an archive, for the preview the file manager shows.
pub fn read_archive_entry(archive: &str, entry: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let file =
        fs::File::open(archive).with_context(|| io_context("打开压缩包", Path::new(archive)))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("不是可读的 zip 压缩包：{archive}"))?;

    let wanted = entry.trim_start_matches('/').trim_end_matches('/');
    for index in 0..zip.len() {
        let mut member = zip.by_index(index)?;
        if zip_name(&member).trim_end_matches('/') != wanted {
            continue;
        }
        if member.size() > max_bytes {
            bail!("文件太大，无法预览：{} 字节", member.size());
        }
        let mut data = Vec::with_capacity(member.size() as usize);
        std::io::copy(&mut member, &mut data)
            .with_context(|| io_context("读取压缩包条目", Path::new(archive)))?;
        return Ok(data);
    }
    bail!("压缩包里没有这一项：{entry}")
}

/// The package name an APK declares, read out of its manifest.
///
/// The file manager describes an installer before installing it, so this has to work on a file
/// that is not a package yet: what `pm install` will put on the device is written in the
/// manifest, and nowhere else in the archive.
#[cfg(target_os = "android")]
pub fn apk_package(path: &str) -> Result<String> {
    let file = fs::File::open(path).with_context(|| io_context("打开安装包", Path::new(path)))?;
    crate::apkparser::extract_package(file).context("这个安装包里读不到包名")
}

/// The same, for an APK that lives inside an archive.
///
/// Bounded by [`MAX_RAW_BYTES`]: a zip is read from its end, so the whole member has to be in
/// hand before it can be read at all. A larger installer answers with an error instead, which
/// the panel shows as "读不出来" — one line of text is not worth unbounded memory.
#[cfg(target_os = "android")]
pub fn apk_package_in_archive(archive: &str, entry: &str) -> Result<String> {
    let bytes = read_archive_entry(archive, entry, MAX_RAW_BYTES)?;
    crate::apkparser::extract_package(std::io::Cursor::new(bytes)).context("这个安装包里读不到包名")
}

#[cfg(not(target_os = "android"))]
pub fn apk_package(_path: &str) -> Result<String> {
    bail!("当前平台不支持读取安装包")
}

#[cfg(not(target_os = "android"))]
pub fn apk_package_in_archive(_archive: &str, _entry: &str) -> Result<String> {
    bail!("当前平台不支持读取安装包")
}

fn existing_archive(archive: &str) -> Result<&Path> {
    let target = Path::new(archive);
    if !target.is_file() {
        bail!("压缩包不存在：{archive}");
    }
    Ok(target)
}

/// Where a member being written into an archive comes from.
enum Content {
    /// Copied from a file on the device.
    FromFile(PathBuf),
    /// Written from memory — the editor saving a member back into the archive.
    Bytes(Vec<u8>),
    /// A folder record, which has no content of its own.
    Directory,
}

/// What one rebuild of an archive did.
struct Rewrite {
    /// The additions, in the order they were given.
    added: Vec<String>,
    /// Existing members left out because they matched a skip name.
    dropped: Vec<String>,
}

/// Rewrite `target` without `skip`, with `additions` written in, and rename the result over the
/// original. `additions` are `(name inside the archive, where its bytes come from)`.
///
/// This is the one place that writes a zip, because a zip can only be changed by rebuilding it:
/// the central directory sits at the end and every offset in it points at content before that,
/// so there is nowhere to put a member. The rebuild goes to a sibling file and is renamed over
/// the original only once it is complete, which is also what makes a failure halfway leave the
/// user's archive exactly as it was.
fn rewrite_archive(
    target: &Path,
    skip: &[String],
    additions: &[(String, Content)],
) -> Result<Rewrite> {
    use zip::write::SimpleFileOptions;

    // `x.zip` -> `x.ksud-tmp`: same directory, so the final step is a rename.
    let staging = target.with_extension("ksud-tmp");
    let skipped: Vec<&str> = skip
        .iter()
        .map(|name| name.trim_end_matches('/'))
        .filter(|name| !name.is_empty())
        .collect();

    let built = (|| -> Result<Rewrite> {
        let source_file =
            fs::File::open(target).with_context(|| io_context("打开压缩包", target))?;
        let mut existing = zip::ZipArchive::new(source_file)
            .with_context(|| format!("不是可读的 zip 压缩包：{}", target.display()))?;

        let out = fs::File::create(&staging).with_context(|| io_context("写入", &staging))?;
        let mut writer = zip::ZipWriter::new(out);

        let mut dropped = Vec::new();
        for index in 0..existing.len() {
            let entry = existing.by_index(index)?;
            let name = zip_name(&entry);
            let matches = |want: &str| name == want || name.starts_with(&format!("{want}/"));
            if skipped.iter().any(|want| matches(want)) {
                dropped.push(name);
                continue;
            }
            // Raw copy: the entry keeps its own compression method, and an encrypted member is
            // passed through as it is instead of being decrypted and rewritten here.
            writer.raw_copy_file(entry)?;
        }

        let file_options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let dir_options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o755);
        let mut added = Vec::new();
        for (name, content) in additions {
            match content {
                Content::Directory => writer.add_directory(format!("{name}/"), dir_options)?,
                Content::FromFile(path) => {
                    writer.start_file(name.clone(), file_options)?;
                    let mut file =
                        fs::File::open(path).with_context(|| io_context("读取", path))?;
                    std::io::copy(&mut file, &mut writer)
                        .with_context(|| io_context("加入压缩包", path))?;
                }
                Content::Bytes(bytes) => {
                    writer.start_file(name.clone(), file_options)?;
                    writer
                        .write_all(bytes)
                        .with_context(|| io_context("写入压缩包条目", Path::new(name)))?;
                }
            }
            added.push(name.clone());
        }

        // The archive's own comment is not part of any entry, so it has to be carried over
        // explicitly; it is stored as raw bytes, not necessarily UTF-8.
        let comment = existing.comment();
        if !comment.is_empty() {
            writer.set_raw_comment(comment.to_vec().into_boxed_slice())?;
        }
        writer.finish().context("写入压缩包失败")?;
        Ok(Rewrite { added, dropped })
    })();

    match built {
        Ok(outcome) => {
            // The staged file was created with the default mode; the archive's own mode and
            // owner are the ones to keep. (A rename cannot carry them across by itself.)
            let _ = fs::set_permissions(&staging, fs::metadata(target)?.permissions());
            fs::rename(&staging, target).with_context(|| io_context("替换压缩包", target))?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = fs::remove_file(&staging);
            Err(e)
        }
    }
}

/// Walk `dir` into archive members named `prefix/…`, emitting a folder entry for every
/// directory so that an empty one survives the round trip.
fn collect_tree(dir: &Path, prefix: &str, out: &mut Vec<(String, Content)>) -> Result<()> {
    out.push((prefix.to_string(), Content::Directory));
    let read = fs::read_dir(dir).with_context(|| io_context("读取目录", dir))?;
    for item in read {
        let item = item?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        if item.file_type()?.is_dir() {
            collect_tree(&path, &format!("{prefix}/{name}"), out)?;
        } else {
            out.push((format!("{prefix}/{name}"), Content::FromFile(path)));
        }
    }
    Ok(())
}

// ---- apk install ----

/// Where an APK is put before `pm` is asked to install it.
#[cfg(target_os = "android")]
const APK_STAGING: &str = "/data/local/tmp/ksu-install.apk";

/// Install an APK that sits on the filesystem.
///
/// The copy is not optional: `pm` runs in the system process, which cannot open a root-only
/// path such as /data/adb/x.apk, so passing the original path would fail with a permission
/// error that looks nothing like the real cause.
#[cfg(target_os = "android")]
pub fn install_apk(path: &str) -> Result<String> {
    let source = Path::new(path);
    if !source.is_file() {
        bail!("不是文件：{path}");
    }
    let staged = Path::new(APK_STAGING);
    fs::copy(source, staged).with_context(|| io_context("暂存安装包", staged))?;
    install_staged_apk(staged)
}

/// Install an APK that lives inside an archive.
///
/// The member is streamed out to the same staging path a loose APK uses, because `pm` can only
/// open a real file — and streamed rather than read into memory, since an installer is easily
/// larger than the read caps the rest of this module works under.
#[cfg(target_os = "android")]
pub fn install_apk_from_archive(archive: &str, entry: &str) -> Result<String> {
    let file =
        fs::File::open(archive).with_context(|| io_context("打开压缩包", Path::new(archive)))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("不是可读的 zip 压缩包：{archive}"))?;

    let wanted = entry.trim_start_matches('/').trim_end_matches('/');
    let staged = Path::new(APK_STAGING);
    for index in 0..zip.len() {
        let mut member = zip.by_index(index)?;
        if zip_name(&member).trim_end_matches('/') != wanted {
            continue;
        }
        let mut out = fs::File::create(staged).with_context(|| io_context("暂存安装包", staged))?;
        std::io::copy(&mut member, &mut out).with_context(|| io_context("解出安装包", staged))?;
        drop(out);
        return install_staged_apk(staged);
    }
    bail!("压缩包里没有这一项：{entry}")
}

/// The half every install path shares: make the staged file readable, run `pm`, clean up.
#[cfg(target_os = "android")]
fn install_staged_apk(staged: &Path) -> Result<String> {
    use std::process::Command;

    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(staged, fs::Permissions::from_mode(0o644));
    }

    let output = Command::new("pm")
        .args(["install", "-r", APK_STAGING])
        .output()
        .with_context(|| "无法执行 pm install")?;
    let _ = fs::remove_file(staged);

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let trimmed = text.trim().to_string();

    if !output.status.success() || trimmed.contains("Failure") {
        bail!(
            "安装失败：{}",
            if trimmed.is_empty() {
                "pm 没有返回原因".to_string()
            } else {
                trimmed
            }
        );
    }
    Ok(trimmed)
}

#[cfg(not(target_os = "android"))]
pub fn install_apk(_path: &str) -> Result<String> {
    bail!("当前平台不支持安装 APK")
}

#[cfg(not(target_os = "android"))]
pub fn install_apk_from_archive(_archive: &str, _entry: &str) -> Result<String> {
    bail!("当前平台不支持安装 APK")
}

// ---- the panel's own state files ----
//
// The quick jumps, the 便捷执行 list and the theme are all small JSON documents under
// `WORKING_DIR`, read and written the same way.

/// Read one of them. A missing or unreadable file is the default rather than an error: the panel
/// has to work before anything has been saved, and a half-written file must not take it down.
#[cfg(target_os = "android")]
fn load_json<T: serde::de::DeserializeOwned + Default>(path: &str) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<T>(&text).ok())
        .unwrap_or_default()
}

#[cfg(not(target_os = "android"))]
fn load_json<T: Default>(_path: &str) -> T {
    T::default()
}

/// Write one back: 0600, under the working directory, like every other file the panel owns.
#[cfg(target_os = "android")]
fn save_json<T: Serialize + ?Sized>(path: &str, value: &T) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::create_dir_all(crate::defs::WORKING_DIR)
        .with_context(|| io_context("创建数据目录", Path::new(crate::defs::WORKING_DIR)))?;
    let body = serde_json::to_vec_pretty(value)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(&body))
        .with_context(|| format!("写入 {path} 失败"))
}

#[cfg(not(target_os = "android"))]
fn save_json<T: ?Sized>(_path: &str, _value: &T) -> Result<()> {
    bail!("当前平台不支持写入网页端数据")
}

// ---- quick-jump entries ----
//
// Kept on the device rather than in the browser: localStorage is keyed by origin, so a
// changed port (or cleared site data) would silently drop the user's saved shortcuts.

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct Jump {
    pub name: String,
    pub path: String,
}

pub fn load_jumps() -> Vec<Jump> {
    load_json(crate::defs::WEBUI_JUMPS_PATH)
}

pub fn save_jumps(jumps: &[Jump]) -> Result<()> {
    save_json(crate::defs::WEBUI_JUMPS_PATH, jumps)
}

// ---- 便捷执行 entries ----
//
// The same device-side storage as the quick jumps, for the same reason: a script kept for the
// file manager's own button has to survive a changed port, and localStorage would not.

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct QuickRun {
    pub name: String,
    pub path: String,
}

pub fn load_quick_run() -> Vec<QuickRun> {
    load_json(crate::defs::WEBUI_QUICK_RUN_PATH)
}

pub fn save_quick_run(items: &[QuickRun]) -> Result<()> {
    save_json(crate::defs::WEBUI_QUICK_RUN_PATH, items)
}

// ---- the web UI's own theme ----
//
// On the device for the reason the two lists above are: the page is served on a port that
// changes whenever the launcher hands out a fresh address, and localStorage is keyed by origin —
// a theme kept there would reset itself the next time the port moved.

/// The Manager app's theme setting has four modes and a key colour; the modes are the app's
/// (system / light / dark / AMOLED) and the colour is a seed the page derives its tones from.
/// There is deliberately no "follow the wallpaper" variant: a browser cannot read it.
fn on() -> bool {
    true
}

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct Theme {
    /// "system" / "light" / "dark" / "amoled". Empty means the same as system.
    #[serde(default)]
    pub mode: String,
    /// A key colour as `#rrggbb`, or `wallpaper` for the app's 默认 — its Monet dynamic colour,
    /// which the page takes from the device's wallpaper file. Empty keeps the page's own accent.
    #[serde(default)]
    pub seed: String,
    /// The app's 色彩风格: one of material-kolor's `PaletteStyle` names.
    #[serde(default)]
    pub style: String,
    /// The app's 色彩标准: `SPEC_2021` or `SPEC_2025`. The 2025 tones only exist for the styles
    /// the app itself gates them to (see the page); anything else is read as 2021.
    #[serde(default)]
    pub spec: String,
    /// The app's 模糊: whether the top and bottom bars are drawn as frosted glass.
    #[serde(default = "on")]
    pub blur: bool,
    /// The app's 悬浮底栏: whether the bar floats over the page instead of docking to its edge.
    #[serde(default = "on")]
    pub float_bar: bool,
    /// The app's 液态玻璃 (`enableFloatingBottomBarBlur`): a heavier backdrop blur and a lit edge on
    /// the floating bar. Off by default, as it is in the app — the bar looks right without it.
    #[serde(default)]
    pub glass: bool,
}

/// Everything on unless a saved setting says otherwise: an old config file — or none at all — has
/// to leave the page looking the way it ships, not silently switch its blur off.
impl Default for Theme {
    fn default() -> Self {
        Self {
            mode: String::new(),
            seed: String::new(),
            style: String::new(),
            spec: String::new(),
            blur: true,
            float_bar: true,
            glass: false,
        }
    }
}

impl Theme {
    fn normalized(mut self) -> Self {
        if self.mode.is_empty() {
            self.mode = "system".to_string();
        }
        if self.style.is_empty() {
            self.style = "TonalSpot".to_string();
        }
        if self.spec.is_empty() {
            self.spec = "SPEC_2025".to_string();
        }
        self
    }
}

pub fn load_theme() -> Theme {
    load_json::<Theme>(crate::defs::WEBUI_THEME_PATH).normalized()
}

pub fn save_theme(theme: &Theme) -> Result<()> {
    // Only the four fields the picker owns are normalized on the way out; the blur and the two
    // switches are stored exactly as they arrived.
    save_json(crate::defs::WEBUI_THEME_PATH, &theme.clone().normalized())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each call gets its own directory: tests run in parallel and would otherwise delete
    /// each other's files.
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ksu-fs-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Writes a zip the way any archiver would: entries only, no folder records.
    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        use zip::write::SimpleFileOptions;
        let file = fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in files {
            writer.start_file(*name, options).expect("start entry");
            writer
                .write_all(content.as_bytes())
                .expect("write entry body");
        }
        writer.finish().expect("finish zip");
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let file = fs::File::open(path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        (0..archive.len())
            .map(|i| zip_name(&archive.by_index(i).expect("entry")))
            .collect()
    }

    #[test]
    fn lists_and_extracts_a_subset_of_an_archive() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(
            &zip,
            &[
                ("top.txt", "one"),
                ("inner/deep.txt", "two"),
                ("inner/more.txt", "three"),
            ],
        );

        let entries = list_archive(zip.to_str().expect("utf8"), "").expect("list root");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // The folder is synthesized from the member names and comes first.
        assert_eq!(names, vec!["inner", "top.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].path, "inner/");
        assert_eq!(entries[1].size, 3);

        let inner = list_archive(zip.to_str().expect("utf8"), "inner").expect("list inner");
        let names: Vec<&str> = inner.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["more.txt", "deep.txt"], "newest/largest first");

        // Extracting the folder takes everything under it, and nothing beside it.
        let dest = dir.join("out");
        fs::create_dir(&dest).expect("mkdir out");
        let written = extract_archive(
            zip.to_str().expect("utf8"),
            dest.to_str().expect("utf8"),
            &["inner".to_string()],
        )
        .expect("extract subset");
        assert_eq!(written.len(), 2);
        assert_eq!(
            fs::read_to_string(dest.join("inner/deep.txt")).expect("read"),
            "two"
        );
        assert!(!dest.join("top.txt").exists());
    }

    #[test]
    fn adds_files_and_folders_to_an_existing_archive() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("keep.txt", "kept"), ("dir/b.txt", "b")]);
        // Uncompressed size of the archive, to show the rebuild did not eat the old members.
        let before = zip_names(&zip);
        assert_eq!(before.len(), 2);

        let new_dir = dir.join("newdir");
        fs::create_dir_all(new_dir.join("nested")).expect("mkdir nested");
        fs::create_dir_all(new_dir.join("empty")).expect("mkdir empty");
        fs::write(new_dir.join("root.txt"), b"root").expect("write");
        fs::write(new_dir.join("nested/deep.txt"), b"deep").expect("write deep");
        let extra = dir.join("extra.txt");
        fs::write(&extra, b"extra").expect("write extra");

        let written = add_to_archive(
            zip.to_str().expect("utf8"),
            &[new_dir.display().to_string(), extra.display().to_string()],
            "",
        )
        .expect("add");
        // Compared as a set: the order members come out of a directory walk is the filesystem's
        // business, not this function's.
        let mut got = written.clone();
        got.sort();
        let mut want = vec![
            "extra.txt",
            "newdir",
            "newdir/empty",
            "newdir/nested",
            "newdir/nested/deep.txt",
            "newdir/root.txt",
        ];
        want.sort();
        assert_eq!(got, want);

        let zip_path = zip.to_str().expect("utf8");
        let names = zip_names(&zip);
        assert!(
            names.contains(&"keep.txt".to_string()),
            "old member lost: {names:?}"
        );
        assert!(
            names.contains(&"dir/b.txt".to_string()),
            "old member lost: {names:?}"
        );
        assert!(
            names.contains(&"newdir/empty/".to_string()),
            "empty folder lost: {names:?}"
        );
        // An empty folder has no members to be inferred from, so it must survive as a folder
        // entry — otherwise the UI would list it as an empty file.
        let inside = list_archive(zip_path, "newdir").expect("list newdir");
        let empty = inside
            .iter()
            .find(|e| e.name == "empty")
            .expect("empty folder");
        assert!(empty.is_dir, "the empty folder came back as a file");

        // The new contents are readable through the same reader the UI uses, and the folder
        // shows up as a folder at the root.
        let root = list_archive(zip_path, "").expect("list");
        assert!(root.iter().any(|e| e.is_dir && e.name == "newdir"));
        let dest = dir.join("out");
        fs::create_dir(&dest).expect("mkdir out");
        extract_archive(zip_path, dest.to_str().expect("utf8"), &[]).expect("extract all");
        assert_eq!(
            fs::read_to_string(dest.join("newdir/nested/deep.txt")).expect("read"),
            "deep"
        );
        assert_eq!(
            fs::read_to_string(dest.join("extra.txt")).expect("read"),
            "extra"
        );
    }

    #[test]
    fn an_added_name_replaces_the_entry_it_shadows() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("same.txt", "old"), ("other.txt", "other")]);
        let replacement = dir.join("same.txt");
        fs::write(&replacement, b"new").expect("write");

        add_to_archive(
            zip.to_str().expect("utf8"),
            &[replacement.display().to_string()],
            "",
        )
        .expect("add");

        let names = zip_names(&zip);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "same.txt").count(),
            1,
            "the entry was duplicated instead of replaced: {names:?}"
        );
        let dest = dir.join("out");
        fs::create_dir(&dest).expect("mkdir out");
        extract_archive(
            zip.to_str().expect("utf8"),
            dest.to_str().expect("utf8"),
            &[],
        )
        .expect("extract");
        assert_eq!(
            fs::read_to_string(dest.join("same.txt")).expect("read"),
            "new"
        );
        assert_eq!(
            fs::read_to_string(dest.join("other.txt")).expect("read"),
            "other",
            "an unrelated entry was dropped"
        );
    }

    #[test]
    fn adding_stays_inside_the_folder_the_archive_is_showing() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("inner/keep.txt", "kept")]);
        let file = dir.join("loose.txt");
        fs::write(&file, b"loose").expect("write");

        let written = add_to_archive(
            zip.to_str().expect("utf8"),
            &[file.display().to_string()],
            "/inner/",
        )
        .expect("add");
        assert_eq!(written, vec!["inner/loose.txt"]);
        let names = zip_names(&zip);
        assert!(names.contains(&"inner/loose.txt".to_string()), "{names:?}");
        assert!(names.contains(&"inner/keep.txt".to_string()), "{names:?}");
    }

    #[test]
    fn two_sources_with_one_name_both_land() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("seed.txt", "seed")]);
        let a = dir.join("a");
        let b = dir.join("b");
        fs::create_dir(&a).expect("mkdir a");
        fs::create_dir(&b).expect("mkdir b");
        fs::write(a.join("logo.png"), b"first").expect("write");
        fs::write(b.join("logo.png"), b"second").expect("write");

        let written = add_to_archive(
            zip.to_str().expect("utf8"),
            &[
                a.join("logo.png").display().to_string(),
                b.join("logo.png").display().to_string(),
            ],
            "",
        )
        .expect("add");
        assert_eq!(written, vec!["logo.png", "logo.png (2)"]);
    }

    #[test]
    fn a_failed_add_leaves_the_archive_as_it_was() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("keep.txt", "kept")]);
        let before = fs::read(&zip).expect("read zip");

        let err = add_to_archive(
            zip.to_str().expect("utf8"),
            &[dir.join("missing").display().to_string()],
            "",
        )
        .expect_err("adding a missing path must fail");
        assert!(
            format!("{err:#}").contains("不存在"),
            "unexpected error: {err:#}"
        );

        assert_eq!(
            fs::read(&zip).expect("read zip"),
            before,
            "archive was modified"
        );
        assert!(
            !zip.with_extension("ksud-tmp").exists(),
            "staging file was left behind"
        );
    }

    #[test]
    fn deletes_members_and_keeps_the_rest() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(
            &zip,
            &[
                ("keep.txt", "kept"),
                ("docs/readme.txt", "readme"),
                ("docs/deep/more.txt", "more"),
            ],
        );

        let zip_path = zip.to_str().expect("utf8");
        // Deleting a folder takes everything under it, not just the folder entry.
        let dropped = delete_from_archive(zip_path, &["docs".to_string()]).expect("delete");
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        assert_eq!(zip_names(&zip), vec!["keep.txt"]);

        // And what is left still reads back through the same reader the UI uses.
        let dest = dir.join("out");
        fs::create_dir(&dest).expect("mkdir out");
        extract_archive(zip_path, dest.to_str().expect("utf8"), &[]).expect("extract");
        assert_eq!(
            fs::read_to_string(dest.join("keep.txt")).expect("read"),
            "kept"
        );
    }

    #[test]
    fn a_delete_that_matches_nothing_leaves_the_archive_as_it_was() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("keep.txt", "kept")]);
        let before = fs::read(&zip).expect("read zip");

        let err = delete_from_archive(zip.to_str().expect("utf8"), &["missing".to_string()])
            .expect_err("nothing matched");
        assert!(
            format!("{err:#}").contains("没有匹配"),
            "unexpected error: {err:#}"
        );

        assert_eq!(
            fs::read(&zip).expect("read zip"),
            before,
            "archive was modified"
        );
        assert!(
            !zip.with_extension("ksud-tmp").exists(),
            "staging file left behind"
        );
    }

    #[test]
    fn reads_one_member_of_an_archive() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("docs/hello.txt", "hello world")]);
        let zip_path = zip.to_str().expect("utf8");

        assert_eq!(
            read_archive_entry(zip_path, "docs/hello.txt", 1024).expect("read member"),
            b"hello world"
        );
        // The cap is what turns a huge member into a clear error rather than a request that
        // never finishes.
        assert!(read_archive_entry(zip_path, "docs/hello.txt", 4).is_err());
        assert!(read_archive_entry(zip_path, "docs/missing.txt", 1024).is_err());
    }

    #[test]
    fn edits_a_text_member_of_an_archive() {
        let dir = tempdir();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("notes.txt", "before"), ("keep.txt", "kept")]);
        let zip_path = zip.to_str().expect("utf8");

        assert_eq!(
            read_archive_text(zip_path, "notes.txt", 1024).expect("read member"),
            "before"
        );
        write_archive_entry(zip_path, "notes.txt", "after").expect("write member");

        assert_eq!(
            read_archive_text(zip_path, "notes.txt", 1024).expect("read back"),
            "after"
        );
        let names = zip_names(&zip);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "notes.txt").count(),
            1,
            "the member was added beside the old one instead of replacing it: {names:?}"
        );
        assert!(
            names.contains(&"keep.txt".to_string()),
            "an unrelated member was dropped: {names:?}"
        );
        // The editor's own rules apply to a member exactly as they do to a loose file.
        assert!(
            read_archive_text(zip_path, "notes.txt", 2).is_err(),
            "size cap not applied"
        );
    }

    #[test]
    fn parses_octal_modes_with_every_accepted_spelling() {
        assert_eq!(parse_mode("777").expect("777"), 0o777);
        assert_eq!(parse_mode("0755").expect("0755"), 0o755);
        assert_eq!(parse_mode("0o644").expect("0o644"), 0o644);
        assert_eq!(parse_mode(" 600 ").expect("padded"), 0o600);
        assert_eq!(parse_mode("0").expect("zero"), 0);
    }

    #[test]
    fn rejects_modes_that_are_not_octal() {
        assert!(parse_mode("").is_err());
        assert!(parse_mode("abc").is_err());
        assert!(parse_mode("8").is_err(), "8 is not an octal digit");
        assert!(parse_mode("77777").is_err(), "too many digits");
    }

    #[test]
    fn renders_mode_text() {
        assert_eq!(mode_text(0o777), "rwxrwxrwx");
        assert_eq!(mode_text(0o755), "rwxr-xr-x");
        assert_eq!(mode_text(0o644), "rw-r--r--");
        assert_eq!(mode_text(0o000), "---------");
    }

    #[test]
    fn lists_directories_first_then_newest() {
        let dir = tempdir();
        fs::create_dir(dir.join("a-dir")).expect("mkdir");
        fs::write(dir.join("older.txt"), b"a").expect("write older");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(dir.join("newer.txt"), b"bb").expect("write newer");

        let entries = list_dir(dir.to_str().expect("utf8")).expect("list");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // Directory first even though the files are newer, then newest file first.
        assert_eq!(names, vec!["a-dir", "newer.txt", "older.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 2);
    }

    #[test]
    fn copies_and_moves_between_directories() {
        let dir = tempdir();
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir(&src).expect("mkdir src");
        fs::create_dir(&dst).expect("mkdir dst");
        fs::write(src.join("file.txt"), b"payload").expect("write");
        fs::create_dir(src.join("nested")).expect("mkdir nested");
        fs::write(src.join("nested").join("deep.txt"), b"deep").expect("write deep");

        copy(
            &[src.join("file.txt").display().to_string()],
            dst.to_str().expect("utf8"),
            false,
        )
        .expect("copy file");
        assert_eq!(fs::read(dst.join("file.txt")).expect("read"), b"payload");

        // Directories come across whole.
        copy(
            &[src.join("nested").display().to_string()],
            dst.to_str().expect("utf8"),
            false,
        )
        .expect("copy dir");
        assert_eq!(
            fs::read(dst.join("nested").join("deep.txt")).expect("read"),
            b"deep"
        );

        // Moving uses a separate file: the first one is already sitting in dst.
        fs::write(src.join("movable.txt"), b"move me").expect("write movable");
        move_paths(
            &[src.join("movable.txt").display().to_string()],
            dst.to_str().expect("utf8"),
            false,
        )
        .expect("move");
        assert!(!src.join("movable.txt").exists());
        assert_eq!(fs::read(dst.join("movable.txt")).expect("read"), b"move me");
    }

    /// Moving onto a name that is already there takes a second, explicit yes — and then the
    /// old entry is gone rather than sitting next to the moved one.
    #[test]
    fn move_can_replace_when_asked() {
        let dir = tempdir();
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).expect("mkdir src");
        fs::create_dir_all(&dst).expect("mkdir dst");
        fs::write(src.join("same.txt"), b"new").expect("write src");
        fs::write(dst.join("same.txt"), b"old").expect("write dst");

        let from = src.join("same.txt").display().to_string();
        let to = dst.to_str().expect("utf8");

        assert!(move_paths(&[from.clone()], to, false).is_err());
        assert_eq!(fs::read(dst.join("same.txt")).expect("read"), b"old");

        move_paths(&[from], to, true).expect("replace");
        assert!(!src.join("same.txt").exists());
        assert_eq!(fs::read(dst.join("same.txt")).expect("read"), b"new");
    }

    #[test]
    fn refuses_to_copy_a_directory_into_itself() {
        let dir = tempdir();
        let outer = dir.join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(&inner).expect("mkdirs");

        let err = copy(
            &[outer.display().to_string()],
            inner.to_str().expect("utf8"),
            false,
        )
        .expect_err("should refuse");
        assert!(
            format!("{err:#}").contains("自己的子目录"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn refuses_to_overwrite_unless_asked() {
        let dir = tempdir();
        let dst = dir.join("dst");
        fs::create_dir(&dst).expect("mkdir");
        fs::write(dir.join("f.txt"), b"a").expect("write");
        fs::write(dst.join("f.txt"), b"b").expect("write dst");

        let source = vec![dir.join("f.txt").display().to_string()];
        assert!(copy(&source, dst.to_str().expect("utf8"), false).is_err());
        copy(&source, dst.to_str().expect("utf8"), true).expect("overwrite allowed");
        assert_eq!(fs::read(dst.join("f.txt")).expect("read"), b"a");
    }

    #[test]
    fn removes_files_and_directories() {
        let dir = tempdir();
        fs::write(dir.join("f.txt"), b"x").expect("write");
        remove(&[dir.join("f.txt").display().to_string()], false).expect("remove file");
        assert!(!dir.join("f.txt").exists());

        let sub = dir.join("sub");
        fs::create_dir_all(sub.join("deeper")).expect("mkdirs");
        let sub_path = vec![sub.display().to_string()];
        assert!(
            remove(&sub_path, false).is_err(),
            "non-empty dir needs recursive"
        );
        remove(&sub_path, true).expect("recursive remove");
        assert!(!sub.exists());
    }

    #[test]
    fn reads_and_writes_text() {
        let dir = tempdir();
        let file = dir.join("note.txt");
        write_text(file.to_str().expect("utf8"), "你好\nworld").expect("write");
        assert_eq!(
            read_text(file.to_str().expect("utf8"), MAX_TEXT_BYTES).expect("read"),
            "你好\nworld"
        );
    }

    #[test]
    fn refuses_binary_files_for_the_editor() {
        let dir = tempdir();
        let file = dir.join("blob.bin");
        fs::write(&file, [0x00, 0x01, 0x02, 0x00]).expect("write");
        let err = read_text(file.to_str().expect("utf8"), MAX_TEXT_BYTES).expect_err("binary");
        assert!(format!("{err:#}").contains("二进制"), "unexpected: {err:#}");
    }

    #[test]
    fn refuses_text_above_the_size_cap() {
        let dir = tempdir();
        let file = dir.join("big.txt");
        fs::write(&file, vec![b'a'; 100]).expect("write");
        assert!(read_text(file.to_str().expect("utf8"), 10).is_err());
    }

    #[test]
    fn entries_serialize_with_the_fields_the_ui_needs() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hi").expect("write");
        let entries = list_dir(dir.to_str().expect("utf8")).expect("list");
        let json = serde_json::to_value(&entries).expect("serialize");
        let array = json.as_array().expect("array");
        // The frontend reads these by name, so a rename here would silently break the UI.
        // snake_case on purpose: it matches the rest of this HTTP API.
        for key in [
            "name",
            "path",
            "is_dir",
            "is_symlink",
            "size",
            "mode",
            "mode_text",
            "uid",
            "gid",
            "mtime",
        ] {
            assert!(
                array[0].get(key).is_some(),
                "entry JSON is missing {key}: {}",
                array[0]
            );
        }
        assert_eq!(array[0]["size"], 2);
    }

    #[cfg(unix)]
    #[test]
    fn chmod_and_chown_actually_change_the_file() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempdir();
        let file = dir.join("mode.txt");
        fs::write(&file, b"x").expect("write");
        let path = vec![file.display().to_string()];

        chmod(&path, 0o600, false).expect("chmod");
        assert_eq!(fs::metadata(&file).expect("stat").mode() & 0o7777, 0o600);

        let md = fs::metadata(&file).expect("stat");
        chown(&path, md.uid(), md.gid(), false).expect("chown to itself is a no-op");
    }

    #[cfg(unix)]
    #[test]
    fn recursive_chmod_walks_the_tree() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempdir();
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).expect("mkdirs");
        fs::write(sub.join("f"), b"x").expect("write");

        chmod(&[dir.display().to_string()], 0o700, true).expect("recursive chmod");
        assert_eq!(fs::metadata(&sub).expect("stat").mode() & 0o7777, 0o700);
        assert_eq!(
            fs::metadata(sub.join("f")).expect("stat").mode() & 0o7777,
            0o700
        );
    }
}
