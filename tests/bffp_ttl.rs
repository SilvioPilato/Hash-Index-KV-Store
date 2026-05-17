//! Wire round-trip coverage for the TTL framing (spec §2.1–2.3).
//!
//! Everything else in the suite exercises the encoder/decoder with
//! `ttl = None`; these tests cover the TTL-present path that was
//! reimplemented: `encode_command` → `decode_input_frame` for
//! `Write`/`Mset`/`Ttl`, plus the reserved-flag-bit robustness of the
//! decoder's HAS_TTL check.

use rustikv::bffp::{Command, decode_input_frame, encode_command};

fn roundtrip(cmd: Command) -> Command {
    decode_input_frame(&encode_command(cmd)).expect("decode_input_frame")
}

#[test]
fn write_without_ttl_roundtrips() {
    match roundtrip(Command::Write("k".into(), "v".into(), None)) {
        Command::Write(k, v, ttl) => {
            assert_eq!(k, "k");
            assert_eq!(v, "v");
            assert_eq!(ttl, None);
        }
        _ => panic!("expected Command::Write"),
    }
}

#[test]
fn write_with_ttl_roundtrips() {
    match roundtrip(Command::Write(
        "session".into(),
        "payload".into(),
        Some(3600),
    )) {
        Command::Write(k, v, ttl) => {
            assert_eq!(k, "session");
            assert_eq!(v, "payload");
            assert_eq!(ttl, Some(3600));
        }
        _ => panic!("expected Command::Write"),
    }
}

#[test]
fn write_value_containing_spaces_roundtrips_with_ttl() {
    // Guards the value_len framing: a space-bearing value plus a trailing
    // seconds field must not desync the reader.
    match roundtrip(Command::Write(
        "k".into(),
        "hello world EX 5".into(),
        Some(1),
    )) {
        Command::Write(_, v, ttl) => {
            assert_eq!(v, "hello world EX 5");
            assert_eq!(ttl, Some(1));
        }
        _ => panic!("expected Command::Write"),
    }
}

#[test]
fn mset_mixed_ttl_roundtrips_per_entry() {
    let items = vec![
        ("a".to_string(), "1".to_string(), Some(10u32)),
        ("b".to_string(), "2".to_string(), None),
        ("c".to_string(), "3".to_string(), Some(99u32)),
    ];
    match roundtrip(Command::Mset(items.clone())) {
        Command::Mset(decoded) => assert_eq!(decoded, items),
        _ => panic!("expected Command::Mset"),
    }
}

#[test]
fn mset_all_none_roundtrips() {
    let items = vec![
        ("k1".to_string(), "v1".to_string(), None),
        ("k2".to_string(), "v2".to_string(), None),
    ];
    match roundtrip(Command::Mset(items.clone())) {
        Command::Mset(decoded) => assert_eq!(decoded, items),
        _ => panic!("expected Command::Mset"),
    }
}

#[test]
fn ttl_command_roundtrips() {
    match roundtrip(Command::Ttl("key".into(), 7200)) {
        Command::Ttl(k, secs) => {
            assert_eq!(k, "key");
            assert_eq!(secs, 7200);
        }
        _ => panic!("expected Command::Ttl"),
    }
}

#[test]
fn ttl_command_zero_persist_roundtrips() {
    // 0 = PERSIST sentinel must survive the wire as a literal 0.
    match roundtrip(Command::Ttl("key".into(), 0)) {
        Command::Ttl(k, secs) => {
            assert_eq!(k, "key");
            assert_eq!(secs, 0);
        }
        _ => panic!("expected Command::Ttl"),
    }
}

#[test]
fn decoder_honors_has_ttl_bit_alongside_reserved_bits() {
    // Spec §2.2 reserves the other 7 flag bits. A frame with HAS_TTL (bit 0)
    // set *and* a reserved bit set must still decode the trailing seconds —
    // an exact `flag == FLAG_HAS_TTL` check would regress this.
    let mut frame = Vec::new();
    let total_len: u32 = 1 /*op*/ + 1 /*flags*/ + 2 /*key_len*/ + 1 /*key*/
        + 4 /*val_len*/ + 1 /*val*/ + 4 /*seconds*/;
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.push(2); // OpCode::Write
    frame.push(0b0000_0011); // HAS_TTL | a reserved bit
    frame.extend_from_slice(&1u16.to_be_bytes());
    frame.extend_from_slice(b"k");
    frame.extend_from_slice(&1u32.to_be_bytes());
    frame.extend_from_slice(b"v");
    frame.extend_from_slice(&42u32.to_be_bytes());

    match decode_input_frame(&frame).expect("decode") {
        Command::Write(k, v, ttl) => {
            assert_eq!(k, "k");
            assert_eq!(v, "v");
            assert_eq!(ttl, Some(42));
        }
        _ => panic!("expected Command::Write"),
    }
}
