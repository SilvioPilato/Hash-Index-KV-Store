// Binary frame fixed protocol

use std::io::{self, Cursor, Read, Write};

const FRAME_LEN_SIZE: u64 = 4;
const OP_CODE_SIZE: usize = 1;
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
    Exists(String),
}

#[repr(u8)]
pub enum OpCode {
    Read = 1,
    Write = 2,
    Delete = 3,
    Compact = 4,
    Stats = 5,
    List = 6,
    Exists = 7,
}

impl TryFrom<u8> for OpCode {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            1 => Ok(OpCode::Read),
            2 => Ok(OpCode::Write),
            3 => Ok(OpCode::Delete),
            4 => Ok(OpCode::Compact),
            5 => Ok(OpCode::Stats),
            6 => Ok(OpCode::List),
            7 => Ok(OpCode::Exists),
            n => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown op code: {n}"),
            )),
        }
    }
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

    let mut op_buf = [0u8; OP_CODE_SIZE];
    cur.read_exact(&mut op_buf)?;

    match OpCode::try_from(op_buf[0]) {
        Ok(OpCode::Read) => Ok(Command::Read(read_key(&mut cur)?)),
        Ok(OpCode::Write) => {
            let key = read_key(&mut cur)?;
            let value = read_value(&mut cur)?;
            Ok(Command::Write(key, value))
        }
        Ok(OpCode::Delete) => Ok(Command::Delete(read_key(&mut cur)?)),
        Ok(OpCode::Compact) => Ok(Command::Compact),
        Ok(OpCode::Stats) => Ok(Command::Stats),
        Ok(OpCode::List) => Ok(Command::List),
        Ok(OpCode::Exists) => Ok(Command::Exists(read_key(&mut cur)?)),
        Err(_) => Ok(Command::Invalid(op_buf[0])),
    }
}

pub fn decode_response_frame(buffer: &[u8]) -> io::Result<DecodedResponse> {
    let mut responses: Vec<String> = Vec::new();
    let mut cur = Cursor::new(buffer);
    let mut len_buf = [0u8; FRAME_LEN_SIZE as usize];
    cur.read_exact(&mut len_buf)?;
    let total_len = u32::from_be_bytes(len_buf);
    let mut op_buf = [0u8; OP_CODE_SIZE];
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

pub fn encode_command(command: Command) -> Vec<u8> {
    let mut payload = Cursor::new(Vec::new());
    match command {
        Command::Read(key) => {
            let total_len = (OP_CODE_SIZE + KEY_LEN_SIZE + key.len()) as u32;
            let key_len = key.len() as u16;
            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Read as u8]).unwrap();

            payload.write_all(&key_len.to_be_bytes()).unwrap();

            payload.write_all(key.as_bytes()).unwrap();

            payload.into_inner()
        }
        Command::Write(key, value) => {
            let total_len =
                (OP_CODE_SIZE + KEY_LEN_SIZE + VALUE_LEN_SIZE + key.len() + value.len()) as u32;
            let key_len = key.len() as u16;
            let val_len = value.len() as u32;

            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Write as u8]).unwrap();

            payload.write_all(&key_len.to_be_bytes()).unwrap();

            payload.write_all(key.as_bytes()).unwrap();

            payload.write_all(&val_len.to_be_bytes()).unwrap();

            payload.write_all(value.as_bytes()).unwrap();

            payload.into_inner()
        }
        Command::Delete(key) => {
            let total_len = (OP_CODE_SIZE + KEY_LEN_SIZE + key.len()) as u32;
            let key_len = key.len() as u16;

            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Delete as u8]).unwrap();

            payload.write_all(&key_len.to_be_bytes()).unwrap();

            payload.write_all(key.as_bytes()).unwrap();

            payload.into_inner()
        }
        Command::Compact => {
            let total_len = OP_CODE_SIZE as u32;
            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Compact as u8]).unwrap();

            payload.into_inner()
        }
        Command::Stats => {
            let total_len = OP_CODE_SIZE as u32;
            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Stats as u8]).unwrap();

            payload.into_inner()
        }
        Command::List => {
            let total_len = OP_CODE_SIZE as u32;
            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::List as u8]).unwrap();

            payload.into_inner()
        }
        Command::Exists(key) => {
            let total_len = (OP_CODE_SIZE + KEY_LEN_SIZE + key.len()) as u32;
            let key_len = key.len() as u16;

            payload.write_all(&total_len.to_be_bytes()).unwrap();

            payload.write_all(&[OpCode::Exists as u8]).unwrap();

            payload.write_all(&key_len.to_be_bytes()).unwrap();

            payload.write_all(key.as_bytes()).unwrap();

            payload.into_inner()
        }
        Command::Invalid(_) => unreachable!("Invalid is a decode-only sentinel, never encoded"),
    }
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
