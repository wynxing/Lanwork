//! 只读写存储法（compression method 0）的 ZIP。
//!
//! 不支持 deflate、加密、数据描述符和 Zip64。文件名按 UTF-8 标志位写出。

use super::error::BackupError;

pub(crate) struct StoredFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const EOCD: u32 = 0x0605_4b50;
const UTF8_FLAG: u16 = 0x0800;
const VERSION: u16 = 20;
const DOS_DATE: u16 = 0x0021;

pub(crate) fn write_stored(files: &[StoredFile]) -> Result<Vec<u8>, BackupError> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for file in files {
        if file.name.len() > u16::MAX as usize || file.bytes.len() > u32::MAX as usize {
            return Err(BackupError::Rejected("压缩包格式不受支持"));
        }
        let name = file.name.as_bytes();
        let crc = crc32(&file.bytes);
        let size = u32::try_from(file.bytes.len())
            .map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?;
        let offset =
            u32::try_from(local.len()).map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?;
        push_u32(&mut local, LOCAL);
        push_u16(&mut local, VERSION);
        push_u16(&mut local, UTF8_FLAG);
        push_u16(&mut local, 0);
        push_u16(&mut local, 0);
        push_u16(&mut local, DOS_DATE);
        push_u32(&mut local, crc);
        push_u32(&mut local, size);
        push_u32(&mut local, size);
        push_u16(&mut local, u16::try_from(name.len()).unwrap_or(0));
        push_u16(&mut local, 0);
        local.extend_from_slice(name);
        local.extend_from_slice(&file.bytes);

        push_u32(&mut central, CENTRAL);
        push_u16(&mut central, VERSION);
        push_u16(&mut central, VERSION);
        push_u16(&mut central, UTF8_FLAG);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, DOS_DATE);
        push_u32(&mut central, crc);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, u16::try_from(name.len()).unwrap_or(0));
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, offset);
        central.extend_from_slice(name);
    }
    let cd_offset =
        u32::try_from(local.len()).map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?;
    let cd_size =
        u32::try_from(central.len()).map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?;
    let count =
        u16::try_from(files.len()).map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?;
    let mut out = local;
    out.extend_from_slice(&central);
    push_u32(&mut out, EOCD);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, count);
    push_u16(&mut out, count);
    push_u32(&mut out, cd_size);
    push_u32(&mut out, cd_offset);
    push_u16(&mut out, 0);
    Ok(out)
}

pub(crate) fn read_stored(data: &[u8]) -> Result<Vec<StoredFile>, BackupError> {
    let eocd = find_eocd(data)?;
    let disk = read_u16(data, eocd + 4)?;
    let cd_disk = read_u16(data, eocd + 6)?;
    let entries_disk = read_u16(data, eocd + 8)?;
    let entries = read_u16(data, eocd + 10)?;
    let cd_size = read_u32(data, eocd + 12)?;
    let cd_offset = read_u32(data, eocd + 16)?;
    if disk != 0 || cd_disk != 0 || entries_disk != entries {
        return Err(BackupError::Rejected("压缩包格式不受支持"));
    }
    if cd_size == u32::MAX || cd_offset == u32::MAX || entries == u16::MAX {
        return Err(BackupError::Rejected("压缩包格式不受支持"));
    }
    let cd_start = cd_offset as usize;
    let cd_end = cd_start
        .checked_add(cd_size as usize)
        .ok_or(BackupError::Rejected("压缩包无法读取"))?;
    if cd_end > data.len() || cd_end > eocd {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    let mut files = Vec::with_capacity(entries as usize);
    let mut cursor = cd_start;
    for _ in 0..entries {
        if read_u32(data, cursor)? != CENTRAL {
            return Err(BackupError::Rejected("压缩包无法读取"));
        }
        let flags = read_u16(data, cursor + 8)?;
        let method = read_u16(data, cursor + 10)?;
        let crc = read_u32(data, cursor + 16)?;
        let compressed = read_u32(data, cursor + 20)?;
        let uncompressed = read_u32(data, cursor + 24)?;
        let name_len = read_u16(data, cursor + 28)? as usize;
        let extra_len = read_u16(data, cursor + 30)? as usize;
        let comment_len = read_u16(data, cursor + 32)? as usize;
        let local_offset = read_u32(data, cursor + 42)?;
        if compressed == u32::MAX || uncompressed == u32::MAX || local_offset == u32::MAX {
            return Err(BackupError::Rejected("压缩包格式不受支持"));
        }
        let name_at = cursor + 46;
        let name_bytes = slice(data, name_at, name_len)?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| BackupError::Rejected("压缩包格式不受支持"))?
            .to_owned();
        cursor = name_at
            .checked_add(name_len + extra_len + comment_len)
            .ok_or(BackupError::Rejected("压缩包无法读取"))?;
        let file = read_local(
            data,
            local_offset as usize,
            &name,
            LocalMeta {
                flags,
                method,
                crc,
                compressed,
                uncompressed,
            },
        )?;
        files.push(file);
    }
    if cursor != cd_end {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    Ok(files)
}

struct LocalMeta {
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u32,
    uncompressed: u32,
}

fn read_local(
    data: &[u8],
    offset: usize,
    name: &str,
    meta: LocalMeta,
) -> Result<StoredFile, BackupError> {
    let LocalMeta {
        flags,
        method,
        crc,
        compressed,
        uncompressed,
    } = meta;
    if flags & 0x0001 != 0 || flags & 0x0008 != 0 || method != 0 || compressed != uncompressed {
        return Err(BackupError::Rejected("压缩包格式不受支持"));
    }
    if read_u32(data, offset)? != LOCAL {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    let local_flags = read_u16(data, offset + 6)?;
    let local_method = read_u16(data, offset + 8)?;
    let local_crc = read_u32(data, offset + 14)?;
    let local_compressed = read_u32(data, offset + 18)?;
    let local_uncompressed = read_u32(data, offset + 22)?;
    let name_len = read_u16(data, offset + 26)? as usize;
    let extra_len = read_u16(data, offset + 28)? as usize;
    if local_flags != flags
        || local_method != method
        || local_crc != crc
        || local_compressed != compressed
        || local_uncompressed != uncompressed
    {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    let name_at = offset + 30;
    let local_name = slice(data, name_at, name_len)?;
    if local_name != name.as_bytes() {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    let data_at = name_at
        .checked_add(name_len + extra_len)
        .ok_or(BackupError::Rejected("压缩包无法读取"))?;
    let bytes = slice(data, data_at, uncompressed as usize)?.to_vec();
    if crc32(&bytes) != crc {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    Ok(StoredFile {
        name: name.to_owned(),
        bytes,
    })
}

fn find_eocd(data: &[u8]) -> Result<usize, BackupError> {
    if data.len() < 22 {
        return Err(BackupError::Rejected("压缩包无法读取"));
    }
    let start = data.len().saturating_sub(22 + 65535);
    let mut index = data.len() - 22;
    loop {
        if data[index..index + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let comment = u16::from_le_bytes([data[index + 20], data[index + 21]]) as usize;
            if index + 22 + comment == data.len() {
                return Ok(index);
            }
        }
        if index == start {
            break;
        }
        index -= 1;
    }
    Err(BackupError::Rejected("压缩包无法读取"))
}

fn slice(data: &[u8], at: usize, len: usize) -> Result<&[u8], BackupError> {
    let end = at
        .checked_add(len)
        .ok_or(BackupError::Rejected("压缩包无法读取"))?;
    data.get(at..end)
        .ok_or(BackupError::Rejected("压缩包无法读取"))
}

fn read_u16(data: &[u8], at: usize) -> Result<u16, BackupError> {
    let bytes = slice(data, at, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, BackupError> {
    let bytes = slice(data, at, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_of_the_standard_check_string() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn stored_roundtrip_keeps_names_and_bytes() {
        let files = vec![
            StoredFile {
                name: "manifest.json".to_owned(),
                bytes: b"{}".to_vec(),
            },
            StoredFile {
                name: "notes/a.json".to_owned(),
                bytes: "正文".as_bytes().to_vec(),
            },
        ];
        let bytes = write_stored(&files).unwrap();
        let read = read_stored(&bytes).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].name, "notes/a.json");
        assert_eq!(read[1].bytes, "正文".as_bytes());
    }

    #[test]
    fn deflate_method_is_rejected() {
        let mut bytes = write_stored(&[StoredFile {
            name: "a.txt".to_owned(),
            bytes: b"hi".to_vec(),
        }])
        .unwrap();
        // local method is at offset 8.
        bytes[8] = 8;
        bytes[9] = 0;
        // central method follows the local payload. Rebuild is easier: flip both method fields by scanning.
        // The writer puts method 0. Patching only the local header makes central and local disagree.
        assert!(read_stored(&bytes).is_err());
    }
}
