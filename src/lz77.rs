use std::collections::HashMap;

const SEARCH_WINDOW: usize = 32_768;
const LOOKAHEAD_WINDOW: usize = 258;
const MAX_CHAIN: usize = 128;
pub struct Lz77;

struct Match {
    offset: usize,
    len: usize,
}

impl Lz77 {
    pub fn encode(data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let (table, chain) = Self::get_hash_chain(data);
        let mut pos: usize = 0;

        while pos < data.len() {
            match Self::find_longest_match(data, pos, &table, &chain) {
                Some(best_match) => {
                    output.push(1);
                    Self::encode_varint(best_match.offset as u32, &mut output);
                    Self::encode_varint(best_match.len as u32, &mut output);
                    pos += best_match.len;
                }
                None => {
                    output.push(0);
                    output.push(data[pos]);
                    pos += 1;
                }
            }
        }

        output
    }

    pub fn decode(data: &[u8]) -> Vec<u8> {
        let mut output: Vec<u8> = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            let token = data[pos];
            pos += 1;

            match token {
                0 => {
                    if pos >= data.len() {
                        panic!("Truncated literal: no byte after token");
                    }
                    output.push(data[pos]);
                    pos += 1;
                }
                1 => {
                    let (offset, offset_len) = Self::decode_varint(&data[pos..]);
                    pos += offset_len;

                    let (length, length_len) = Self::decode_varint(&data[pos..]);
                    pos += length_len;

                    let offset = offset as usize;
                    if offset > output.len() {
                        panic!(
                            "Invalid match offset: {} > output length {}",
                            offset,
                            output.len()
                        );
                    }
                    let source_start = output.len() - offset;
                    for i in 0..length as usize {
                        let byte = output[source_start + i];
                        output.push(byte);
                    }
                }
                _ => panic!("Invalid compression token"),
            }
        }

        output
    }

    fn get_hash_chain(data: &[u8]) -> (HashMap<[u8; 3], usize>, Vec<Option<usize>>) {
        let mut table = HashMap::new();
        let mut chain = vec![None; data.len()];

        // Iterate all positions with at least 3 bytes remaining
        for i in 0..data.len().saturating_sub(2) {
            // Slice is guaranteed to be exactly 3 bytes
            let hash: [u8; 3] = data[i..i + 3].try_into().unwrap();
            if let Some(&offset) = table.get(&hash) {
                chain[i] = Some(offset);
            }
            table.insert(hash, i);
        }
        (table, chain)
    }

    fn find_longest_match(
        data: &[u8],
        pos: usize,
        table: &HashMap<[u8; 3], usize>,
        chain: &[Option<usize>],
    ) -> Option<Match> {
        let mut best_offset = 0;
        let mut best_len = 0;
        let lookahead = &data[pos..(pos + LOOKAHEAD_WINDOW).min(data.len())];
        let search_start = pos.saturating_sub(SEARCH_WINDOW);
        let mut iteration = 0;

        if lookahead.len() < 3 {
            return None;
        }
        let key: [u8; 3] = lookahead[..3].try_into().unwrap();
        let mut candidate = *table.get(&key)?;

        loop {
            // Case 1: candidate is ahead of us (or at us) — skip, don't match
            // This happens because the hash table stores the LAST occurrence
            if candidate >= pos {
                match chain[candidate] {
                    Some(prev) => {
                        candidate = prev;
                        continue;
                    }
                    None => break, // No earlier occurrence exists
                }
            }

            // Case 2: candidate is too far back — outside search window
            if candidate < search_start {
                break;
            }

            // Case 3: we've followed too many chain links — stop searching
            if iteration >= MAX_CHAIN {
                break;
            }

            // Happy path: candidate is valid, measure the match
            iteration += 1;
            let len = data[candidate..]
                .iter()
                .zip(lookahead.iter())
                .take_while(|(a, b)| a == b)
                .count();

            if len > best_len {
                best_len = len;
                best_offset = pos - candidate;
            }

            // Follow chain to older occurrences
            match chain[candidate] {
                Some(prev) => candidate = prev,
                None => break,
            }
        }

        if best_len < 3 {
            return None;
        }

        Some(Match {
            offset: best_offset,
            len: best_len,
        })
    }

    fn encode_varint(mut value: u32, output: &mut Vec<u8>) {
        while value >= 128 {
            output.push((value & 127 | 128) as u8);
            value >>= 7;
        }

        output.push((value & 127) as u8);
    }

    fn decode_varint(encoded: &[u8]) -> (u32, usize) {
        if encoded.is_empty() {
            return (0, 0);
        }
        let mut value: u32 = 0;
        let mut shift = 0;
        let mut i = 0;

        loop {
            let byte = encoded[i];
            let flag = encoded[i] & 128;
            value |= ((byte & 127) as u32) << shift;
            if flag == 0 {
                break;
            }
            i += 1;
            shift += 7;
        }

        (value, i + 1)
    }
}
