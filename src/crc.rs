const REFLECTED_POLYNOMIAL: u32 = 0xEDB88320;

const fn make_lookup_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;

    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ REFLECTED_POLYNOMIAL;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

const LOOKUP_TABLE: [u32; 256] = make_lookup_table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;

    for &byte in data {
        let index = ((crc as u8) ^ byte) as usize;
        crc = LOOKUP_TABLE[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

pub fn crc32_seeded(data: &[u8], seed: u32) -> u32 {
    let mut crc = seed;

    for &byte in data {
        let index = ((crc as u8) ^ byte) as usize;
        crc = LOOKUP_TABLE[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}
