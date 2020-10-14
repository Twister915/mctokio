use super::{ReadBridge, WriteBridge};
use tokio::net::{ToSocketAddrs, TcpStream};
use tokio::io;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use mcproto_rs::v1_15_2::{PacketDirection, State, Id, Packet578};
use crate::Bridge;
use mcproto_rs::protocol::RawPacket;

pub type TcpReadBridge = ReadBridge<io::BufReader<OwnedReadHalf>>;

pub type TcpWriteBridge = WriteBridge<OwnedWriteHalf>;

pub struct TcpConnection {
    pub reader: TcpReadBridge,
    pub writer: TcpWriteBridge,
}

const BUF_CAP: usize = 8192;

impl TcpConnection {
    pub async fn connect_to_server<A: ToSocketAddrs>(target: A) -> io::Result<Self> {
        let conn = TcpStream::connect(target).await?;
        conn.set_nodelay(true)?;
        Ok(Self::from_server_connection(conn))
    }

    pub fn from_server_connection(server: TcpStream) -> Self {
        Self::from_connection(server, PacketDirection::ClientBound)
    }

    pub fn from_client_connection(client: TcpStream) -> Self {
        Self::from_connection(client, PacketDirection::ServerBound)
    }

    pub fn from_connection(conn: TcpStream, read_direction: PacketDirection) -> Self {
        let (reader, writer) = conn.into_split();
        let reader = io::BufReader::with_capacity(BUF_CAP, reader);
        Self {
            reader: TcpReadBridge::initial(read_direction, reader),
            writer: TcpWriteBridge::initial(read_direction.opposite(), writer),
        }
    }

    pub fn split(&mut self) -> (&mut TcpReadBridge, &mut TcpWriteBridge) {
        (&mut self.reader, &mut self.writer)
    }

    pub fn into_split(self) -> (TcpReadBridge, TcpWriteBridge) {
        (self.reader, self.writer)
    }

    pub fn into_inner(self) -> (io::BufReader<OwnedReadHalf>, OwnedWriteHalf) {
        (self.reader.into_inner(), self.writer.into_inner())
    }

    pub async fn read_packet(&mut self) -> anyhow::Result<Option<RawPacket<'_, Id>>> {
        self.reader.read_packet().await
    }

    pub async fn write_packet(&mut self, packet: Packet578) -> anyhow::Result<()> {
        self.writer.write_packet(packet).await
    }

    pub async fn write_raw_packet<'a>(&'a mut self, packet: RawPacket<'a, Id>) -> anyhow::Result<()> {
        self.writer.write_raw_packet(packet).await
    }
}

impl Bridge for TcpConnection {
    fn set_state(&mut self, next: State) {
        self.reader.set_state(next.clone());
        self.writer.set_state(next);
    }

    fn set_compression_threshold(&mut self, threshold: Option<i32>) {
        self.reader.set_compression_threshold(threshold.clone());
        self.writer.set_compression_threshold(threshold);
    }

    fn enable_encryption(&mut self, key: &[u8], iv: &[u8]) -> anyhow::Result<()> {
        self.reader.enable_encryption(key.clone(), iv.clone())?;
        self.writer.enable_encryption(key, iv)
    }
}