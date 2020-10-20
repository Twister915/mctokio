use super::{bridge::Bridge, util::{get_sized_buf, init_buf}, cfb8::MinecraftCipher};
use mcproto_rs::{
    types::VarInt,
    protocol::{State, PacketDirection, Id, RawPacket, Packet},
    SerializeResult,
    Serialize,
    Serializer,
};
use anyhow::{Result, anyhow};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use std::ops::Range;
use flate2::{Compression, FlushCompress, Status};

pub struct WriteBridge<W> {
    stream: W,
    raw_buf: Option<Vec<u8>>,
    compress_buf: Option<Vec<u8>>,
    compression_threshold: Option<i32>,
    state: State,
    direction: PacketDirection,
    encryption: Option<MinecraftCipher>,
}

const EXTRA_FREE_SPACE: usize = 15;

impl<W> WriteBridge<W> where W: AsyncWrite + Unpin {
    pub fn initial(direction: PacketDirection, stream: W) -> Self {
        Self {
            stream,
            direction,
            state: State::Handshaking,
            raw_buf: None,
            compress_buf: None,
            compression_threshold: None,
            encryption: None,
        }
    }

    pub async fn write_raw_packet<'a, P>(&mut self, packet: P) -> Result<()> where P: RawPacket<'a> {
        let raw_buf = init_buf(&mut self.raw_buf, 512);
        let start_at = EXTRA_FREE_SPACE;
        let data = packet.data();
        let body_len = data.len();
        let end_at = start_at + body_len;
        get_sized_buf(raw_buf, end_at);
        (&mut raw_buf[start_at..end_at]).copy_from_slice(data);
        self.write_packet_in_buf(packet.id(), EXTRA_FREE_SPACE, body_len).await
    }

    pub async fn write_packet<P>(&mut self, packet: P) -> Result<()> where P: Packet {
        let len = {
            let mut serializer = GrowVecSerializer {
                buf: init_buf(&mut self.raw_buf, 512),
                at: EXTRA_FREE_SPACE,
            };

            packet.mc_serialize_body(&mut serializer)?;
            serializer.at - EXTRA_FREE_SPACE
        };

        self.write_packet_in_buf(
            packet.id(),
            EXTRA_FREE_SPACE,
            len,
        ).await
    }

    async fn write_packet_in_buf(&mut self, id: Id, packet_offset: usize, body_len: usize) -> Result<()> {
        if id.direction != self.direction {
            return Err(anyhow!("tried to write packet {:?} but valid direction is {:?}", id, self.direction));
        }

        if id.state != self.state {
            return Err(anyhow!("tried to write packet {:?} but valid state is {:?}", id, self.state));
        }

        let this = &mut *self;
        let raw_buf = init_buf(&mut this.raw_buf, 512);
        let mut id_serializer = SliceSerializer {
            slice: &mut raw_buf[packet_offset - 5..packet_offset],
            at: 0,
        };
        id.mc_serialize(&mut id_serializer)?;
        let id_len = id_serializer.at;
        let id_start_at = packet_offset - 5;
        let id_end_at = id_start_at + id_len;
        let id_shift_n = 5 - id_len;
        copy_data_rightwards(raw_buf.as_mut_slice(), id_start_at..id_end_at, id_shift_n);

        let data_len = id_len + body_len;
        let data_start_at = packet_offset - id_len;
        let (packet_buf, start_at, end_at) = if let Some(threshold) = this.compression_threshold.as_ref() {
            if data_len < (*threshold as usize) {
                let data_len_at = data_start_at - 1;
                let packet_end_at = data_start_at + data_len;
                raw_buf[data_len_at] = 0;
                (raw_buf, data_len_at, packet_end_at)
            } else {
                let src = &raw_buf[data_start_at..data_start_at + data_len];

                let mut compressor = flate2::Compress::new_with_window_bits(Compression::fast(), true, 15);
                let compress_buf = &mut this.compress_buf;
                let compress_buf = match compress_buf.as_mut() {
                    Some(buf) => buf,
                    None => {
                        compress_buf.replace(Vec::with_capacity(src.len()));
                        compress_buf.as_mut().unwrap()
                    }
                };

                get_sized_buf(compress_buf, src.len());

                loop {
                    let input = &src[(compressor.total_in() as usize)..];
                    let eof = input.is_empty();
                    let output = &mut compress_buf[EXTRA_FREE_SPACE + (compressor.total_out() as usize)..];
                    let flush = if eof {
                        FlushCompress::Finish
                    } else {
                        FlushCompress::None
                    };
                    match compressor.compress(input, output, flush)? {
                        Status::Ok => {}
                        Status::BufError => {
                            // ensure size
                            get_sized_buf(compress_buf, compressor.total_out() as usize);
                        }
                        Status::StreamEnd => break
                    }
                }

                // write data_len to raw_buf
                let data_len_start_at = EXTRA_FREE_SPACE - 5;
                let data_len_target = &mut compress_buf[data_len_start_at..EXTRA_FREE_SPACE];
                let mut data_len_serializer = SliceSerializer {
                    slice: data_len_target,
                    at: 0,
                };
                &VarInt(data_len as i32).mc_serialize(&mut data_len_serializer)?;
                let data_len_len = data_len_serializer.at;
                let data_len_end_at = data_len_start_at + data_len_len;
                let data_len_shift_n = 5 - data_len_len;
                copy_data_rightwards(compress_buf.as_mut_slice(), data_len_start_at..data_len_end_at, data_len_shift_n);
                let compressed_end_at = EXTRA_FREE_SPACE + (compressor.total_out() as usize);
                (compress_buf, data_len_start_at + data_len_shift_n, compressed_end_at)
            }
        } else {
            (raw_buf, data_start_at, data_start_at + data_len)
        };

        // now just prefix the actual length
        if start_at < 5 {
            panic!("need space to write length, not enough!");
        }

        let len = VarInt((end_at - start_at) as i32);
        let len_start_at = start_at - 5;
        let mut len_serializer = SliceSerializer {
            slice: &mut packet_buf[len_start_at..start_at],
            at: 0,
        };
        len.mc_serialize(&mut len_serializer)?;
        let len_len = len_serializer.at;
        let len_end_at = len_start_at + len_len;
        let len_shift_n = 5 - len_len;

        copy_data_rightwards(packet_buf.as_mut_slice(), len_start_at..len_end_at, len_shift_n);
        let new_len_start_at = len_start_at + len_shift_n;
        let packet_data = &mut packet_buf[new_len_start_at..end_at];
        if let Some(enc) = this.encryption.as_mut() {
            enc.encrypt(packet_data);
        }

        this.stream.write_all(packet_data).await?;
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.stream
    }
}

impl<W> Bridge for WriteBridge<W> {
    fn set_state(&mut self, next: State) {
        self.state = next;
    }

    fn set_compression_threshold(&mut self, threshold: Option<i32>) {
        self.compression_threshold = threshold;
    }

    fn enable_encryption(&mut self, key: &[u8], iv: &[u8]) -> Result<()> {
        if self.encryption.is_some() {
            return Err(anyhow!("cannot enable encryption more than once!"));
        }

        self.encryption = Some(MinecraftCipher::new(key, iv)?);
        Ok(())
    }
}

struct SliceSerializer<'a> {
    slice: &'a mut [u8],
    at: usize,
}

impl<'a> Serializer for SliceSerializer<'a> {
    fn serialize_bytes(&mut self, data: &[u8]) -> SerializeResult {
        let start_at = self.at;
        let end_at = start_at + data.len();
        if end_at > self.slice.len() {
            panic!("failed to serialize, out of space!")
        }

        self.slice[start_at..end_at].copy_from_slice(data);
        self.at = end_at;
        Ok(())
    }

    fn serialize_byte(&mut self, byte: u8) -> SerializeResult {
        self.serialize_bytes(&[byte])
    }
}

struct GrowVecSerializer<'a> {
    buf: &'a mut Vec<u8>,
    at: usize,
}

impl<'a> Serializer for GrowVecSerializer<'a> {
    fn serialize_bytes(&mut self, data: &[u8]) -> SerializeResult {
        let start_at = self.at;
        let additional_data_len = data.len();
        let end_at = start_at + additional_data_len;
        let buf = get_sized_buf(self.buf, end_at);
        let buf = &mut buf[start_at..end_at];

        buf.copy_from_slice(data);
        self.at += additional_data_len;
        Ok(())
    }

    fn serialize_byte(&mut self, byte: u8) -> SerializeResult {
        self.serialize_bytes(&[byte])
    }

    fn serialize_other<S: Serialize>(&mut self, other: &S) -> SerializeResult {
        other.mc_serialize(self)
    }
}

fn copy_data_rightwards(target: &mut [u8], range: Range<usize>, shift_amount: usize) {
    if shift_amount == 0 {
        return;
    }

    // check bounds
    let buf_len = target.len();
    let src_start_at = range.start;
    let src_end_at = range.end;
    let data_len = src_end_at - src_start_at;
    if src_start_at >= buf_len || src_end_at > buf_len {
        panic!("source out of bounds!");
    }

    let dest_start_at = src_start_at + shift_amount;
    let dest_end_at = dest_start_at + data_len;
    if dest_start_at >= buf_len || dest_end_at > buf_len {
        panic!("dest out of bounds")
    }

    unsafe {
        let src_ptr = target.as_mut_ptr();
        let data_src_ptr = src_ptr.offset(src_start_at as isize);
        let data_dst_ptr = data_src_ptr.offset(shift_amount as isize);
        std::ptr::copy(data_src_ptr, data_dst_ptr, data_len);
    }
}