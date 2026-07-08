use std::{collections::HashMap, io::Write, path::Path};

use anyhow::{Context, Result};
use sequencer_service_rpc::{RpcClient, SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::SequencerConnectionData;

pub const CALLIBRATION_LIMIT: usize = 100;

pub fn extract_metrics_from_path(path: &Path) -> Result<HashMap<Url, Metrics>, anyhow::Error> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let reader = std::io::BufReader::new(file);
            Ok(serde_json::from_reader(reader)?)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("Metrics not found, choosing empty");
            Ok(HashMap::new())
        }
        Err(err) => Err(err).context("IO error"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub latency_avg: f32,
    pub latency_var: f32,
    pub sample_size: usize,
    pub latest_block_id: u64,
    pub errors: u64,
}

#[derive(Debug, Clone)]
pub struct MetricsUpdate {
    pub latency: f32,
    pub new_latest_block_id: Option<u64>,
    pub is_failed: bool,
}

#[derive(Clone)]
pub struct MultiSequencerClient {
    pub client_list: HashMap<Url, SequencerClient>,
    pub leader: SequencerClient,
    pub leader_url: Url,
}

impl MultiSequencerClient {
    pub async fn new(
        conn_data: &[SequencerConnectionData],
        metrics: &mut HashMap<Url, Metrics>,
    ) -> Result<Self> {
        let mut client_list = HashMap::new();

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

            // If there is no metrics for client, callibrate it
            if !metrics.contains_key(sequencer_addr) {
                metrics.insert(
                    sequencer_addr.clone(),
                    callibration(sequencer_client.clone()).await,
                );
            }

            client_list.insert(sequencer_addr.clone(), sequencer_client);
        }

        let (leader_url, leader) = choose_leader(&client_list, metrics);

        Ok(Self {
            client_list,
            leader,
            leader_url,
        })
    }

    pub fn leader_ref(&self) -> &SequencerClient {
        &self.leader
    }

    pub fn leader_clone(&self) -> SequencerClient {
        self.leader.clone()
    }

    pub async fn metered_call<R, E, I: AsyncFnOnce(&SequencerClient) -> Result<R, E>>(
        &self,
        call: I,
    ) -> (Result<R, E>, MetricsUpdate) {
        let call_last_block = self.leader_ref().get_last_block_id();

        let now = tokio::time::Instant::now();

        let (call_future_res, call_last_block_res) =
            tokio::join!(call(self.leader_ref()), call_last_block);

        let latency = tokio::time::Instant::now().duration_since(now).as_millis() as f32;
        let is_failed = call_future_res.is_err() || call_last_block_res.is_err();
        let mut new_last_block = None;

        if let Ok(last_block) = call_last_block_res {
            new_last_block = Some(last_block);
        }

        let metrics_update = MetricsUpdate {
            latency,
            new_latest_block_id: new_last_block,
            is_failed,
        };

        (call_future_res, metrics_update)
    }
}

pub async fn callibration(client: SequencerClient) -> Metrics {
    let mut latencies = vec![];
    let mut latest_block_id = 0;
    let mut errors = 0;

    for _ in 0..CALLIBRATION_LIMIT {
        let now = tokio::time::Instant::now();

        let block_id = client.get_last_block_id().await;

        let latency = tokio::time::Instant::now().duration_since(now).as_millis();

        let Ok(block_id) = block_id else {
            errors += 1;
            continue;
        };

        latest_block_id = block_id;
        latencies.push(latency);
    }

    // Precision loss if fine there
    let sample_size = latencies.len();
    let latency_avg = (latencies.iter().fold(0, |acc, x| acc + x) as f32) / (sample_size as f32);
    let latency_var = (latencies.iter().fold(0f32, |acc, x| {
        acc + ((*x as f32) - latency_avg) * ((*x as f32) - latency_avg)
    }) / (sample_size as f32))
        .sqrt();

    Metrics {
        latency_avg,
        latency_var,
        sample_size,
        latest_block_id,
        errors,
    }
}

pub fn choose_leader(
    _client_list: &HashMap<Url, SequencerClient>,
    _metrics: &HashMap<Url, Metrics>,
) -> (Url, SequencerClient) {
    todo!()
}

pub fn save_metrics_at_path(
    metrics: &HashMap<Url, Metrics>,
    path: &Path,
) -> Result<(), anyhow::Error> {
    let metrics_serialized = serde_json::to_vec_pretty(metrics)?;
    let mut file = std::fs::File::create(path).context("Failed to create file")?;
    file.write_all(&metrics_serialized)
        .context("Failed to write to file")?;
    file.sync_all().context("Failed to sync file")?;

    Ok(())
}

pub fn save_metrics_at_path_with_updates(
    mut metrics: HashMap<Url, Metrics>,
    leader_url: &Url,
    metric_updates: &[MetricsUpdate],
    path: &Path,
) -> Result<(), anyhow::Error> {
    let leader_metric = metrics
        .get_mut(leader_url)
        .ok_or(anyhow::anyhow!("Leader URL is in present in metrics"))?;

    let (failure_count, latest_block_id, cumulative_latency) =
        metric_updates.iter().fold((0u64, None, 0f32), |acc, x| {
            let MetricsUpdate {
                latency,
                new_latest_block_id,
                is_failed,
            } = x;
            (
                if *is_failed { acc.0 + 1 } else { acc.0 },
                acc.1.map(|inner| {
                    if let Some(new_latest_block_id) = new_latest_block_id {
                        if inner < *new_latest_block_id {
                            *new_latest_block_id
                        } else {
                            inner
                        }
                    } else {
                        inner
                    }
                }),
                acc.2 + latency,
            )
        });

    leader_metric.errors += failure_count;
    if let Some(latest_block_id) = latest_block_id {
        leader_metric.latest_block_id = latest_block_id;
    }

    let orig_size_f = leader_metric.sample_size as f32;
    let mod_size_f = (leader_metric.sample_size + metric_updates.len()) as f32;

    let latency_avg_old = leader_metric.latency_avg;
    let latency_avg_new =
        (leader_metric.latency_avg * orig_size_f + cumulative_latency) / mod_size_f;

    let latency_disp_new = (latency_avg_old - latency_avg_new)
        * ((3f32 * latency_avg_old + latency_avg_new) * orig_size_f / mod_size_f)
        + (leader_metric.latency_var * leader_metric.latency_var) * orig_size_f / mod_size_f;

    leader_metric.latency_avg = latency_avg_new;
    leader_metric.latency_var = latency_disp_new.sqrt();

    save_metrics_at_path(&metrics, path)
}
