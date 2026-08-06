use std::net::SocketAddr;

use kameo::Reply;

pub struct GetAddress;

#[derive(Reply)]
pub struct GetAddressReply {
    pub addr: SocketAddr,
}
