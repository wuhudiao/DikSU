
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

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

pub fn parse_dumpsys_package(text: &str) -> PackageDetails {
    let mut out = PackageDetails::default();

    for line in text.lines() {
        let line = line.trim();
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

#[derive(Default, Serialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct SignatureSchemes {
    pub v1: bool,
    pub v2: bool,
    pub v3: bool,
}

impl SignatureSchemes {
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
            0xf053_68c0 | 0x1b93_ad61 => out.v3 = true,
            _ => {}
        }
    }
    Ok(out)
}

fn signing_block_ids(apk: &Path) -> Result<Vec<u32>> {
    const EOCD: u32 = 0x0605_4b50;
    const MAGIC: &[u8] = b"APK Sig Block 42";

    let mut file = fs::File::open(apk).with_context(|| format!("打开 {} 失败", apk.display()))?;
    let len = file.metadata()?.len();
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

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApkFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_base: bool,
}

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

pub fn data_dirs(package: &str) -> (String, String) {
    (
        format!("/data/user/0/{package}"),
        format!("/storage/emulated/0/Android/data/{package}"),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExtractionTarget {
    Apk { from: PathBuf, to: PathBuf },
    Apks { to: PathBuf, parts: Vec<PathBuf> },
}

pub fn extraction_targets(
    dest: &Path,
    package: &str,
    label: &str,
    version_code: i64,
    apks: &[ApkFile],
) -> Result<ExtractionTarget> {
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
        return Ok(ExtractionTarget::Apk {
            from: PathBuf::from(&apks[0].path),
            to: dest.join(file),
        });
    }
    Ok(ExtractionTarget::Apks {
        to: dest.join(format!("{named}.apks")),
        parts: apks.iter().map(|apk| PathBuf::from(&apk.path)).collect(),
    })
}

fn write_apks(to: &Path, parts: &[PathBuf]) -> Result<()> {
    let file = fs::File::create(to).with_context(|| format!("创建 {} 失败", to.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for part in parts {
        let name = part
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "split.apk".to_string());
        let mut source =
            fs::File::open(part).with_context(|| format!("读取 {} 失败", part.display()))?;
        zip.start_file(name, options)?;
        std::io::copy(&mut source, &mut zip)
            .with_context(|| format!("打包 {} 失败", part.display()))?;
    }
    zip.finish().with_context(|| format!("收尾 {} 失败", to.display()))?;
    Ok(())
}

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
    pub data_dir_1: String,
    pub data_dir_2: String,
}

#[cfg(target_os = "android")]
fn shell_output(args: &[&str]) -> String {
    std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "android")]
pub fn package_detail(package: &str) -> Result<AppDetail> {
    let dump = shell_output(&["dumpsys", "package", package]);
    let parsed = parse_dumpsys_package(&dump);
    let paths = parse_pm_paths(&shell_output(&["pm", "path", package]));

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

#[cfg(target_os = "android")]
pub fn extract_package(package: &str, dest: &str, label: &str) -> Result<Vec<String>> {
    let dest = Path::new(dest);
    fs::create_dir_all(dest).with_context(|| format!("创建目录 {} 失败", dest.display()))?;
    if !dest.is_dir() {
        anyhow::bail!("目标不是目录：{}", dest.display());
    }

    let paths = parse_pm_paths(&shell_output(&["pm", "path", package]));
    let version_code =
        parse_dumpsys_package(&shell_output(&["dumpsys", "package", package])).version_code;
    let mut written = Vec::new();
    match extraction_targets(dest, package, label, version_code, &paths)? {
        ExtractionTarget::Apk { from, to } => {
            fs::copy(&from, &to)
                .with_context(|| format!("复制 {} 到 {} 失败", from.display(), to.display()))?;
            written.push(to.display().to_string());
        }
        ExtractionTarget::Apks { to, parts } => {
            write_apks(&to, &parts)?;
            written.push(to.display().to_string());
        }
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

    #[test]
    fn picks_names_that_cannot_leave_the_destination() {
        let dest = Path::new("/sdcard/Download");
        let one = vec![ApkFile {
            path: "/data/app/x/base.apk".into(),
            name: "base.apk".into(),
            size: 1,
            is_base: true,
        }];
        let target = extraction_targets(dest, "com.x", "QQ", 15900, &one).expect("single");
        assert_eq!(
            target,
            ExtractionTarget::Apk {
                from: PathBuf::from("/data/app/x/base.apk"),
                to: dest.join("base.apk"),
            }
        );

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
        let target = extraction_targets(dest, "com.x", "QQ", 15900, &many).expect("split");
        assert_eq!(
            target,
            ExtractionTarget::Apks {
                to: dest.join("QQ_15900.apks"),
                parts: vec![
                    PathBuf::from("/d/base.apk"),
                    PathBuf::from("/d/split_a.apk"),
                ],
            }
        );

        let evil = vec![ApkFile {
            path: "/d/base.apk".into(),
            name: "base.apk".into(),
            size: 1,
            is_base: true,
        }];
        let target =
            extraction_targets(dest, "com.x", "../../etc/passwd", 0, &evil).expect("evil");
        let to = match target {
            ExtractionTarget::Apk { to, .. } | ExtractionTarget::Apks { to, .. } => to,
        };
        assert!(
            to.starts_with(dest),
            "escaped: {:?}",
            to
        );
    }

    #[test]
    fn writes_an_apks_that_holds_every_piece() {
        let dir = std::env::temp_dir().join(format!("ksu-apks-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let base = dir.join("base.apk");
        let split = dir.join("split_config.arm64_v8a.apk");
        fs::write(&base, b"base-bytes").expect("write base");
        fs::write(&split, b"split-bytes").expect("write split");

        let out = dir.join("QQ_15900.apks");
        write_apks(&out, &[base, split]).expect("pack");

        let file = fs::File::open(&out).expect("open");
        let mut zip = zip::ZipArchive::new(file).expect("read zip");
        let mut names: Vec<String> = (0..zip.len())
            .map(|index| zip.by_index(index).expect("entry").name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "base.apk".to_string(),
                "split_config.arm64_v8a.apk".to_string()
            ]
        );
        let mut body = String::new();
        zip.by_name("split_config.arm64_v8a.apk")
            .expect("split entry")
            .read_to_string(&mut body)
            .expect("read split");
        assert_eq!(body, "split-bytes");

        let _ = fs::remove_dir_all(&dir);
    }

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
