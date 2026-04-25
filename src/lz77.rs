const SEARCH_WINDOW: usize = 32_768;
const LOOKAHEAD_WINDOW: usize = 258;
const MAX_CHAIN: usize = 128;
const HASH_SIZE: usize = 32_768; // must equal 2^(3 * H_SHIFT) for sliding-window property
const H_SHIFT: u32 = 5; // 3 * 5 = 15 bits → mask = 32767
pub struct Lz77;

struct Match {
    offset: usize,
    len: usize,
}

impl Lz77 {
    pub fn encode(data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut table = vec![u32::MAX; HASH_SIZE];
        let mut chain = vec![None; data.len()];
        let mut pos: usize = 0;

        // Prime the rolling hash with the first two bytes so that feeding
        // data[pos+2] at each step produces the trigram hash for position pos.
        let mut hash = 0u32;
        if !data.is_empty() {
            hash = Self::rolling_hash(hash, data[0]);
        }
        if data.len() >= 2 {
            hash = Self::rolling_hash(hash, data[1]);
        }

        while pos < data.len() {
            // Feed data[pos+2] to complete the trigram hash for this position.
            if pos + 2 < data.len() {
                hash = Self::rolling_hash(hash, data[pos + 2]);
            }

            match Self::find_longest_match(data, pos, hash as usize, &table, &chain) {
                Some(best_match) => {
                    output.push(1);
                    Self::encode_varint(best_match.offset as u32, &mut output);
                    Self::encode_varint(best_match.len as u32, &mut output);

                    // Insert pos with the current hash, then roll through the
                    // remaining matched positions, inserting each one.
                    for i in 0..best_match.len {
                        if pos + i + 3 <= data.len() {
                            let h = hash as usize;
                            let old_head = table[h];
                            if old_head != u32::MAX {
                                chain[pos + i] = Some(old_head as usize);
                            }
                            table[h] = (pos + i) as u32;
                        }
                        // Roll hash forward for pos+i+1 (feed data[(pos+i+1)+2]).
                        let next = pos + i + 3;
                        if next < data.len() {
                            hash = Self::rolling_hash(hash, data[next]);
                        }
                    }
                    pos += best_match.len;
                }
                None => {
                    output.push(0);
                    output.push(data[pos]);

                    // Insert position into table.
                    if pos + 3 <= data.len() {
                        let h = hash as usize;
                        let old_head = table[h];
                        if old_head != u32::MAX {
                            chain[pos] = Some(old_head as usize);
                        }
                        table[h] = pos as u32;
                    }
                    pos += 1;
                    // Hash for pos+1 will be completed at the top of the next
                    // iteration when we feed data[pos+2].
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

    fn find_longest_match(
        data: &[u8],
        pos: usize,
        hash: usize,
        table: &[u32],
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
        let first = table[hash];
        if first == u32::MAX {
            return None;
        }
        let mut candidate = first as usize;

        loop {
            if iteration >= MAX_CHAIN {
                break;
            }
            iteration += 1;

            // Case 1: candidate is ahead of us (or at us) — skip, don't match
            // With incremental chain building, this should not happen, but we guard against it
            if candidate >= pos {
                match chain[candidate] {
                    Some(prev) => {
                        candidate = prev;
                        continue;
                    }
                    None => break,
                }
            }

            // Case 2: candidate is too far back — outside search window
            if candidate < search_start {
                break;
            }

            // Happy path: candidate is valid, measure the match
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

    // Feed one byte into the rolling hash. With HASH_SIZE = 2^(3*H_SHIFT), the
    // contribution of a byte shifts out after exactly 3 calls, giving a true
    // 3-byte sliding window: rolling_hash(rolling_hash(rolling_hash(s,a),b),c)
    // hashes the trigram (a,b,c) regardless of prior state.
    fn rolling_hash(prev: u32, byte: u8) -> u32 {
        ((prev << H_SHIFT) ^ byte as u32) & (HASH_SIZE as u32 - 1)
    }
}
