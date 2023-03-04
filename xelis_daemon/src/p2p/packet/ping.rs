use xelis_common::{
    config::P2P_PING_PEER_LIST_LIMIT,
    crypto::hash::Hash,
    serializer::{
        Writer,
        Serializer,
        ReaderError,
        Reader
    },
    globals::{
        ip_to_bytes,
        ip_from_bytes
    }
};
use crate::p2p::peer::Peer;
use std::{
    fmt::Display,
    borrow::Cow,
    net::SocketAddr,
    sync::Arc
};
use log::trace;

#[derive(Clone, Debug)]
pub struct Ping<'a> {
    top_hash: Cow<'a, Hash>,
    topoheight: u64,
    height: u64,
    cumulative_difficulty: u64,
    peer_list: Vec<SocketAddr>
}

impl<'a> Ping<'a> {
    pub fn new(top_hash: Cow<'a, Hash>, topoheight: u64, height: u64, cumulative_difficulty: u64, peer_list: Vec<SocketAddr>) -> Self {
        Self {
            top_hash,
            topoheight,
            height,
            cumulative_difficulty,
            peer_list
        }
    }

    pub async fn update_peer(self, peer: &Arc<Peer>) {
        trace!("Updating {} with {}", peer, self);
        peer.set_block_top_hash(self.top_hash.into_owned()).await;
        peer.set_topoheight(self.topoheight);
        peer.set_height(self.height);
        peer.set_cumulative_difficulty(self.cumulative_difficulty);

        let mut peers = peer.get_peers().lock().await;
        for peer in self.peer_list {
            if !peers.contains(&peer) {
                peers.insert(peer);
            }
        }
    }

    pub fn get_height(&self) -> u64 {
        self.height
    }

    pub fn get_peers(&self) -> &Vec<SocketAddr> {
        &self.peer_list
    }
}

impl Serializer for Ping<'_> {
    fn write(&self, writer: &mut Writer) {
        writer.write_hash(&self.top_hash);
        writer.write_u64(&self.topoheight);
        writer.write_u64(&self.height);
        writer.write_u64(&self.cumulative_difficulty);
        writer.write_u8(self.peer_list.len() as u8);
        for peer in &self.peer_list {
            writer.write_bytes(&ip_to_bytes(peer));
        }
    }

    fn read(reader: &mut Reader) -> Result<Self, ReaderError> {
        let top_hash = Cow::Owned(reader.read_hash()?);
        let topoheight = reader.read_u64()?;
        let height = reader.read_u64()?;
        let cumulative_difficulty = reader.read_u64()?;
        let peers_len = reader.read_u8()? as usize;
        if peers_len > P2P_PING_PEER_LIST_LIMIT {
            return Err(ReaderError::InvalidValue)
        }

        let mut peer_list = Vec::with_capacity(peers_len);
        for _ in 0..peers_len {
            let peer = ip_from_bytes(reader)?;
            peer_list.push(peer);
        }

        Ok(Self { top_hash, topoheight, height, cumulative_difficulty, peer_list })
    }
}

impl Display for Ping<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ping[top_hash: {}, topoheight: {}, height: {}, peers length: {}]", self.top_hash, self.topoheight, self.height, self.peer_list.len())
    }
}