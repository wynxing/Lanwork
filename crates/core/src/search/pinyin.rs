//! 构建期生成的拼音表。查询时只按码位取已存的音节 id。

include!(concat!(env!("OUT_DIR"), "/pinyin_table.rs"));

static SYLLABLE_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syllables.bin"));
static INDEX_EXT_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index_ext_a.bin"));
static INDEX_BASIC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index_basic.bin"));
static READING_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/readings.bin"));

const EXT_A_START: u32 = 0x3400;
const EXT_A_LEN: usize = (0x4DBF - 0x3400 + 1) as usize;
const BASIC_START: u32 = 0x4E00;
const BASIC_LEN: usize = (0x9FFF - 0x4E00 + 1) as usize;

/// 拼音表的来源和体积。体积是映射进进程的生成数据字节数，不是 Private Bytes。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableInfo {
    pub unicode_version: &'static str,
    pub source_url: &'static str,
    pub readings_sha256: &'static str,
    pub coverage: &'static str,
    pub syllable_count: usize,
    pub character_count: usize,
    pub resident_bytes: usize,
}

pub fn table_info() -> TableInfo {
    TableInfo {
        unicode_version: UNICODE_VERSION,
        source_url: SOURCE_URL,
        readings_sha256: READINGS_SHA256,
        coverage: COVERAGE,
        syllable_count: SYLLABLE_COUNT,
        character_count: CHARACTER_COUNT,
        resident_bytes: resident_bytes(),
    }
}

/// 生成表在 64 位进程里占用的数据字节数。
#[must_use]
pub fn resident_bytes() -> usize {
    SYLLABLE_BYTES.len()
        + std::mem::size_of_val(&SYLLABLE_OFFSETS)
        + INDEX_EXT_A.len()
        + INDEX_BASIC.len()
        + READING_BLOB.len()
}

/// 某个字的全部无声调读音。不在覆盖范围内或表内没有读音时返回空切片的空 `Vec`，不报错。
#[must_use]
pub fn readings(ch: char) -> Vec<&'static str> {
    let mut ids = Vec::new();
    if !push_reading_ids(ch, &mut ids) {
        return Vec::new();
    }
    ids.into_iter().map(syllable).collect()
}

pub(crate) fn push_reading_ids(ch: char, out: &mut Vec<u16>) -> bool {
    let Some(offset) = reading_offset(ch) else {
        return false;
    };
    let count = READING_BLOB[offset] as usize;
    let base = offset + 1;
    for index in 0..count {
        let pos = base + index * 2;
        let id = u16::from_le_bytes([READING_BLOB[pos], READING_BLOB[pos + 1]]);
        out.push(id);
    }
    count > 0
}

pub(crate) fn syllable(id: u16) -> &'static str {
    let index = usize::from(id);
    let start = SYLLABLE_OFFSETS[index] as usize;
    let end = SYLLABLE_OFFSETS[index + 1] as usize;
    std::str::from_utf8(&SYLLABLE_BYTES[start..end]).expect("syllable bytes are UTF-8")
}

fn reading_offset(ch: char) -> Option<usize> {
    let cp = ch as u32;
    let encoded = if (EXT_A_START..EXT_A_START + EXT_A_LEN as u32).contains(&cp) {
        u32_at(INDEX_EXT_A, (cp - EXT_A_START) as usize)
    } else if (BASIC_START..BASIC_START + BASIC_LEN as u32).contains(&cp) {
        u32_at(INDEX_BASIC, (cp - BASIC_START) as usize)
    } else {
        return None;
    };
    if encoded == u32::MAX {
        None
    } else {
        Some(encoded as usize)
    }
}

fn u32_at(bytes: &[u8], index: usize) -> u32 {
    let pos = index * 4;
    u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
}
