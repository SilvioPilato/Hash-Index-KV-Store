// Binary frame fixed protocol

use std::io::{self, Cursor, Read, Write};

const FRAME_LEN_SIZE: u64 = 4;
const STATUS_SIZE: u64 = 1;
const KEY_LEN_SIZE: usize = 2;
const VALUE_LEN_SIZE: usize = 4;
const PAYLOAD_LEN_SIZE: usize = 4;

pub enum Command {
    Invalid(u8),
    Read(String),
    Write(String, String),
    Delete(String),
    Compact,
    Stats,
    List,
}

#[repr(u8)]
pub enum ResponseStatus {
    Ok = 0,
    NotFound = 1,
    Error = 2,
    Noop = 3,
}

impl TryFrom<u8> for ResponseStatus {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(ResponseStatus::Ok),
            1 => Ok(ResponseStatus::NotFound),
            2 => Ok(ResponseStatus::Error),
            3 => Ok(ResponseStatus::Noop),
            n => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown status byte: {n}"),
            )),
        }
    }
}

pub struct DecodedResponse {
    pub payload: Vec<String>,
    pub status: ResponseStatus,
}

pub fn decode_input_frame(buffer: &[u8]) -> io::Result<Command> {
    let mut cur = Cursor::new(buffer);

    let mut len_buf = [0u8; FRAME_LEN_SIZE as usize];
    cur.read_exact(&mut len_buf)?;
    let _total_len = u32::from_be_bytes(len_buf);

    let mut op_buf = [0u8; STATUS_SIZE as usize];
    cur.read_exact(&mut op_buf)?;

    match op_buf[0] {
        1 => Ok(Command::Read(read_key(&mut cur)?)),
        2 => {
            let key = read_key(&mut cur)?;
            let value = read_value(&mut cur)?;
            Ok(Command::Write(key, value))
        }
        3 => Ok(Command::Delete(read_key(&mut cur)?)),
        4 => Ok(Command::Compact),
        5 => Ok(Command::Stats),
        6 => Ok(Command::List),
        n => Ok(Command::Invalid(n)),
    }
}

pub fn decode_response_frame(buffer: &[u8]) -> io::Result<DecodedResponse> {
    let mut responses: Vec<String> = Vec::new();
    let mut cur = Cursor::new(buffer);
    let mut len_buf = [0u8; FRAME_LEN_SIZE as usize];
    cur.read_exact(&mut len_buf)?;
    let total_len = u32::from_be_bytes(len_buf);
    let mut op_buf = [0u8; STATUS_SIZE as usize];
    cur.read_exact(&mut op_buf)?;

    while cur.position() < total_len as u64 + FRAME_LEN_SIZE {
        let mut size_buf = [0u8; PAYLOAD_LEN_SIZE];
        cur.read_exact(&mut size_buf)?;
        let payload_size = u32::from_be_bytes(size_buf);
        let mut buf = vec![0u8; payload_size as usize];
        cur.read_exact(&mut buf)?;
        responses.push(
            String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
        );
    }

    Ok(DecodedResponse {
        payload: responses,
        status: ResponseStatus::try_from(op_buf[0])?,
    })
}

pub fn encode_frame(status: ResponseStatus, responses: &[String]) -> Vec<u8> {
    let mut payload = Cursor::new(Vec::new());
    payload.write_all(&[status as u8]).unwrap();
    for res in responses {
        let bytes = res.as_bytes();
        payload
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        payload.write_all(bytes).unwrap();
    }

    let payload = payload.into_inner();
    let mut frame = Cursor::new(Vec::new());
    frame
        .write_all(&(payload.len() as u32).to_be_bytes())
        .unwrap();
    frame.write_all(&payload).unwrap();
    frame.into_inner()
}

fn read_string(cur: &mut Cursor<&[u8]>, len_bytes: usize) -> io::Result<String> {
    let len = match len_bytes {
        KEY_LEN_SIZE => {
            let mut b = [0u8; KEY_LEN_SIZE];
            cur.read_exact(&mut b)?;
            u16::from_be_bytes(b) as usize
        }
        VALUE_LEN_SIZE => {
            let mut b = [0u8; VALUE_LEN_SIZE];
            cur.read_exact(&mut b)?;
            u32::from_be_bytes(b) as usize
        }
        _ => unreachable!(),
    };
    let mut buf = vec![0u8; len];
    cur.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn read_key(cur: &mut Cursor<&[u8]>) -> io::Result<String> {
    read_string(cur, KEY_LEN_SIZE)
}

fn read_value(cur: &mut Cursor<&[u8]>) -> io::Result<String> {
    read_string(cur, VALUE_LEN_SIZE)
}
