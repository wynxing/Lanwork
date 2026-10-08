//! 从固定版本的 Unihan 读音摘录生成紧凑拼音表。
//!
//! 查询路径只读生成结果，不解析这份摘录。

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const READINGS_SHA256: &str = "a5ad0750009db5a4461efc9c66a87c359b9cf7e6e972a6172ba45c673556fa18";
const UNICODE_VERSION: &str = "18.0.0";
const SOURCE_URL: &str = "https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip";
const EXT_A_START: u32 = 0x3400;
const EXT_A_END: u32 = 0x4DBF;
const BASIC_START: u32 = 0x4E00;
const BASIC_END: u32 = 0x9FFF;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("../../third_party/unihan/kMandarin_kHanyuPinyin.txt");
    println!("cargo:rerun-if-changed={}", source.display());

    let bytes = std::fs::read(&source).unwrap_or_else(|err| {
        panic!("read {}: {err}", source.display());
    });
    let digest = hex_encode(&Sha256::digest(&bytes));
    if digest != READINGS_SHA256 {
        panic!(
            "kMandarin_kHanyuPinyin.txt SHA-256 is {digest}, expected {READINGS_SHA256}. \
             The pinned Unihan extract changed; update the license notes and this check together."
        );
    }
    let text = std::str::from_utf8(&bytes).expect("kMandarin_kHanyuPinyin.txt is UTF-8");
    if !text.contains("Unicode Version 18.0.0") {
        panic!("kMandarin_kHanyuPinyin.txt header is not Unicode 18.0.0");
    }

    let mut mandarin: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut hanyu: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let Some(cp_text) = parts.next() else {
            continue;
        };
        let Some(field) = parts.next() else {
            continue;
        };
        if field != "kMandarin" && field != "kHanyuPinyin" {
            continue;
        }
        let Some(value) = parts.next() else {
            panic!("line {}: missing value", line_no + 1);
        };
        if parts.next().is_some() {
            panic!("line {}: extra columns", line_no + 1);
        }
        let cp = parse_codepoint(cp_text, line_no + 1);
        if !in_coverage(cp) {
            continue;
        }
        let parsed = if field == "kMandarin" {
            parse_mandarin(value)
        } else {
            parse_hanyu(value)
        };
        let parsed = parsed.unwrap_or_else(|err| {
            panic!("line {} U+{cp:04X} {field}: {err}", line_no + 1);
        });
        let slot = if field == "kMandarin" {
            mandarin.entry(cp).or_default()
        } else {
            hanyu.entry(cp).or_default()
        };
        push_dedup(slot, parsed);
    }

    let mut readings: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let codepoints: Vec<u32> = mandarin.keys().chain(hanyu.keys()).copied().collect();
    for cp in codepoints {
        let mut merged = Vec::new();
        if let Some(values) = mandarin.get(&cp) {
            push_dedup(&mut merged, values.clone());
        }
        if let Some(values) = hanyu.get(&cp) {
            push_dedup(&mut merged, values.clone());
        }
        if !merged.is_empty() {
            readings.insert(cp, merged);
        }
    }

    must_contain(&readings, 0x91CD, &["zhong", "chong"]);
    must_contain(&readings, 0x884C, &["xing", "hang"]);

    let mut syllables: Vec<String> = Vec::new();
    let mut syllable_of: BTreeMap<String, u16> = BTreeMap::new();
    let mut ids_of: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    for (cp, values) in &readings {
        let mut ids = Vec::with_capacity(values.len());
        for value in values {
            let id = if let Some(id) = syllable_of.get(value) {
                *id
            } else {
                let id = u16::try_from(syllables.len()).expect("syllable count fits u16");
                syllable_of.insert(value.clone(), id);
                syllables.push(value.clone());
                id
            };
            ids.push(id);
        }
        if ids.len() > usize::from(u8::MAX) {
            panic!("U+{cp:04X} has {} readings", ids.len());
        }
        ids_of.insert(*cp, ids);
    }

    let mut syllable_bytes = Vec::new();
    let mut syllable_offsets = Vec::with_capacity(syllables.len() + 1);
    syllable_offsets.push(0u32);
    for syllable in &syllables {
        syllable_bytes.extend_from_slice(syllable.as_bytes());
        syllable_offsets.push(u32::try_from(syllable_bytes.len()).expect("syllable blob fits u32"));
    }

    let mut reading_blob = Vec::new();
    let mut index_ext_a = vec![u32::MAX; (EXT_A_END - EXT_A_START + 1) as usize];
    let mut index_basic = vec![u32::MAX; (BASIC_END - BASIC_START + 1) as usize];
    for (cp, ids) in &ids_of {
        let offset = u32::try_from(reading_blob.len()).expect("reading blob fits u32");
        reading_blob.push(u8::try_from(ids.len()).expect("reading count fits u8"));
        for id in ids {
            reading_blob.extend_from_slice(&id.to_le_bytes());
        }
        if (EXT_A_START..=EXT_A_END).contains(cp) {
            index_ext_a[(cp - EXT_A_START) as usize] = offset;
        } else {
            index_basic[(cp - BASIC_START) as usize] = offset;
        }
    }

    let resident_bytes = syllable_bytes.len()
        + syllable_offsets.len() * 4
        + index_ext_a.len() * 4
        + index_basic.len() * 4
        + reading_blob.len();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    write_bytes(&out_dir.join("syllables.bin"), &syllable_bytes);
    write_u32s(&out_dir.join("index_ext_a.bin"), &index_ext_a);
    write_u32s(&out_dir.join("index_basic.bin"), &index_basic);
    write_bytes(&out_dir.join("readings.bin"), &reading_blob);
    write_meta(
        &out_dir.join("pinyin_table.rs"),
        &syllable_offsets,
        syllables.len(),
        ids_of.len(),
        resident_bytes,
    );
}

fn write_meta(
    path: &std::path::Path,
    offsets: &[u32],
    syllable_count: usize,
    character_count: usize,
    resident_bytes: usize,
) {
    let mut file = BufWriter::new(File::create(path).expect("create pinyin_table.rs"));
    writeln!(file, "// Generated by crates/core/build.rs. Do not edit.").unwrap();
    writeln!(
        file,
        "pub const UNICODE_VERSION: &str = \"{UNICODE_VERSION}\";"
    )
    .unwrap();
    writeln!(file, "pub const SOURCE_URL: &str = \"{SOURCE_URL}\";").unwrap();
    writeln!(
        file,
        "pub const READINGS_SHA256: &str = \"{READINGS_SHA256}\";"
    )
    .unwrap();
    writeln!(
        file,
        "pub const COVERAGE: &str = \"U+3400..=U+4DBF, U+4E00..=U+9FFF\";"
    )
    .unwrap();
    writeln!(file, "pub const SYLLABLE_COUNT: usize = {syllable_count};").unwrap();
    writeln!(
        file,
        "pub const CHARACTER_COUNT: usize = {character_count};"
    )
    .unwrap();
    writeln!(
        file,
        "pub const TABLE_RESIDENT_BYTES: usize = {resident_bytes};"
    )
    .unwrap();
    writeln!(
        file,
        "pub static SYLLABLE_OFFSETS: [u32; {}] = [",
        offsets.len()
    )
    .unwrap();
    for offset in offsets {
        writeln!(file, "    {offset},").unwrap();
    }
    writeln!(file, "];").unwrap();
    file.flush().unwrap();
}

fn write_bytes(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

fn write_u32s(path: &std::path::Path, values: &[u32]) {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    write_bytes(path, &bytes);
}

fn must_contain(readings: &BTreeMap<u32, Vec<String>>, cp: u32, required: &[&str]) {
    let Some(got) = readings.get(&cp) else {
        panic!("U+{cp:04X} has no readings; do not drop the acceptance sample");
    };
    for reading in required {
        if !got.iter().any(|item| item == reading) {
            panic!(
                "U+{cp:04X} is missing `{reading}` after tone folding, got {got:?}. \
                 Record the data limit and extend the table; do not delete the sample."
            );
        }
    }
}

fn push_dedup(slot: &mut Vec<String>, readings: Vec<String>) {
    for reading in readings {
        if !slot.iter().any(|item| item == &reading) {
            slot.push(reading);
        }
    }
}

fn in_coverage(cp: u32) -> bool {
    (EXT_A_START..=EXT_A_END).contains(&cp) || (BASIC_START..=BASIC_END).contains(&cp)
}

fn parse_codepoint(text: &str, line_no: usize) -> u32 {
    let Some(hex) = text.strip_prefix("U+") else {
        panic!("line {line_no}: code point `{text}`");
    };
    u32::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("line {line_no}: code point `{text}`"))
}

fn parse_mandarin(value: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for reading in value.split(' ') {
        if reading.is_empty() {
            continue;
        }
        out.push(normalize_reading(reading)?);
    }
    if out.is_empty() {
        return Err(format!("empty kMandarin `{value}`"));
    }
    Ok(out)
}

fn parse_hanyu(value: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for group in value.split(' ') {
        if group.is_empty() {
            continue;
        }
        let Some((locations, readings)) = group.split_once(':') else {
            return Err(format!("missing colon in `{group}`"));
        };
        if locations.is_empty() || readings.is_empty() {
            return Err(format!("empty location or reading in `{group}`"));
        }
        for location in locations.split(',') {
            if !location_ok(location) {
                return Err(format!("bad location `{location}` in `{value}`"));
            }
        }
        for reading in readings.split(',') {
            if reading.is_empty() {
                return Err(format!("empty reading in `{group}`"));
            }
            out.push(normalize_reading(reading)?);
        }
    }
    if out.is_empty() {
        return Err(format!("empty kHanyuPinyin `{value}`"));
    }
    Ok(out)
}

fn location_ok(location: &str) -> bool {
    let Some((page, pos)) = location.split_once('.') else {
        return false;
    };
    !page.is_empty()
        && !pos.is_empty()
        && page.bytes().all(|byte| byte.is_ascii_digit())
        && pos.bytes().all(|byte| byte.is_ascii_digit())
}

fn normalize_reading(raw: &str) -> Result<String, String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if is_tone_mark(ch) {
            index += 1;
            continue;
        }
        let Some(mut base) = fold_letter(ch) else {
            return Err(format!("unexpected U+{:04X} in `{raw}`", ch as u32));
        };
        index += 1;
        while index < chars.len() && (is_tone_mark(chars[index]) || chars[index] == '\u{0308}') {
            if chars[index] == '\u{0308}' {
                if base != 'u' {
                    return Err(format!("diaeresis not on u in `{raw}`"));
                }
                base = 'v';
            }
            index += 1;
        }
        out.push(base);
    }
    if out.is_empty() {
        return Err(format!("empty reading `{raw}`"));
    }
    Ok(out)
}

fn is_tone_mark(ch: char) -> bool {
    matches!(
        ch,
        '\u{0300}' | '\u{0301}' | '\u{0302}' | '\u{0304}' | '\u{030C}'
    )
}

fn fold_letter(ch: char) -> Option<char> {
    const MAP: &[(char, char)] = &[
        ('ā', 'a'),
        ('á', 'a'),
        ('ǎ', 'a'),
        ('à', 'a'),
        ('ē', 'e'),
        ('é', 'e'),
        ('ě', 'e'),
        ('è', 'e'),
        ('ê', 'e'),
        ('ế', 'e'),
        ('ề', 'e'),
        ('ī', 'i'),
        ('í', 'i'),
        ('ǐ', 'i'),
        ('ì', 'i'),
        ('ō', 'o'),
        ('ó', 'o'),
        ('ǒ', 'o'),
        ('ò', 'o'),
        ('ū', 'u'),
        ('ú', 'u'),
        ('ǔ', 'u'),
        ('ù', 'u'),
        ('ü', 'v'),
        ('ǖ', 'v'),
        ('ǘ', 'v'),
        ('ǚ', 'v'),
        ('ǜ', 'v'),
        ('ń', 'n'),
        ('ň', 'n'),
        ('ǹ', 'n'),
        ('ḿ', 'm'),
    ];
    if let Some((_, folded)) = MAP.iter().find(|(from, _)| *from == ch) {
        return Some(*folded);
    }
    if ch.is_ascii_alphabetic() {
        return Some(ch.to_ascii_lowercase());
    }
    None
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
