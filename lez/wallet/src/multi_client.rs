use anyhow::{Context, Result};
use sequencer_service_rpc::{SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};

use crate::config::SequencerConnectionData;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub latency_avg: u64,
    pub latency_var: u64,
    pub last_block_id: u64,
}

#[derive(Clone)]
pub struct MultiSequencerClient {
    pub client_list: Vec<SequencerClient>,
}

impl MultiSequencerClient {
    pub fn new(conn_data: &[SequencerConnectionData]) -> Result<Self> {
        let mut client_list = vec![];

        for SequencerConnectionData {
            sequencer_addr,
            basic_auth,
        } in conn_data
        {
            let sequencer_client = {
                let mut builder = SequencerClientBuilder::default();
                if let Some(basic_auth) = &basic_auth {
                    builder = builder.set_headers(
                        std::iter::once((
                            "Authorization".parse().expect("Header name is valid"),
                            format!("Basic {basic_auth}")
                                .parse()
                                .context("Invalid basic auth format")?,
                        ))
                        .collect(),
                    );
                }
                builder
                    .build(sequencer_addr)
                    .context("Failed to create sequencer client")?
            };

            client_list.push(sequencer_client);
        }

        Ok(Self { client_list })
    }

    pub fn optimal_client_ref(&self, _metrics: &[Metrics]) -> &SequencerClient {
        todo!();
    }

    pub fn optimal_client_clone(&self, _metrics: &[Metrics]) -> SequencerClient {
        todo!();
    }
}
