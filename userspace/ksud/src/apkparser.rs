//! Minimal APK parser to extract app label without Android framework APIs.
//! Parses binary AndroidManifest.xml and resources.arsc directly.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::io::{Read, Seek};

// ---------- little-endian readers ----------
const fn rd_u16(b: &[u8], o: &mut usize) -> Option<u16> {
    if *o + 2 > b.len() {
        return None;
    }
    let v = u16::from_le_bytes([b[*o], b[*o + 1]]);
    *o += 2;
    Some(v)
}
const fn rd_u32(b: &[u8], o: &mut usize) -> Option<u32> {
    if *o + 4 > b.len() {
        return None;
    }
    let v = u32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
    *o += 4;
    Some(v)
}
const fn rd_u8(b: &[u8], o: &mut usize) -> Option<u8> {
    if *o + 1 > b.len() {
        return None;
    }
    let v = b[*o];
    *o += 1;
    Some(v)
}

// ---------- binary XML string pool ----------
struct StringPool {
    strings: Vec<String>,
}

fn parse_string_pool(b: &[u8], start: usize) -> Option<StringPool> {
    let mut o = start;
    let _chunk_type = rd_u16(b, &mut o)?;
    let _header_size = rd_u16(b, &mut o)?;
    let _chunk_size = rd_u32(b, &mut o)?;
    let string_count = rd_u32(b, &mut o)? as usize;
    let _style_count = rd_u32(b, &mut o)?;
    let flags = rd_u32(b, &mut o)?;
    let strings_start = rd_u32(b, &mut o)? as usize;
    let _styles_start = rd_u32(b, &mut o)?;
    let is_utf8 = (flags & (1 << 8)) != 0;

    let mut offsets = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        offsets.push(rd_u32(b, &mut o)? as usize);
    }
    let strings_base = start + strings_start;
    let mut strings = Vec::with_capacity(string_count);
    for &offset in &offsets {
        let mut p = strings_base + offset;
        let s = if is_utf8 {
            let _clen = decode_len8(b, &mut p);
            let blen = decode_len8(b, &mut p).unwrap_or(0);
            if p + blen > b.len() {
                String::new()
            } else {
                String::from_utf8_lossy(&b[p..p + blen]).to_string()
            }
        } else {
            let clen = decode_len16(b, &mut p).unwrap_or(0);
            let bytes = clen * 2;
            if p + bytes > b.len() {
                String::new()
            } else {
                let mut u16s = Vec::with_capacity(clen);
                for _ in 0..clen {
                    if p + 2 > b.len() {
                        break;
                    }
                    u16s.push(u16::from_le_bytes([b[p], b[p + 1]]));
                    p += 2;
                }
                String::from_utf16_lossy(&u16s)
            }
        };
        strings.push(s);
    }
    Some(StringPool { strings })
}

const fn decode_len8(b: &[u8], p: &mut usize) -> Option<usize> {
    if *p >= b.len() {
        return None;
    }
    let mut v = b[*p] as usize;
    *p += 1;
    if v & 0x80 != 0 {
        if *p >= b.len() {
            return None;
        }
        v = ((v & 0x7f) << 8) | b[*p] as usize;
        *p += 1;
    }
    Some(v)
}
fn decode_len16(b: &[u8], p: &mut usize) -> Option<usize> {
    let mut v = rd_u16(b, p)? as usize;
    if v & 0x8000 != 0 {
        let hi = rd_u16(b, p)? as usize;
        v = ((v & 0x7fff) << 16) | hi;
    }
    Some(v)
}

impl StringPool {
    fn get(&self, idx: u32) -> Option<&str> {
        if idx == u32::MAX {
            return None;
        }
        self.strings.get(idx as usize).map(String::as_str)
    }
}

// ---------- walk AndroidManifest.xml ----------

/// Hand every attribute of every start element to `visit`, which returns `Some` to stop the walk
/// with that answer.
///
/// A `ResXMLTree_node` opens with the line number and the comment slot before the element's own
/// fields, so those four bytes of each are stepped over here rather than mistaken for the
/// namespace and name.
fn find_attribute<T>(
    data: &[u8],
    mut visit: impl FnMut(&StringPool, &str, &str, u32, u8, u32) -> Option<T>,
) -> Option<T> {
    let mut o = 0;
    let _ftype = rd_u16(data, &mut o)?;
    let _fhsize = rd_u16(data, &mut o)?;
    let _fsize = rd_u32(data, &mut o)?;

    let mut pool: Option<StringPool> = None;
    while o + 8 <= data.len() {
        let chunk_start = o;
        let ctype = rd_u16(data, &mut o)?;
        let _chsize = rd_u16(data, &mut o)?;
        let csize = rd_u32(data, &mut o)? as usize;
        if csize == 0 {
            break;
        }
        if ctype == 0x0001 {
            pool = parse_string_pool(data, chunk_start);
        } else if ctype == 0x0102 {
            let pool = pool.as_ref()?;
            let _line = rd_u32(data, &mut o)?;
            let _comment = rd_u32(data, &mut o)?;
            let _ns = rd_u32(data, &mut o)?;
            let name_idx = rd_u32(data, &mut o)?;
            let elem_name = pool.get(name_idx).unwrap_or("");
            let attr_start = rd_u16(data, &mut o)? as usize;
            let _attr_size = rd_u16(data, &mut o)?;
            let attr_count = rd_u16(data, &mut o)?;
            let _id_idx = rd_u16(data, &mut o)?;
            let _class_idx = rd_u16(data, &mut o)?;
            let _style_idx = rd_u16(data, &mut o)?;
            // `attributeStart` is measured from the attrExt block, which begins at the node's
            // header — 16 bytes of it are fixed.
            let attrs = chunk_start + 16 + attr_start;
            for i in 0..attr_count {
                // ResXMLTree_attribute: ns, name, rawValue, then Res_value's size, res0, dataType
                // and data — 20 bytes, one after another.
                let mut a = attrs + i as usize * 20;
                let _a_ns = rd_u32(data, &mut a)?;
                let a_name = rd_u32(data, &mut a)?;
                let a_raw = rd_u32(data, &mut a)?;
                let _a_size = rd_u16(data, &mut a)?;
                let _res0 = rd_u8(data, &mut a)?;
                let a_dtype = rd_u8(data, &mut a)?;
                let a_data = rd_u32(data, &mut a)?;
                let attr_name = pool.get(a_name).unwrap_or("");
                if let Some(hit) = visit(pool, elem_name, attr_name, a_raw, a_dtype, a_data) {
                    return Some(hit);
                }
            }
        }
        o = chunk_start + csize;
    }
    None
}

/// The label an APK's `<application>` carries, either spelled out or as a resource id.
struct LabelResult {
    str_value: Option<String>,
    ref_id: Option<u32>,
}

fn parse_manifest_label(data: &[u8]) -> Option<LabelResult> {
    find_attribute(data, |pool, elem, attr, a_raw, a_dtype, a_data| {
        if elem != "application" || attr != "label" {
            return None;
        }
        Some(if a_dtype == 0x03 {
            LabelResult {
                str_value: pool.get(a_data).map(str::to_string),
                ref_id: None,
            }
        } else if a_dtype == 0x01 {
            LabelResult {
                str_value: None,
                ref_id: Some(a_data),
            }
        } else if a_raw != u32::MAX {
            LabelResult {
                str_value: pool.get(a_raw).map(str::to_string),
                ref_id: None,
            }
        } else {
            LabelResult {
                str_value: None,
                ref_id: None,
            }
        })
    })
}

/// The `package` attribute of `<manifest>`, which is always a plain string.
fn parse_manifest_package(data: &[u8]) -> Option<String> {
    find_attribute(data, |pool, elem, attr, _a_raw, a_dtype, a_data| {
        if elem == "manifest" && attr == "package" && a_dtype == 0x03 {
            return pool.get(a_data).map(str::to_string);
        }
        None
    })
}

// ---------- resources.arsc parser ----------
struct ArscTable {
    values: StringPool,
    entries: std::collections::HashMap<u32, u32>, // res_id -> value string index
}

fn parse_arsc(data: &[u8]) -> Option<ArscTable> {
    let mut o = 0;
    let ttype = rd_u16(data, &mut o)?;
    let _thsize = rd_u16(data, &mut o)?;
    let _tsize = rd_u32(data, &mut o)?;
    if ttype != 0x0002 {
        return None;
    }
    let _pkg_count = rd_u32(data, &mut o)?;

    let mut values: Option<StringPool> = None;
    let mut entries = std::collections::HashMap::new();

    while o + 8 <= data.len() {
        let chunk_start = o;
        let ctype = rd_u16(data, &mut o)?;
        let _chsize = rd_u16(data, &mut o)?;
        let csize = rd_u32(data, &mut o)? as usize;
        if csize == 0 {
            break;
        }
        if ctype == 0x0001 {
            values = parse_string_pool(data, chunk_start);
        } else if ctype == 0x0200 {
            scan_package(data, chunk_start, csize, &mut o, &mut entries);
        }
        o = chunk_start + csize;
    }

    Some(ArscTable {
        values: values?,
        entries,
    })
}

/// Walk a ResTable_package chunk and harvest string entries from each ResTable_type.
fn scan_package(
    data: &[u8],
    chunk_start: usize,
    csize: usize,
    o: &mut usize,
    entries: &mut std::collections::HashMap<u32, u32>,
) {
    let Some(pkg_id) = rd_u32(data, o) else {
        return;
    };
    *o += 256 * 2; // package name stored as 256 u16
    let _type_strings_off = rd_u32(data, o);
    let _last_public_type = rd_u32(data, o);
    let _key_strings_off = rd_u32(data, o);
    let _last_public_key = rd_u32(data, o);
    let _type_id_offset = rd_u32(data, o);
    let pkg_end = chunk_start + csize;
    let mut po = *o;
    while po + 8 <= pkg_end {
        let sub_start = po;
        let Some(stype) = rd_u16(data, &mut po) else {
            break;
        };
        let Some(sheader_size) = rd_u16(data, &mut po) else {
            break;
        };
        let Some(ssize) = rd_u32(data, &mut po) else {
            break;
        };
        let ssize = ssize as usize;
        if ssize == 0 {
            break;
        }
        if stype == 0x0201 {
            collect_type_entries(data, sub_start, sheader_size as usize, pkg_id, entries);
        }
        po = sub_start + ssize;
    }
    *o = po;
}

/// Read one ResTable_type chunk and insert its simple string values into `entries`.
fn collect_type_entries(
    data: &[u8],
    sub_start: usize,
    sheader_size: usize,
    pkg_id: u32,
    entries: &mut std::collections::HashMap<u32, u32>,
) {
    // After the 8-byte chunk header: u8 id, u8 res0, u16 res1,
    // u32 entryCount, u32 entriesStart, then a config block.
    if sub_start + 20 > data.len() {
        return;
    }
    let type_id = u32::from(data[sub_start + 8]);
    let entry_count = u32::from_le_bytes([
        data[sub_start + 12],
        data[sub_start + 13],
        data[sub_start + 14],
        data[sub_start + 15],
    ]);
    let entries_start = u32::from_le_bytes([
        data[sub_start + 16],
        data[sub_start + 17],
        data[sub_start + 18],
        data[sub_start + 19],
    ]) as usize;
    let offsets_base = sub_start + sheader_size;
    for e in 0..entry_count {
        let off_pos = offsets_base + (e as usize) * 4;
        if off_pos + 4 > data.len() {
            break;
        }
        let entry_off = u32::from_le_bytes([
            data[off_pos],
            data[off_pos + 1],
            data[off_pos + 2],
            data[off_pos + 3],
        ]) as usize;
        if entry_off == u32::MAX as usize {
            continue;
        }
        let entry_pos = sub_start + entries_start + entry_off;
        if entry_pos + 8 > data.len() {
            continue;
        }
        let eflags = u16::from_le_bytes([data[entry_pos + 2], data[entry_pos + 3]]);
        if eflags & 0x0001 != 0 {
            continue; // complex entry (array/style), not a plain value
        }
        let val_pos = entry_pos + 8;
        if val_pos + 8 > data.len() {
            continue;
        }
        let dtype = data[val_pos + 3];
        let ddata = u32::from_le_bytes([
            data[val_pos + 4],
            data[val_pos + 5],
            data[val_pos + 6],
            data[val_pos + 7],
        ]);
        if dtype == 0x03 {
            let res_id = (pkg_id << 24) | (type_id << 16) | e;
            entries.insert(res_id, ddata);
        }
    }
}

impl ArscTable {
    fn get_string(&self, res_id: u32) -> Option<String> {
        // try exact id, then same type/entry with package 0x7f
        let idx = self.entries.get(&res_id).copied().or_else(|| {
            self.entries
                .get(&(0x7f000000 | (res_id & 0x00ffffff)))
                .copied()
        })?;
        self.values.get(idx).map(ToString::to_string)
    }
}

/// The APK's binary `AndroidManifest.xml`, read whole.
fn read_manifest<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Option<Vec<u8>> {
    let mut manifest = Vec::new();
    archive
        .by_name("AndroidManifest.xml")
        .ok()?
        .read_to_end(&mut manifest)
        .ok()?;
    Some(manifest)
}

/// Extract the human-readable label for an APK given a readable+seekable file.
#[must_use]
pub fn extract_label<R: Read + Seek>(zip_file: R) -> Option<String> {
    let mut archive = zip::ZipArchive::new(zip_file).ok()?;
    let manifest = read_manifest(&mut archive)?;
    let label = parse_manifest_label(&manifest)?;
    if let Some(s) = label.str_value {
        if !s.is_empty() && !s.starts_with('@') {
            return Some(s);
        }
    }
    let ref_id = label.ref_id?;

    let mut arsc = Vec::new();
    {
        let mut f = archive.by_name("resources.arsc").ok()?;
        f.read_to_end(&mut arsc).ok()?;
    }
    let table = parse_arsc(&arsc)?;
    let s = table.get_string(ref_id)?;
    (!s.is_empty()).then_some(s)
}

/// The package name an APK declares, given a readable+seekable file.
///
/// The file manager needs this for an installer that is not installed yet: what `pm install`
/// will put on the device is written in the manifest, and nowhere else.
#[must_use]
pub fn extract_package<R: Read + Seek>(zip_file: R) -> Option<String> {
    let mut archive = zip::ZipArchive::new(zip_file).ok()?;
    let manifest = read_manifest(&mut archive)?;
    parse_manifest_package(&manifest).filter(|package| !package.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A chunk: its type, how big its header is, and its body.
    fn chunk(kind: u16, header: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&header.to_le_bytes());
        out.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A UTF-16 string pool — the format a manifest written by `aapt` uses here.
    fn string_pool(strings: &[&str]) -> Vec<u8> {
        let mut offsets = Vec::new();
        let mut data = Vec::new();
        for s in strings {
            offsets.push(data.len() as u32);
            let units: Vec<u16> = s.encode_utf16().collect();
            data.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for unit in units {
                data.extend_from_slice(&unit.to_le_bytes());
            }
            data.extend_from_slice(&0u16.to_le_bytes()); // the pool's terminator
        }

        let mut body = Vec::new();
        body.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // styleCount
        body.extend_from_slice(&0u32.to_le_bytes()); // flags: UTF-16
        body.extend_from_slice(&((28 + offsets.len() * 4) as u32).to_le_bytes()); // stringsStart
        body.extend_from_slice(&0u32.to_le_bytes()); // stylesStart
        for offset in &offsets {
            body.extend_from_slice(&offset.to_le_bytes());
        }
        body.extend_from_slice(&data);
        chunk(0x0001, 28, &body)
    }

    /// One `<element …/>`, whose attributes are `(name, dataType, data)` indexes into the pool.
    fn start_element(name: usize, attrs: &[(usize, u8, u32)]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_le_bytes()); // lineNumber
        body.extend_from_slice(&u32::MAX.to_le_bytes()); // comment
        body.extend_from_slice(&u32::MAX.to_le_bytes()); // ns
        body.extend_from_slice(&(name as u32).to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes()); // attributeStart
        body.extend_from_slice(&20u16.to_le_bytes()); // attributeSize
        body.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes()); // idIndex
        body.extend_from_slice(&0u16.to_le_bytes()); // classIndex
        body.extend_from_slice(&0u16.to_le_bytes()); // styleIndex
        for (attr, dtype, data) in attrs {
            body.extend_from_slice(&u32::MAX.to_le_bytes()); // ns
            body.extend_from_slice(&(*attr as u32).to_le_bytes());
            body.extend_from_slice(&u32::MAX.to_le_bytes()); // rawValue
            body.extend_from_slice(&8u16.to_le_bytes()); // Res_value size
            body.push(0); // res0
            body.push(*dtype);
            body.extend_from_slice(&data.to_le_bytes());
        }
        chunk(0x0102, 16, &body)
    }

    /// A manifest with the strings the elements below refer to.
    fn manifest(attrs: &[(usize, u8, u32)], with_label: bool) -> Vec<u8> {
        let strings = [
            "manifest",
            "package",
            "com.example.app",
            "application",
            "label",
            "示例",
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&0x0003u16.to_le_bytes()); // RES_XML_TYPE
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // the file's size, patched below
        out.extend_from_slice(&string_pool(&strings));
        out.extend_from_slice(&start_element(0, attrs));
        if with_label {
            out.extend_from_slice(&start_element(3, &[(4, 0x03, 5)]));
        }
        let size = out.len() as u32;
        out[4..8].copy_from_slice(&size.to_le_bytes());
        out
    }

    /// The manifest inside a zip, which is how both readers are handed a file.
    fn apk_with(manifest: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            writer
                .start_file(
                    "AndroidManifest.xml",
                    zip::write::SimpleFileOptions::default(),
                )
                .expect("start");
            writer.write_all(manifest).expect("write");
            writer.finish().expect("finish");
        }
        buf
    }

    #[test]
    fn reads_the_package_and_the_label_out_of_a_manifest() {
        let apk = apk_with(&manifest(&[(1, 0x03, 2)], true));
        assert_eq!(
            extract_package(std::io::Cursor::new(&apk)).as_deref(),
            Some("com.example.app")
        );
        assert_eq!(
            extract_label(std::io::Cursor::new(&apk)).as_deref(),
            Some("示例")
        );
    }

    /// The package attribute is what the reader is looking for, and nothing else in the manifest
    /// may stand in for it.
    #[test]
    fn a_manifest_without_a_package_answers_nothing() {
        let apk = apk_with(&manifest(&[], true));
        assert_eq!(extract_package(std::io::Cursor::new(&apk)), None);
    }
}
