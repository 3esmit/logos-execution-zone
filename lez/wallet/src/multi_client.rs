use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result};
use sequencer_service_rpc::{RpcClient, SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::config::SequencerConnectionData;

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

impl Metrics {
    fn apply_updates(&mut self, updates: &[MetricsUpdate]) {
        let CumulativeUpdates {
            failure_count,
            latest_block_id,
            cumulative_latency,
            cumulative_latency_squares,
            additional_sample_size,
        } = cumulative_updates(updates);

        self.errors += failure_count;
        if let Some(latest_block_id) = latest_block_id {
            self.latest_block_id = latest_block_id;
        }

        let orig_size_f = self.sample_size as f32;
        let mod_size_f = additional_sample_size as f32;

        let latency_avg_old = self.latency_avg;
        let latency_avg_new =
            cumulative_avg(latency_avg_old, cumulative_latency, orig_size_f, mod_size_f);

        let latency_var_new = cumulative_var(
            latency_avg_old,
            latency_avg_new,
            self.latency_var,
            cumulative_latency,
            cumulative_latency_squares,
            orig_size_f,
            mod_size_f,
        );

        self.latency_avg = latency_avg_new;
        self.latency_var = latency_var_new;
        self.sample_size += additional_sample_size;
    }
}

#[derive(Clone)]
pub struct MultiSequencerClient {
    // For now we store only leader, it is possible, that
    // in future for important sends(for example for transactions)
    // we would want to distribute call between known sequencers
    pub leader: SequencerClient,
    pub leader_url: Url,
}

impl MultiSequencerClient {
    pub async fn new(
        conn_data: &[SequencerConnectionData],
        metrics: &mut HashMap<Url, Metrics>,
        callibration_limit: usize,
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
                    callibrate_client(&sequencer_client, callibration_limit).await,
                );
            // Otherwise actualize client data
            } else {
                let metric_updates = actualize_client(&sequencer_client).await;
                let metric_mut = metrics.get_mut(sequencer_addr).unwrap();
                metric_mut.apply_updates(&[metric_updates]);
            }

            client_list.insert(sequencer_addr.clone(), sequencer_client);
        }

        let (leader_url, leader) =
            choose_leader(&client_list, metrics).ok_or(anyhow::anyhow!("Failed to find leader"))?;

        // Dropping client list, for reasons why, see comment in structure definition.
        Ok(Self { leader, leader_url })
    }

    pub fn leader_ref(&self) -> &SequencerClient {
        &self.leader
    }

    pub fn leader_clone(&self) -> SequencerClient {
        self.leader.clone()
    }

    // Keeping this call abstract, in case if we need to do more than one request
    pub async fn metered_call<R, E, I: AsyncFn(&SequencerClient) -> Result<R, E>>(
        &self,
        call: I,
    ) -> (Result<R, E>, MetricsUpdate) {
        tokio::join!(call(self.leader_ref()), actualize_client(self.leader_ref()))
    }
}

pub async fn callibrate_client(client: &SequencerClient, callibration_limit: usize) -> Metrics {
    let mut latencies = vec![];
    let mut latest_block_id = 0;
    let mut errors = 0;

    // ToDo: Add some DDoS adaptation
    for _ in 0..callibration_limit {
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
    let latency_var = latencies.iter().fold(0f32, |acc, x| {
        acc + ((*x as f32) - latency_avg) * ((*x as f32) - latency_avg)
    }) / (sample_size as f32);

    Metrics {
        latency_avg,
        latency_var,
        sample_size,
        latest_block_id,
        errors,
    }
}

pub async fn actualize_client(client: &SequencerClient) -> MetricsUpdate {
    let now = tokio::time::Instant::now();

    let block_id = client.get_last_block_id().await.ok();

    let latency = tokio::time::Instant::now().duration_since(now).as_millis() as f32;

    MetricsUpdate {
        latency,
        new_latest_block_id: block_id,
        is_failed: block_id.is_none(),
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

    // Considering the nature of our requests, the latest_block_id is the dominant characteristic
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

    // Sort out all clients running late
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

    // Get the clients with lesser or equal to average error count
    let avg_err_count = client_vec
        .iter()
        .fold(0, |acc, x| acc + metrics.get(*x).unwrap().errors)
        / (client_vec.len() as u64);

    client_vec.sort_by(|a, b| {
        metrics
            .get(*a)
            .unwrap()
            .errors
            .cmp(&metrics.get(*b).unwrap().errors)
    });

    client_vec = client_vec[..(client_vec
        .iter()
        .position(|item| metrics.get(*item).unwrap().errors > avg_err_count)
        .unwrap_or(client_vec.len()))]
        .to_vec();

    // Choose clients with least latency and variance
    let min_lat_var_addr = client_vec.iter().fold(client_vec[0], |acc, x| {
        let old = metrics.get(acc).unwrap();
        let (old_lat, old_var) = (old.latency_avg, old.latency_var);
        let new = metrics.get(*x).unwrap();
        let (new_lat, new_var) = (new.latency_avg, new.latency_var);

        let new_std = new_var.sqrt();
        let old_std = old_var.sqrt();

        // Client is better if its averabe is better and variance does not make it worse
        // So basically we want this:
        // [-old_std............new_lat.......old_lat...............+new_std.........+old_std]
        //
        // However one can argue that this:
        //
        // [-old_std...................old_lat........new_lat.........+new_std.......+old_std]
        //
        // is still better, but it is up to discussion
        if (new_lat <= old_lat) && ((new_lat + new_std) < (old_lat + old_std)) {
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

pub async fn save_metrics_at_path(
    metrics: &HashMap<Url, Metrics>,
    path: &Path,
) -> Result<(), anyhow::Error> {
    let metrics_serialized = serde_json::to_vec_pretty(metrics)?;
    let mut file = tokio::fs::File::create(path)
        .await
        .context("Failed to create file")?;
    file.write_all(&metrics_serialized)
        .await
        .context("Failed to write to file")?;
    file.sync_all().await.context("Failed to sync file")?;

    Ok(())
}

struct CumulativeUpdates {
    pub failure_count: u64,
    pub latest_block_id: Option<u64>,
    /// Necessary for cumulative average calculation
    pub cumulative_latency: f32,
    /// Necessary for cumulative variance calculation
    pub cumulative_latency_squares: f32,
    pub additional_sample_size: usize,
}

fn cumulative_updates(metric_updates: &[MetricsUpdate]) -> CumulativeUpdates {
    let (failure_count, latest_block_id, cumulative_latency, cumulative_latency_squares) =
        metric_updates
            .iter()
            .fold((0u64, None, 0f32, 0_f32), |acc, x| {
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
                    if !*is_failed {
                        acc.3 + latency * latency
                    } else {
                        acc.3
                    },
                )
            });

    CumulativeUpdates {
        failure_count,
        latest_block_id: latest_block_id.copied(),
        cumulative_latency,
        cumulative_latency_squares,
        additional_sample_size: metric_updates.len() - (failure_count as usize),
    }
}

/// Helperfunction to calculate cumulative average
///
/// Cumulative average calculation is the following problem:
///
/// We want to calculate avarage of a sample of size `N + N_1`
/// where average for `N` is known
///
/// To do so we need:
/// - old average value
/// - sum_{i=1}^{N_1}{n_i}
/// - N
/// - N_1
fn cumulative_avg(
    latency_avg_old: f32,
    cumulative_latency: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    (latency_avg_old * orig_size_f + cumulative_latency) / (orig_size_f + mod_size_f)
}

/// Helperfunction to calculate cumulative variance
///
/// Cumulative variance calculation is the following problem:
///
/// We want to calculate variance of a sample of size `N + N_1`
/// where average for `N` is known
///
/// To do so we need:
/// - old average value
/// - new average value
/// - old variance
/// - sum_{i=1}^{N_1}{n_i}
/// - sum_{i=1}^{N_1}{n_i^2}
/// - N
/// - N_1
fn cumulative_var(
    latency_avg_old: f32,
    latency_avg_new: f32,
    latency_var: f32,
    cumulative_latency: f32,
    cumulative_latency_squares: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    (latency_var * orig_size_f
        + (latency_avg_new - latency_avg_old) * (latency_avg_new - latency_avg_old) * orig_size_f
        + cumulative_latency_squares
        + mod_size_f * (latency_avg_new * latency_avg_new)
        - 2_f32 * cumulative_latency * latency_avg_new)
        / (orig_size_f + mod_size_f)
}

pub fn update_metrics(
    metrics: &mut HashMap<Url, Metrics>,
    leader_url: &Url,
    metric_updates: &[MetricsUpdate],
) -> Result<(), anyhow::Error> {
    let leader_metric = metrics
        .get_mut(leader_url)
        .ok_or(anyhow::anyhow!("Leader URL is not present in metrics"))?;

    leader_metric.apply_updates(metric_updates);

    Ok(())
}

pub async fn save_metrics_at_path_with_updates(
    mut metrics: HashMap<Url, Metrics>,
    leader_url: &Url,
    metric_updates: &[MetricsUpdate],
    path: &Path,
) -> Result<(), anyhow::Error> {
    update_metrics(&mut metrics, leader_url, metric_updates)?;

    save_metrics_at_path(&metrics, path).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sequencer_service_rpc::{SequencerClient, SequencerClientBuilder};
    use url::Url;

    use crate::multi_client::{
        CumulativeUpdates, Metrics, MetricsUpdate, choose_leader, cumulative_avg,
        cumulative_updates, cumulative_var, update_metrics,
    };

    fn client_from_url_unchecked(url: &Url) -> SequencerClient {
        let builder = SequencerClientBuilder::default();
        builder.build(url).unwrap()
    }

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

        let CumulativeUpdates {
            failure_count,
            latest_block_id,
            cumulative_latency,
            cumulative_latency_squares,
            additional_sample_size,
        } = cumulative_updates(&metrics_updates_vec);

        let epsilon = 0.01_f32;

        let sum_squared_manual = 100_f32 * 100_f32 + 115_f32 * 115_f32;

        assert_eq!(additional_sample_size, 2);
        assert_eq!(failure_count, 1);
        assert_eq!(latest_block_id, Some(16));
        assert!((cumulative_latency - 215_f32).abs() < epsilon);
        assert!((cumulative_latency_squares - sum_squared_manual).abs() < epsilon);
    }

    #[test]
    fn cumulative_avg_test() {
        let mut sample = vec![100_f32; 40];

        let old_sample_size_f = sample.len() as f32;
        let old_avg = sample.iter().sum::<f32>() / old_sample_size_f;

        let new_samples = vec![
            101_f32, 110_f32, 112_f32, 97_f32, 78_f32, 25_f32, 75_f32, 189_f32, 120_f32, 50_f32,
        ];
        let mod_sample_size_f = new_samples.len() as f32;
        let cumulative = new_samples.iter().sum();

        sample.extend_from_slice(&new_samples);
        let new_sample_size_f = sample.len() as f32;
        let new_avg_1 = sample.iter().sum::<f32>() / new_sample_size_f;

        let new_avg_2 = cumulative_avg(old_avg, cumulative, old_sample_size_f, mod_sample_size_f);

        let epsilon = 0.01_f32;

        assert!((new_avg_1 - new_avg_2).abs() < epsilon);
    }

    #[test]
    fn cumulative_var_test() {
        let mut sample = vec![100_f32; 40];

        let old_sample_size_f = sample.len() as f32;
        let old_avg = sample.iter().sum::<f32>() / old_sample_size_f;

        let old_var = sample
            .iter()
            .fold(0_f32, |acc, x| acc + (x - old_avg) * (x - old_avg))
            / old_sample_size_f;

        let new_samples = vec![
            101_f32, 110_f32, 112_f32, 97_f32, 78_f32, 25_f32, 75_f32, 189_f32, 120_f32, 50_f32,
        ];
        let mod_sample_size_f = new_samples.len() as f32;
        let cumulative = new_samples.iter().sum();
        let cumulative_squares = new_samples.iter().fold(0_f32, |acc, x| acc + (*x) * (*x));

        let new_avg = cumulative_avg(old_avg, cumulative, old_sample_size_f, mod_sample_size_f);

        sample.extend_from_slice(&new_samples);
        let new_var_1 = sample
            .iter()
            .fold(0_f32, |acc, x| acc + (x - new_avg) * (x - new_avg))
            / (sample.len() as f32);

        let new_var_2 = cumulative_var(
            old_avg,
            new_avg,
            old_var,
            cumulative,
            cumulative_squares,
            old_sample_size_f,
            mod_sample_size_f,
        );

        let epsilon = 0.01_f32;

        assert!((new_var_1 - new_var_2).abs() < epsilon);
    }

    #[test]
    fn metric_updates_correctness() {
        let metrics_updates_vec = vec![
            MetricsUpdate {
                latency: 100_f32,
                new_latest_block_id: Some(105),
                is_failed: false,
            },
            MetricsUpdate {
                latency: 115_f32,
                new_latest_block_id: Some(106),
                is_failed: false,
            },
            MetricsUpdate {
                latency: 50_f32,
                new_latest_block_id: None,
                is_failed: true,
            },
        ];

        let addr_leader = Url::parse("https://127.0.0.1:3040").unwrap();

        let leader_metrics = Metrics {
            latency_avg: 100_f32,
            latency_var: 25_f32,
            sample_size: 10,
            latest_block_id: 100,
            errors: 5,
        };

        let cumulative_latency = 100_f32 + 115_f32;
        let cumulative_latency_squares = 100_f32 * 100_f32 + 115_f32 * 115_f32;

        let avg_manual = cumulative_avg(
            leader_metrics.latency_avg,
            cumulative_latency,
            10_f32,
            2_f32,
        );
        let var_manual = cumulative_var(
            leader_metrics.latency_avg,
            avg_manual,
            leader_metrics.latency_var,
            cumulative_latency,
            cumulative_latency_squares,
            10_f32,
            2_f32,
        );

        let mut metric_map = HashMap::new();
        metric_map.insert(addr_leader.clone(), leader_metrics);

        update_metrics(&mut metric_map, &addr_leader, &metrics_updates_vec).unwrap();

        let Metrics {
            latency_avg,
            latency_var,
            sample_size,
            latest_block_id,
            errors,
        } = metric_map.get(&addr_leader).unwrap();

        let epsilon = 0.01_f32;

        assert_eq!(*errors, 6);
        assert_eq!(*latest_block_id, 106);
        assert_eq!(*sample_size, 12);
        assert!((*latency_avg - avg_manual).abs() < epsilon);
        assert!((*latency_var - var_manual).abs() < epsilon);
    }

    #[test]
    fn choose_leader_latest_block() {
        let addr_leader = Url::parse("http://127.0.0.1:3040").unwrap();
        let addr_1 = Url::parse("http://127.0.0.1:3041").unwrap();
        let addr_2 = Url::parse("http://127.0.0.1:3042").unwrap();
        let addr_3 = Url::parse("http://127.0.0.1:3043").unwrap();

        let leader = client_from_url_unchecked(&addr_leader);
        let client_1 = client_from_url_unchecked(&addr_1);
        let client_2 = client_from_url_unchecked(&addr_2);
        let client_3 = client_from_url_unchecked(&addr_3);

        let mut client_list = HashMap::new();

        client_list.insert(addr_leader.clone(), leader);
        client_list.insert(addr_1.clone(), client_1);
        client_list.insert(addr_2.clone(), client_2);
        client_list.insert(addr_3.clone(), client_3);

        let mut metrics = HashMap::new();

        metrics.insert(
            addr_3,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 97,
                errors: 5,
            },
        );

        metrics.insert(
            addr_2,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 98,
                errors: 5,
            },
        );

        metrics.insert(
            addr_1,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 99,
                errors: 5,
            },
        );

        metrics.insert(
            addr_leader.clone(),
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        let (leader_url, _) = choose_leader(&client_list, &metrics).unwrap();

        assert_eq!(leader_url, addr_leader);
    }

    #[test]
    fn choose_leader_least_errors() {
        let addr_leader = Url::parse("http://127.0.0.1:3040").unwrap();
        let addr_1 = Url::parse("http://127.0.0.1:3041").unwrap();
        let addr_2 = Url::parse("http://127.0.0.1:3042").unwrap();
        let addr_3 = Url::parse("http://127.0.0.1:3043").unwrap();

        let leader = client_from_url_unchecked(&addr_leader);
        let client_1 = client_from_url_unchecked(&addr_1);
        let client_2 = client_from_url_unchecked(&addr_2);
        let client_3 = client_from_url_unchecked(&addr_3);

        let mut client_list = HashMap::new();

        client_list.insert(addr_leader.clone(), leader);
        client_list.insert(addr_1.clone(), client_1);
        client_list.insert(addr_2.clone(), client_2);
        client_list.insert(addr_3.clone(), client_3);

        let mut metrics = HashMap::new();

        metrics.insert(
            addr_3,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_2,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 4,
            },
        );

        metrics.insert(
            addr_1,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 3,
            },
        );

        metrics.insert(
            addr_leader.clone(),
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 2,
            },
        );

        let (leader_url, _) = choose_leader(&client_list, &metrics).unwrap();

        assert_eq!(leader_url, addr_leader);
    }

    #[test]
    fn choose_leader_simple_latency_check() {
        let addr_leader = Url::parse("http://127.0.0.1:3040").unwrap();
        let addr_1 = Url::parse("http://127.0.0.1:3041").unwrap();
        let addr_2 = Url::parse("http://127.0.0.1:3042").unwrap();
        let addr_3 = Url::parse("http://127.0.0.1:3043").unwrap();

        let leader = client_from_url_unchecked(&addr_leader);
        let client_1 = client_from_url_unchecked(&addr_1);
        let client_2 = client_from_url_unchecked(&addr_2);
        let client_3 = client_from_url_unchecked(&addr_3);

        let mut client_list = HashMap::new();

        client_list.insert(addr_leader.clone(), leader);
        client_list.insert(addr_1.clone(), client_1);
        client_list.insert(addr_2.clone(), client_2);
        client_list.insert(addr_3.clone(), client_3);

        let mut metrics = HashMap::new();

        metrics.insert(
            addr_3,
            Metrics {
                latency_avg: 103_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_2,
            Metrics {
                latency_avg: 102_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_1,
            Metrics {
                latency_avg: 101_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_leader.clone(),
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        let (leader_url, _) = choose_leader(&client_list, &metrics).unwrap();

        assert_eq!(leader_url, addr_leader);
    }

    #[test]
    fn choose_leader_latency_var_check() {
        let addr_leader = Url::parse("http://127.0.0.1:3040").unwrap();
        let addr_1 = Url::parse("http://127.0.0.1:3041").unwrap();
        let addr_2 = Url::parse("http://127.0.0.1:3042").unwrap();
        let addr_3 = Url::parse("http://127.0.0.1:3043").unwrap();

        let leader = client_from_url_unchecked(&addr_leader);
        let client_1 = client_from_url_unchecked(&addr_1);
        let client_2 = client_from_url_unchecked(&addr_2);
        let client_3 = client_from_url_unchecked(&addr_3);

        let mut client_list = HashMap::new();

        client_list.insert(addr_leader.clone(), leader);
        client_list.insert(addr_1.clone(), client_1);
        client_list.insert(addr_2.clone(), client_2);
        client_list.insert(addr_3.clone(), client_3);

        let mut metrics = HashMap::new();

        metrics.insert(
            addr_3,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 13_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_2,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 12_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_1,
            Metrics {
                latency_avg: 100_f32,
                latency_var: 11_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        metrics.insert(
            addr_leader.clone(),
            Metrics {
                latency_avg: 100_f32,
                latency_var: 10_f32,
                sample_size: 10,
                latest_block_id: 100,
                errors: 5,
            },
        );

        let (leader_url, _) = choose_leader(&client_list, &metrics).unwrap();

        assert_eq!(leader_url, addr_leader);
    }
}
