use super::{bridge::Bridge, cfb8::MinecraftCipher, util::{get_sized_buf, init_buf}};
use mcproto_rs::{
    protocol::{RawPacket as RawPacketContainer},
    types::VarInt,
    Deserialize,
    Deserialized,
};
use super::proto::{RawPacket578 as RawPacket, State, PacketDirection, Id};
use tokio::io::{AsyncRead, AsyncReadExt};
use anyhow::{Result, anyhow};
use flate2::{FlushDecompress, Status};
use std::convert::TryInto;

pub struct ReadBridge<R> {
    stream: R,
    raw_buf: Option<Vec<u8>>,
    decompress_buf: Option<Vec<u8>>,
    compression_threshold: Option<i32>,
    state: State,
    direction: PacketDirection,
    encryption: Option<MinecraftCipher>
}

impl<R> ReadBridge<R> where R: AsyncRead + Unpin {
    pub fn initial(direction: PacketDirection, stream: R) -> Self {
        Self {
            stream,
            direction,
            state: State::Handshaking,
            raw_buf: None,
            decompress_buf: None,
            compression_threshold: None,
            encryption: None,
        }
    }

    pub async fn read_packet(&mut self) -> Result<Option<RawPacket<'_>>> {
        // pinning stuff makes this a requirement
        let this = &mut *self;

        // read the packet length
        let packet_len = match this.read_one_varint().await? {
            Some(v) => v,
            None => return Ok(None)
        };

        // grab the stuff we need from our inner:

        // source stream
        let reader = &mut this.stream;
        // buf for raw data
        let raw_buf = init_buf(&mut this.raw_buf, 512);
        let mut buf = get_sized_buf(raw_buf, packet_len.0 as usize);
        reader.read_exact(buf).await?;

        // decrypt if we have encryption state
        if let Some(encryption) = this.encryption.as_mut() {
            encryption.decrypt(buf);
        }

        // decompress if it's compressed
        let buf = if let Some(_) = this.compression_threshold {
            let Deserialized { value: data_len, data: rest } = VarInt::mc_deserialize(buf)?;
            let bytes_consumed = buf.len() - rest.len();
            buf = &mut buf[bytes_consumed..];

            // data_len is 0 when it is not compressed, and non-zero otherwise
            // if it is non-zero, decompress:
            if data_len.0 != 0 {
                let mut decompress = flate2::Decompress::new(true);
                let needed = data_len.0 as usize;
                let decompress_buf = &mut this.decompress_buf;
                let decompress_buf = match decompress_buf {
                    Some(buf) => get_sized_buf(buf, needed),
                    None => {
                        *decompress_buf = Some(Vec::with_capacity(needed));
                        get_sized_buf(decompress_buf.as_mut().unwrap(), needed)
                    }
                };
                loop {
                    match decompress.decompress(buf, decompress_buf, FlushDecompress::Finish)? {
                        Status::BufError => return Err(anyhow!("unable to deserialize because of buf err while reading packet")),
                        Status::StreamEnd => break,
                        Status::Ok => {}
                    }
                }

                &mut decompress_buf[..(decompress.total_out() as usize)]
            } else {
                buf
            }
        } else {
            buf
        };

        // read packet id from buf
        let Deserialized { value: packet_id, data: buf } = VarInt::mc_deserialize(buf)?;
        Ok(Some(RawPacketContainer{
            id: Id{
                state: this.state.clone(),
                direction: this.direction.clone(),
                id: packet_id.0,
            },
            data: buf
        }.try_into()?))
    }

    async fn read_one_varint(&mut self) -> Result<Option<VarInt>> {
        let mut buf = [0u8; 5];
        let mut len = 0usize;
        let mut has_more = true;
        while has_more {
            if len == 5 {
                return Err(anyhow!("varint too long while reading id/length/whatever"));
            }

            let target = &mut buf[len..len + 1];
            let size = self.stream.read(target).await?;
            if size == 0 {
                return Ok(None);
            }

            if let Some(encryption) = self.encryption.as_mut() {
                encryption.decrypt(target);
            }

            has_more = buf[len] & 0x80 != 0;
            len += 1;
        }

        Ok(Some(VarInt::mc_deserialize(&buf[..len])?.value))
    }

    pub fn into_inner(self) -> R {
        self.stream
    }
}

impl<R> Bridge for ReadBridge<R> {
    fn set_state(&mut self, next: State) {
        self.state = next;
    }

    fn set_compression_threshold(&mut self, threshold: Option<i32>) {
        self.compression_threshold = threshold;
    }

    fn enable_encryption(&mut self, key: &[u8], iv: &[u8]) -> Result<()> {
        if self.encryption.is_some() {
            return Err(anyhow!("cannot enable encryption more than once!"))
        }

        self.encryption = Some(MinecraftCipher::new(key, iv)?);
        Ok(())
    }
}