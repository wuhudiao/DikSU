//! Installed-app inspection for the file manager's 提取APK panel.
//!
//! Everything here reads what the platform already knows — `pm` and `dumpsys` — plus the APK
//! files themselves, the way an archiver's "app info" screen does. The parsing half and the
//! APK-reading half are deliberately free of the shell, so both can be tested off-device.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// What `dumpsys package <pkg>` reveals about one package.
#[derive(Default, Serialize, Debug, PartialEq, Eq)]
pub struct PackageDetails {
    pub package: String,
    pub version_name: String,
    pub version_code: i64,
    pub target_sdk: i64,
    pub min_sdk: i64,
    pub uid: i64,
    pub data_dir: String,
    pub code_path: String,
    pub first_install: String,
    pub last_update: String,
}

/// Pull those fields out of `dumpsys package` output.
///
/// The format is not a published contract and differs between releases, so this reads
/// `key=value` pairs wherever they turn up rather than trusting line positions — the same
/// reason `parse_pm_lines` works the way it does.
pub fn parse_dumpsys_package(text: &str) -> PackageDetails {
    let mut out = PackageDetails::default();

    for line in text.lines() {
        let line = line.trim();
        // `Package [com.x.y] (abc):` names the package; everything after it belongs to it.
        if out.package.is_empty() {
            if let Some(rest) = line.strip_prefix("Package [") {
                if let Some((name, _)) = rest.split_once(']') {
                    out.package = name.to_string();
                }
            }
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let raw = value.trim();
        let first = raw.split_whitespace().next().unwrap_or("");

        match key {
            // A timestamp is two words (`2026-09-13 22:51:27`), so it takes the whole value.
            "firstInstallTime" => out.first_install = raw.to_string(),
            "lastUpdateTime" => out.last_update = raw.to_string(),
            "versionName" => out.version_name = first.to_string(),
            "dataDir" => out.data_dir = first.to_string(),
            "codePath" => out.code_path = first.to_string(),
            "versionCode" => out.version_code = parse_int(first),
            "minSdk" => out.min_sdk = parse_int(first),
            "targetSdk" => out.target_sdk = parse_int(first),
            "userId" => out.uid = parse_int(first),
            _ => {}
        }

        // Android then packs further fields onto the same line
        // (`versionCode=1 minSdk=23 targetSdk=34`), so each later `k=v` word counts too.
        for word in raw.split_whitespace().skip(1) {
            let Some((word_key, word_value)) = word.split_once('=') else {
                continue;
            };
            match word_key {
                "versionCode" => out.version_code = parse_int(word_value),
                "minSdk" => out.min_sdk = parse_int(word_value),
                "targetSdk" => out.target_sdk = parse_int(word_value),
                _ => {}
            }
        }
    }
    out
}

fn parse_int(text: &str) -> i64 {
    text.trim().trim_end_matches(',').parse().unwrap_or(0)
}

/// Which APK signature schemes an APK actually carries.
#[derive(Default, Serialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct SignatureSchemes {
    pub v1: bool,
    pub v2: bool,
    pub v3: bool,
}

impl SignatureSchemes {
    /// `V1 + V2 + V3`, the way an app-info screen writes it.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.v1 {
            parts.push("V1");
        }
        if self.v2 {
            parts.push("V2");
        }
        if self.v3 {
            parts.push("V3");
        }
        if parts.is_empty() {
            "无（或无法读取）".to_string()
        } else {
            parts.join(" + ")
        }
    }
}

/// Read an APK's signing schemes.
///
/// V1 is a JAR signature, so it is visible as `META-INF/*.SF` among the entries. V2 and V3 live
/// in the APK Signing Block between the entries and the central directory, which only a
/// hand-rolled read can find — the zip crate stops at the central directory.
pub fn signature_schemes(apk: &Path) -> Result<SignatureSchemes> {
    let mut out = SignatureSchemes::default();

    {
        let file = fs::File::open(apk).with_context(|| format!("打开 {} 失败", apk.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("不是可读的 APK：{}", apk.display()))?;
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            let name = entry.name().to_ascii_uppercase();
            if name.starts_with("META-INF/")
                && (name.ends_with(".SF")
                    || name.ends_with(".RSA")
                    || name.ends_with(".DSA")
                    || name.ends_with(".EC"))
            {
                out.v1 = true;
                break;
            }
        }
    }

    for id in signing_block_ids(apk)? {
        match id {
            0x7109_871a => out.v2 = true,
            // V3 and its 3.1 revision read as the same thing to anyone looking at a list.
            0xf053_68c0 | 0x1b93_ad61 => out.v3 = true,
            _ => {}
        }
    }
    Ok(out)
}

/// The IDs of the id-value pairs in the APK Signing Block, when there is one.
fn signing_block_ids(apk: &Path) -> Result<Vec<u32>> {
    const EOCD: u32 = 0x0605_4b50;
    const MAGIC: &[u8] = b"APK Sig Block 42";

    let mut file = fs::File::open(apk).with_context(|| format!("打开 {} 失败", apk.display()))?;
    let len = file.metadata()?.len();
    // The end-of-central-directory record is at the very end, after a comment of at most 64 KiB.
    let tail_len = len.min(0xffff + 22);
    let mut tail = vec![0u8; tail_len as usize];
    file.seek(SeekFrom::Start(len - tail_len))?;
    file.read_exact(&mut tail)?;

    let mut cd_offset = None;
    for at in (0..tail.len().saturating_sub(21)).rev() {
        if u32::from_le_bytes([tail[at], tail[at + 1], tail[at + 2], tail[at + 3]]) == EOCD {
            cd_offset = Some(u32::from_le_bytes([
                tail[at + 16],
                tail[at + 17],
                tail[at + 18],
                tail[at + 19],
            ]) as u64);
            break;
        }
    }
    let Some(cd_offset) = cd_offset else {
        return Ok(Vec::new());
    };
    // The block is: entries | size | pairs | size | magic, and the central directory follows.
    if cd_offset < 32 {
        return Ok(Vec::new());
    }
    let mut footer = vec![0u8; 24];
    file.seek(SeekFrom::Start(cd_offset - 24))?;
    file.read_exact(&mut footer)?;
    if &footer[8..24] != MAGIC {
        return Ok(Vec::new());
    }
    let size = u64::from_le_bytes(footer[0..8].try_into().expect("8 bytes"));
    let Some(start) = cd_offset.checked_sub(size + 8) else {
        return Ok(Vec::new());
    };

    let pairs_len = (size as usize).saturating_sub(24);
    let mut pairs = vec![0u8; pairs_len];
    file.seek(SeekFrom::Start(start + 8))?;
    file.read_exact(&mut pairs)?;

    let mut ids = Vec::new();
    let mut at = 0usize;
    while at + 12 <= pairs.len() {
        let pair_len = u64::from_le_bytes(pairs[at..at + 8].try_into().expect("8 bytes")) as usize;
        if pair_len < 4 || at + 8 + pair_len > pairs.len() {
            break;
        }
        ids.push(u32::from_le_bytes(
            pairs[at + 8..at + 12].try_into().expect("4 bytes"),
        ));
        at += 8 + pair_len;
    }
    Ok(ids)
}

/// One APK of a package, as `pm path` reports it.
///
/// Serialized in camelCase: this is read by the panel's JavaScript, where every other app-list
/// field is already written that way.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApkFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// The base APK, or one of the splits an App Bundle installs beside it.
    pub is_base: bool,
}

/// Parse `pm path <pkg>` output: one `package:<path>` line per APK.
pub fn parse_pm_paths(stdout: &str) -> Vec<ApkFile> {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("package:"))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| ApkFile {
            name: Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string()),
            is_base: Path::new(path)
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("base.apk")),
            size: fs::metadata(path).map_or(0, |md| md.len()),
            path: path.to_string(),
        })
        .collect()
}

/// The two directories the detail screen lists, and where tapping one should go.
///
/// Both are offered whether or not they exist yet: the row is a jump target, and a folder that
/// is not there is something the file manager itself can say plainly.
pub fn data_dirs(package: &str) -> (String, String) {
    (
        format!("/data/user/0/{package}"),
        format!("/storage/emulated/0/Android/data/{package}"),
    )
}

/// Where an extracted APK lands: a file for a single APK, a folder when the app is split.
///
/// The name is built from the package (and the label the caller passed, when it is usable as a
/// file name), never from anything that could climb out of the destination.
pub fn extraction_targets(
    dest: &Path,
    package: &str,
    label: &str,
    version_code: i64,
    apks: &[ApkFile],
) -> Result<Vec<(PathBuf, PathBuf)>> {
    if apks.is_empty() {
        anyhow::bail!("没有找到 {package} 的 APK 文件");
    }
    let base = sanitize_name(label)
        .unwrap_or_else(|| sanitize_name(package).unwrap_or_else(|| "app".into()));
    let named = if version_code > 0 {
        format!("{base}_{version_code}")
    } else {
        base
    };

    if apks.len() == 1 {
        let file = apks[0].name.clone();
        return Ok(vec![(PathBuf::from(&apks[0].path), dest.join(file))]);
    }
    // A split install is only installable as a set, so it goes into a folder of its own.
    let folder = dest.join(named);
    Ok(apks
        .iter()
        .map(|apk| (PathBuf::from(&apk.path), folder.join(&apk.name)))
        .collect())
}

/// Keep a label usable as a file name without letting it point anywhere.
fn sanitize_name(text: &str) -> Option<String> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
            )
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() || cleaned == ".." {
        None
    } else {
        Some(cleaned)
    }
}

/// Everything the detail screen shows about one installed app.
///
/// Serialized in camelCase, and `serializes_the_names_the_panel_reads` below is the contract:
/// the JavaScript reads `versionName`/`dataDir1`, so a snake_case field here is a field the
/// panel silently renders as blank.
#[derive(Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppDetail {
    pub package: String,
    pub version_name: String,
    pub version_code: i64,
    pub target_sdk: i64,
    pub min_sdk: i64,
    pub uid: i64,
    pub data_dir: String,
    pub code_path: String,
    pub first_install: String,
    pub last_update: String,
    pub signatures: String,
    pub apk_files: Vec<ApkFile>,
    pub total_size: u64,
    /// 数据目录1 and 数据目录2: the app's own directory, and the one on shared storage.
    pub data_dir_1: String,
    pub data_dir_2: String,
}

/// Run a platform command and hand back its stdout, or the empty string when it cannot run.
///
/// `pm` and `dumpsys` are how the panel learns anything at all, so a command that is missing
/// or refused must not turn into a panic — the reader simply has fewer fields to show.
#[cfg(target_os = "android")]
fn shell_output(args: &[&str]) -> String {
    std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// Read every field the panel shows for one package.
#[cfg(target_os = "android")]
pub fn package_detail(package: &str) -> Result<AppDetail> {
    let dump = shell_output(&["dumpsys", "package", package]);
    let parsed = parse_dumpsys_package(&dump);
    let paths = parse_pm_paths(&shell_output(&["pm", "path", package]));

    // The base APK is what the schemes and the packer check describe; the splits ship beside it.
    let base = paths
        .iter()
        .find(|apk| apk.is_base)
        .or_else(|| paths.first())
        .map(|apk| PathBuf::from(&apk.path));

    let signatures = base.as_deref().map_or_else(
        || "无法读取".to_string(),
        |apk| match signature_schemes(apk) {
            Ok(schemes) => schemes.label(),
            Err(_) => "无法读取".to_string(),
        },
    );
    let (data_dir_1, data_dir_2) = data_dirs(package);

    Ok(AppDetail {
        package: if parsed.package.is_empty() {
            package.to_string()
        } else {
            parsed.package
        },
        version_name: parsed.version_name,
        version_code: parsed.version_code,
        target_sdk: parsed.target_sdk,
        min_sdk: parsed.min_sdk,
        uid: parsed.uid,
        data_dir: parsed.data_dir,
        code_path: parsed.code_path,
        first_install: parsed.first_install,
        last_update: parsed.last_update,
        signatures,
        total_size: paths.iter().map(|apk| apk.size).sum(),
        apk_files: paths,
        data_dir_1,
        data_dir_2,
    })
}

#[cfg(not(target_os = "android"))]
pub fn package_detail(_package: &str) -> Result<AppDetail> {
    anyhow::bail!("当前平台不支持读取应用信息")
}

/// Copy an app's APK files into `dest`, base and splits alike. Returns what was written.
#[cfg(target_os = "android")]
pub fn extract_package(package: &str, dest: &str, label: &str) -> Result<Vec<String>> {
    let dest = Path::new(dest);
    // Created on demand: 提取安装包 points at a folder of its own, which does not exist until
    // something is extracted into it.
    fs::create_dir_all(dest).with_context(|| format!("创建目录 {} 失败", dest.display()))?;
    if !dest.is_dir() {
        anyhow::bail!("目标不是目录：{}", dest.display());
    }

    let paths = parse_pm_paths(&shell_output(&["pm", "path", package]));
    let version_code =
        parse_dumpsys_package(&shell_output(&["dumpsys", "package", package])).version_code;
    let targets = extraction_targets(dest, package, label, version_code, &paths)?;

    let mut written = Vec::new();
    for (from, to) in targets {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
        }
        fs::copy(&from, &to)
            .with_context(|| format!("复制 {} 到 {} 失败", from.display(), to.display()))?;
        written.push(to.display().to_string());
    }
    Ok(written)
}

#[cfg(not(target_os = "android"))]
pub fn extract_package(_package: &str, _dest: &str, _label: &str) -> Result<Vec<String>> {
    anyhow::bail!("当前平台不支持提取安装包")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `dumpsys package com.tencent.mobileqq`, with the field order and the
    /// two-pairs-on-one-line shape left as Android writes them.
    const DUMPSYS: &str = r#"
Packages:
  Package [com.tencent.mobileqq] (a1b2c3):
    userId=10358
    pkg=Package{9f0abb4 com.tencent.mobileqq}
    codePath=/data/app/~~gX2YdtBfhnBSOFWyJ==/com.tencent.mobileqq-xyz==
    resourcePath=/data/app/~~gX2YdtBfhnBSOFWyJ==/com.tencent.mobileqq-xyz==
    versionName=9.3.55
    versionCode=15900 minSdk=23 targetSdk=34
    dataDir=/data/user/0/com.tencent.mobileqq
    primaryCpuAbi=arm64-v8a
    firstInstallTime=2026-09-13 22:51:27
    lastUpdateTime=2026-09-13 22:51:27
    User 0: ceDataInode=123 installed=true hidden=false
"#;

    #[test]
    fn reads_the_fields_out_of_a_dumpsys_package() {
        let details = parse_dumpsys_package(DUMPSYS);
        assert_eq!(details.package, "com.tencent.mobileqq");
        assert_eq!(details.version_name, "9.3.55");
        assert_eq!(details.version_code, 15900);
        assert_eq!(details.min_sdk, 23, "two pairs on one line");
        assert_eq!(details.target_sdk, 34);
        assert_eq!(details.uid, 10358);
        assert_eq!(details.data_dir, "/data/user/0/com.tencent.mobileqq");
        assert_eq!(details.first_install, "2026-09-13 22:51:27");
        assert_eq!(details.last_update, "2026-09-13 22:51:27");
    }

    #[test]
    fn an_empty_or_unknown_dump_does_not_panic() {
        assert_eq!(parse_dumpsys_package("").package, "");
        let odd = parse_dumpsys_package("Package [com.x] (1):\n  versionCode=N/A\n");
        assert_eq!(odd.version_code, 0, "an unparsable number reads as unknown");
    }

    /// The panel reads these exact keys; a serde rename here is what once made every
    /// multi-word field of the detail screen come back blank.
    #[test]
    fn serializes_the_names_the_panel_reads() {
        let json = serde_json::to_value(AppDetail {
            package: "com.x".into(),
            version_name: "1.2".into(),
            version_code: 7,
            target_sdk: 34,
            min_sdk: 23,
            uid: 10123,
            data_dir: "/data/user/0/com.x".into(),
            code_path: "/data/app/x".into(),
            first_install: "2026-09-13 22:51:27".into(),
            last_update: "2026-09-13 22:51:27".into(),
            signatures: "V1 + V2 + V3".into(),
            apk_files: vec![ApkFile {
                path: "/data/app/x/base.apk".into(),
                name: "base.apk".into(),
                size: 1,
                is_base: true,
            }],
            total_size: 1,
            data_dir_1: "/data/user/0/com.x".into(),
            data_dir_2: "/storage/emulated/0/Android/data/com.x".into(),
        })
        .expect("serialize");

        for key in [
            "package", "versionName", "versionCode", "targetSdk", "minSdk", "uid",
            "dataDir", "codePath", "firstInstall", "lastUpdate", "signatures",
            "apkFiles", "totalSize", "dataDir1", "dataDir2",
        ] {
            assert!(json.get(key).is_some(), "the panel reads `{key}`, which is missing");
        }
        assert!(
            json["apkFiles"][0].get("isBase").is_some(),
            "split detection is read per APK"
        );
    }

    #[test]
    fn reads_the_apk_files_out_of_pm_path() {
        let stdout = "package:/data/app/~~x==/com.x-y==/base.apk\npackage:/data/app/~~x==/com.x-y==/split_config.arm64_v8a.apk\n";
        let files = parse_pm_paths(stdout);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "base.apk");
        assert!(files[0].is_base);
        assert!(!files[1].is_base);
    }

    /// A single APK is written as one file; a split install needs its folder, because the
    /// pieces are only installable together.
    #[test]
    fn picks_names_that_cannot_leave_the_destination() {
        let dest = Path::new("/sdcard/Download");
        let one = vec![ApkFile {
            path: "/data/app/x/base.apk".into(),
            name: "base.apk".into(),
            size: 1,
            is_base: true,
        }];
        let targets = extraction_targets(dest, "com.x", "QQ", 15900, &one).expect("single");
        assert_eq!(targets[0].1, dest.join("base.apk"));

        let many = vec![
            ApkFile {
                path: "/d/base.apk".into(),
                name: "base.apk".into(),
                size: 1,
                is_base: true,
            },
            ApkFile {
                path: "/d/split_a.apk".into(),
                name: "split_a.apk".into(),
                size: 1,
                is_base: false,
            },
        ];
        let targets = extraction_targets(dest, "com.x", "QQ", 15900, &many).expect("split");
        assert_eq!(targets[0].1, dest.join("QQ_15900").join("base.apk"));
        assert_eq!(targets[1].1, dest.join("QQ_15900").join("split_a.apk"));

        // A label full of separators is not allowed to escape the destination.
        let evil = vec![ApkFile {
            path: "/d/base.apk".into(),
            name: "base.apk".into(),
            size: 1,
            is_base: true,
        }];
        let targets =
            extraction_targets(dest, "com.x", "../../etc/passwd", 0, &evil).expect("evil");
        assert!(
            targets[0].1.starts_with(dest),
            "escaped: {:?}",
            targets[0].1
        );
    }

    /// The schemes are read out of real APKs: the ones this repository builds.
    #[test]
    fn reads_signature_schemes_from_real_apks() {
        let manager = Path::new("../dist/manager");
        let Some(apk) = fs::read_dir(manager).ok().and_then(|dir| {
            dir.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "apk"))
        }) else {
            eprintln!("skipping: no built manager APK to inspect");
            return;
        };
        let schemes = signature_schemes(&apk).expect("read schemes");
        assert!(
            schemes.v1 || schemes.v2 || schemes.v3,
            "no scheme found in {apk:?}"
        );
        assert!(!schemes.label().is_empty());
    }

    #[test]
    fn reports_a_missing_signing_block_as_no_v2_or_v3() {
        // A plain zip has entries but no signing block; only V1 can be present, and here it is
        // not either.
        let dir = std::env::temp_dir().join(format!("ksu-apps-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("plain.zip");
        {
            let file = fs::File::create(&path).expect("create");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("classes.dex", zip::write::SimpleFileOptions::default())
                .expect("start");
            use std::io::Write as _;
            writer.write_all(b"not a real dex").expect("write");
            writer.finish().expect("finish");
        }
        let schemes = signature_schemes(&path).expect("read schemes");
        assert_eq!(schemes, SignatureSchemes::default());
        let _ = fs::remove_file(&path);
    }
}
