//! RPC Server Actor server RPC queries and retranslates them to Executor.

use std::net::SocketAddr;

use anyhow::{Context as _, Result};
use bytesize::ByteSize;
use jsonrpsee::{server::ServerHandle, tracing::warn};
use kameo::{Actor, actor::ActorRef, mailbox::Signal, message::Message};
use log::info;
use sequencer_service_rpc::RpcServer as _;

use crate::protocol::{GetAddress, GetAddressReply};

pub mod protocol;
mod service;

const REQUEST_BODY_MAX_SIZE: ByteSize = ByteSize::mib(10);

pub struct RpcServerActor {
    server_handle: Option<ServerHandle>,
    addr: SocketAddr,
}

impl RpcServerActor {
    pub async fn new(
        executor_ref: ActorRef<sequencer_executor_actor::ExecutorActor>,
        listen_addr: SocketAddr,
        max_block_size: u64,
    ) -> Result<Self> {
        let server = jsonrpsee::server::ServerBuilder::with_config(
            jsonrpsee::server::ServerConfigBuilder::new()
                .max_request_body_size(
                    u32::try_from(REQUEST_BODY_MAX_SIZE.as_u64())
                        .expect("REQUEST_BODY_MAX_SIZE should be less than u32::MAX"),
                )
                .build(),
        )
        .build(listen_addr)
        .await
        .context("Failed to build RPC server")?;

        let addr = server
            .local_addr()
            .context("Failed to get local address of RPC server")?;

        info!("Starting RPC Server on {addr}");

        let service = service::Service::new(executor_ref, max_block_size);
        let server_handle = server.start(service.into_rpc());

        Ok(Self {
            server_handle: Some(server_handle),
            addr,
        })
    }
}

impl Actor for RpcServerActor {
    type Args = Self;
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        Ok(args)
    }

    async fn next(
        &mut self,
        _actor_ref: kameo::prelude::WeakActorRef<Self>,
        _mailbox_rx: &mut kameo::prelude::MailboxReceiver<Self>,
    ) -> Result<Option<Signal<Self>>, Self::Error> {
        if let Some(server_handle) = self.server_handle.take() {
            server_handle.stopped().await;
            warn!("RPC server stopped");
        }

        Ok(Some(Signal::Stop))
    }

    async fn on_stop(
        &mut self,
        _actor_ref: kameo::prelude::WeakActorRef<Self>,
        _reason: kameo::prelude::ActorStopReason,
    ) -> Result<(), Self::Error> {
        if let Some(server_handle) = self.server_handle.take() {
            server_handle.stop()?;
            info!("RPC server stopped");
        }

        Ok(())
    }
}

impl Message<GetAddress> for RpcServerActor {
    type Reply = GetAddressReply;

    async fn handle(
        &mut self,
        GetAddress: GetAddress,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetAddressReply { addr: self.addr }
    }
}
