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

        let (leader_url, leader) =
            choose_leader(&client_list, metrics).ok_or(anyhow::anyhow!("Failed to find leader"))?;

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
    client_list: &HashMap<Url, SequencerClient>,
    metrics: &HashMap<Url, Metrics>,
) -> Option<(Url, SequencerClient)> {
    let mut client_vec = vec![];

    // Sort out all unmetered clients
    for addr in client_list.keys() {
        if metrics.contains_key(addr) {
            client_vec.push(addr);
        }
    }

    if client_vec.is_empty() {
        return None;
    }

    // Considering the nature of our requests, the latest_block_id is dominant characteristic
    let max_block_id_addr = client_vec.iter().fold(client_vec[0], |acc, x| {
        let old_latest_block_id = metrics.get(acc).unwrap().latest_block_id;
        let new_latest_block_id = metrics.get(*x).unwrap().latest_block_id;
        if new_latest_block_id > old_latest_block_id {
            *x
        } else {
            acc
        }
    });

    let max_block_id = metrics.get(max_block_id_addr).unwrap().latest_block_id;

    // Sort out all latest clients
    client_vec = client_vec
        .iter()
        .filter_map(|x| {
            let latest_block_id = metrics.get(*x).unwrap().latest_block_id;

            if latest_block_id == max_block_id {
                Some(*x)
            } else {
                None
            }
        })
        .collect();

    // Conservative assumption: least amount of errors is the best
    let min_err_addr = client_vec.iter().fold(client_vec[0], |acc, x| {
        let old_err_count = metrics.get(acc).unwrap().errors;
        let new_err_count = metrics.get(*x).unwrap().errors;
        if new_err_count < old_err_count {
            *x
        } else {
            acc
        }
    });

    let min_err_count = metrics.get(min_err_addr).unwrap().errors;

    // Sort out all latest clients
    client_vec = client_vec
        .iter()
        .filter_map(|x| {
            let err_count = metrics.get(*x).unwrap().errors;

            if err_count == min_err_count {
                Some(*x)
            } else {
                None
            }
        })
        .collect();

    // Choose clients with least latency and variance
    let min_lat_var_addr = client_vec.iter().fold(client_vec[0], |acc, x| {
        let old = metrics.get(acc).unwrap();
        let (old_lat, old_var) = (old.latency_avg, old.latency_var);
        let new = metrics.get(*x).unwrap();
        let (new_lat, new_var) = (new.latency_avg, new.latency_var);

        // Client is better if it is strongly linearly better
        if ((new_lat - new_var) > (old_lat - old_var))
            && ((new_lat + new_var) > (old_lat + old_var))
        {
            *x
        } else {
            acc
        }
    });

    Some((
        min_lat_var_addr.clone(),
        client_list.get(min_lat_var_addr).unwrap().clone(),
    ))
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

struct CumulativeUpdates {
    pub failure_count: u64,
    pub latest_block_id: Option<u64>,
    pub cumulative_latency: f32,
    pub additional_sample_size: usize,
}

fn cumulative_updates(metric_updates: &[MetricsUpdate]) -> CumulativeUpdates {
    let (failure_count, latest_block_id, cumulative_latency) =
        metric_updates.iter().fold((0u64, None, 0f32), |acc, x| {
            let MetricsUpdate {
                latency,
                new_latest_block_id,
                is_failed,
            } = x;
            (
                if *is_failed { acc.0 + 1 } else { acc.0 },
                match (acc.1, new_latest_block_id) {
                    (None, None) => None,
                    (None, Some(val)) | (Some(val), None) => Some(val),
                    (Some(val_old), Some(val_new)) => Some(std::cmp::max(val_old, val_new)),
                },
                if !*is_failed { acc.2 + latency } else { acc.2 },
            )
        });

    CumulativeUpdates {
        failure_count,
        latest_block_id: latest_block_id.copied(),
        cumulative_latency,
        additional_sample_size: metric_updates.len() - (failure_count as usize),
    }
}

fn cumulative_avg(
    latency_avg_old: f32,
    cumulative_latency: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    (latency_avg_old * orig_size_f + cumulative_latency) / mod_size_f
}

fn cumulative_var(
    latency_avg_old: f32,
    latency_avg_new: f32,
    latency_var: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    (latency_avg_old - latency_avg_new)
        * ((3f32 * latency_avg_old + latency_avg_new) * orig_size_f / mod_size_f)
        + (latency_var * latency_var) * orig_size_f / mod_size_f
}

pub fn update_metrics(
    metrics: &mut HashMap<Url, Metrics>,
    leader_url: &Url,
    metric_updates: &[MetricsUpdate],
) -> Result<(), anyhow::Error> {
    let leader_metric = metrics
        .get_mut(leader_url)
        .ok_or(anyhow::anyhow!("Leader URL is in present in metrics"))?;

    let CumulativeUpdates {
        failure_count,
        latest_block_id,
        cumulative_latency,
        additional_sample_size,
    } = cumulative_updates(metric_updates);

    leader_metric.errors += failure_count;
    if let Some(latest_block_id) = latest_block_id {
        leader_metric.latest_block_id = latest_block_id;
    }

    let orig_size_f = leader_metric.sample_size as f32;
    let mod_size_f = (leader_metric.sample_size + additional_sample_size) as f32;

    let latency_avg_old = leader_metric.latency_avg;
    let latency_avg_new =
        cumulative_avg(latency_avg_old, cumulative_latency, orig_size_f, mod_size_f);

    let latency_disp_new = cumulative_var(
        latency_avg_old,
        latency_avg_new,
        leader_metric.latency_var,
        orig_size_f,
        mod_size_f,
    );

    leader_metric.latency_avg = latency_avg_new;
    leader_metric.latency_var = latency_disp_new.sqrt();

    Ok(())
}

pub fn save_metrics_at_path_with_updates(
    mut metrics: HashMap<Url, Metrics>,
    leader_url: &Url,
    metric_updates: &[MetricsUpdate],
    path: &Path,
) -> Result<(), anyhow::Error> {
    update_metrics(&mut metrics, leader_url, metric_updates)?;

    save_metrics_at_path(&metrics, path)
}

#[cfg(test)]
mod tests {
    use crate::multi_client::{MetricsUpdate, cumulative_avg, cumulative_updates};

    #[test]
    fn cumulative_updates_test() {
        let metrics_updates_vec = vec![
            MetricsUpdate {
                latency: 100_f32,
                new_latest_block_id: Some(15),
                is_failed: false,
            },
            MetricsUpdate {
                latency: 115_f32,
                new_latest_block_id: Some(16),
                is_failed: false,
            },
            MetricsUpdate {
                latency: 50_f32,
                new_latest_block_id: None,
                is_failed: true,
            },
        ];

        let cumulative_updates = cumulative_updates(&metrics_updates_vec);

        let epsilon = 0.01_f32;

        assert_eq!(cumulative_updates.additional_sample_size, 2);
        assert_eq!(cumulative_updates.failure_count, 1);
        assert_eq!(cumulative_updates.latest_block_id, Some(16));
        assert!((cumulative_updates.cumulative_latency - 215_f32).abs() < epsilon);
    }

    #[test]
    fn cumulative_avg_test() {
        let mut sample = vec![100_f32; 20];

        let old_sample_size_f = sample.len() as f32;
        let old_avg = sample.iter().sum::<f32>() / old_sample_size_f;

        let new_samples = vec![101_f32, 110_f32, 112_f32, 97_f32, 78_f32];
        let mod_sample_size_f = new_samples.len() as f32 + old_sample_size_f;
        let cumulative = new_samples.iter().sum();

        sample.extend_from_slice(&new_samples);
        let new_sample_size_f = sample.len() as f32;
        let new_avg_1 = sample.iter().sum::<f32>() / new_sample_size_f;

        let new_avg_2 = cumulative_avg(old_avg, cumulative, old_sample_size_f, mod_sample_size_f);

        let epsilon = 0.01_f32;

        assert!((new_avg_1 - new_avg_2).abs() < epsilon);
    }
}
