use crate::crc::{crc32, crc32_seeded};

const BLOOM_SALT: u32 = 0x517CC1B7;

pub struct BloomFilter {
    bits: Vec<u8>,
    hash_count: u32,
}

impl BloomFilter {
    pub fn new(size: usize, hash_count: u32) -> Self {
        let array: Vec<u8> = vec![0u8; size];

        BloomFilter {
            bits: array,
            hash_count,
        }
    }

    pub fn might_contain(&self, key: &str) -> bool {
        let h1 = crc32(key.as_bytes());
        let h2 = crc32_seeded(key.as_bytes(), BLOOM_SALT);

        for i in 0..self.hash_count {
            let hash_i = h1.wrapping_add(i.wrapping_mul(h2));
            let bit_index = hash_i as usize % (self.bits.len() * 8);
            let byte_index = bit_index / 8;

            let bit_offset = bit_index % 8;
            if self.bits[byte_index] & (1 << bit_offset) == 0 {
                return false;
            }
        }
        true
    }

    pub fn insert(&mut self, key: &str) {
        let h1 = crc32(key.as_bytes());
        let h2 = crc32_seeded(key.as_bytes(), BLOOM_SALT);

        for i in 0..self.hash_count {
            let hash_i = h1.wrapping_add(i.wrapping_mul(h2));
            let bit_index = hash_i as usize % (self.bits.len() * 8);
            let byte_index = bit_index / 8;

            let bit_offset = bit_index % 8;
            self.bits[byte_index as usize] |= 1 << bit_offset;
        }
    }
}
