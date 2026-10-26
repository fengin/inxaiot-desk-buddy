use crate::core::error::{AppError, AppResult};

pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: u64 = 8 * 1024 * 1024;

/// 只解析有界 PNG 结构和 CRC，不在采集任务里解压整张图片。
pub fn dimensions(bytes: &[u8]) -> AppResult<(u32, u32)> {
    let invalid = || AppError::Integrity {
        operation: "核对屏幕截图格式",
    };
    if bytes.len() < 45 || bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid());
    }
    let mut offset = 8usize;
    let mut size = None;
    let mut data_seen = false;
    let mut chunks = 0usize;
    while offset < bytes.len() {
        chunks += 1;
        if chunks > 10_000 || bytes.len() - offset < 12 {
            return Err(invalid());
        }
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
            .ok_or_else(invalid)?;
        if end > bytes.len() {
            return Err(invalid());
        }
        let kind = &bytes[offset + 4..offset + 8];
        let data = &bytes[offset + 8..end - 4];
        let mut crc = flate2::Crc::new();
        crc.update(&bytes[offset + 4..end - 4]);
        if crc.sum() != u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap()) {
            return Err(invalid());
        }
        match kind {
            b"IHDR" => {
                if size.is_some() || offset != 8 || length != 13 {
                    return Err(invalid());
                }
                let width = u32::from_be_bytes(data[..4].try_into().unwrap());
                let height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                if width == 0
                    || height == 0
                    || width > MAX_DIMENSION
                    || height > MAX_DIMENSION
                    || u64::from(width) * u64::from(height) > MAX_PIXELS
                    || !matches!(data[8], 1 | 2 | 4 | 8 | 16)
                    || !matches!(data[9], 0 | 2 | 3 | 4 | 6)
                    || data[10] != 0
                    || data[11] != 0
                    || data[12] > 1
                {
                    return Err(invalid());
                }
                size = Some((width, height));
            }
            b"IDAT" if size.is_some() && length > 0 => data_seen = true,
            b"IEND" if size.is_some() && data_seen && length == 0 && end == bytes.len() => {
                return Ok(size.unwrap());
            }
            b"IDAT" | b"IEND" => return Err(invalid()),
            _ if size.is_none() => return Err(invalid()),
            _ => {}
        }
        offset = end;
    }
    Err(invalid())
}
