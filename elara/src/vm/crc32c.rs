//! Castagnoli CRC32C (poly `0x82F63B78`, reflected).

/// IEEE / iSCSI Castagnoli polynomial in reflected form.
const POLY: u32 = 0x82F6_3B78;

/// Computes CRC32C over `data` (init/xor `0xFFFF_FFFF`, reflected).
#[must_use]
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn crc32c_empty_and_known_vector() {
        assert_eq!(crc32c(b""), 0);
        // "123456789" Castagnoli check value.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }
}
