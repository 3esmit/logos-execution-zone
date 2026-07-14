#![expect(
    clippy::float_arithmetic,
    reason = "One should expect floating point arithmetic in statistic calculations"
)]
#![expect(
    clippy::cast_precision_loss,
    reason = "Operated numbers is not big enough to have precision loss"
)]

use std::{collections::HashMap, path::Path};

use anyhow::{Context as _, Result};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::SequencerConnectionData;

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
    pub fn apply_updates(&mut self, updates: &[MetricsUpdate]) {
        let CumulativeUpdates {
            failure_count,
            latest_block_id,
            cumulative_latency,
            cumulative_latency_squares,
            additional_sample_size,
        } = CumulativeUpdates::from_metric_updates(updates);

        self.errors = self.errors.saturating_add(failure_count);
        if let Some(latest_block_id) = latest_block_id {
            self.latest_block_id = latest_block_id;
        }

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let orig_size_f = self.sample_size as f32;
        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
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
        self.sample_size = self.sample_size.saturating_add(additional_sample_size);
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
            if metrics.contains_key(sequencer_addr) {
                let metric_updates = actualize_client(&sequencer_client).await;

                log::debug!(
                    "Metered call for {sequencer_addr:?}, metric updates is {metric_updates:?}"
                );

                let metric_mut = metrics.get_mut(sequencer_addr).unwrap();
                metric_mut.apply_updates(&[metric_updates]);
            // Otherwise actualize client data
            } else {
                metrics.insert(
                    sequencer_addr.clone(),
                    callibrate_client(&sequencer_client, callibration_limit).await,
                );
            }

            client_list.insert(sequencer_addr.clone(), sequencer_client);
        }

        let (leader_url, leader) = choose_leader(&client_list, metrics)
            .ok_or_else(|| anyhow::anyhow!("Failed to find leader"))?;

        log::info!("Chosen leader is {leader_url:?}");

        // Dropping client list, for reasons why, see comment in structure definition.
        Ok(Self { leader, leader_url })
    }

    #[must_use]
    pub const fn leader_ref(&self) -> &SequencerClient {
        &self.leader
    }

    #[must_use]
    pub fn leader_clone(&self) -> SequencerClient {
        self.leader.clone()
    }

    // Keeping this call abstract, in case if we need to do more than one request
    pub async fn metered_call<R, E, I: AsyncFn(&SequencerClient) -> Result<R, E>>(
        &self,
        call: I,
    ) -> (Result<R, E>, MetricsUpdate) {
        let resp = tokio::join!(call(self.leader_ref()), actualize_client(self.leader_ref()));

        log::debug!(
            "Metered call for {:?}, metric updates is {:?}",
            self.leader_url,
            resp.1
        );

        resp
    }
}

struct CumulativeUpdates {
    pub failure_count: u64,
    pub latest_block_id: Option<u64>,
    /// Necessary for cumulative average calculation.
    pub cumulative_latency: f32,
    /// Necessary for cumulative variance calculation.
    pub cumulative_latency_squares: f32,
    pub additional_sample_size: usize,
}

impl CumulativeUpdates {
    fn from_metric_updates(metric_updates: &[MetricsUpdate]) -> Self {
        let (failure_count, latest_block_id, cumulative_latency, cumulative_latency_squares) =
            metric_updates
                .iter()
                .fold((0_u64, None, 0_f32, 0_f32), |acc, x| {
                    let MetricsUpdate {
                        latency,
                        new_latest_block_id,
                        is_failed,
                    } = x;
                    (
                        if *is_failed {
                            acc.0.saturating_add(1)
                        } else {
                            acc.0
                        },
                        match (acc.1, new_latest_block_id) {
                            (None, None) => None,
                            (None, Some(val)) | (Some(val), None) => Some(val),
                            (Some(val_old), Some(val_new)) => Some(std::cmp::max(val_old, val_new)),
                        },
                        if *is_failed { acc.2 } else { acc.2 + latency },
                        if *is_failed {
                            acc.3
                        } else {
                            latency.mul_add(*latency, acc.3)
                        },
                    )
                });

        Self {
            failure_count,
            latest_block_id: latest_block_id.copied(),
            cumulative_latency,
            cumulative_latency_squares,
            additional_sample_size: metric_updates.len().saturating_sub(
                usize::try_from(failure_count).expect("Sample size should fit usize"),
            ),
        }
    }
}

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

pub async fn callibrate_client(client: &SequencerClient, callibration_limit: usize) -> Metrics {
    let mut latencies = vec![];
    let mut latest_block_id = 0;
    let mut errors: u64 = 0;

    // ToDo: Add some DDoS adaptation
    for _ in 0..callibration_limit {
        let now = tokio::time::Instant::now();

        let block_id = client.get_last_block_id().await;

        let latency = tokio::time::Instant::now().duration_since(now).as_millis();

        let Ok(block_id) = block_id else {
            errors = errors.saturating_add(1);
            continue;
        };

        latest_block_id = block_id;
        latencies.push(latency);
    }

    // Precision loss if fine there
    let sample_size = latencies.len();
    #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
    let latency_avg = (latencies.iter().sum::<u128>() as f32) / (sample_size as f32);
    #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
    let latency_var = latencies.iter().fold(0_f32, |acc, x| {
        ((*x as f32) - latency_avg).mul_add((*x as f32) - latency_avg, acc)
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

    #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
    let latency = tokio::time::Instant::now().duration_since(now).as_millis() as f32;

    MetricsUpdate {
        latency,
        new_latest_block_id: block_id,
        is_failed: block_id.is_none(),
    }
}

#[must_use]
pub fn choose_leader(
    client_list: &HashMap<Url, SequencerClient>,
    metrics: &HashMap<Url, Metrics>,
) -> Option<(Url, SequencerClient)> {
    let mut client_vec = vec![];

    // Sort out all unmetered clients
    client_vec = client_list
        .keys()
        .filter(|item| metrics.contains_key(*item))
        .collect();

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

            (latest_block_id == max_block_id).then_some(*x)
        })
        .collect();

    // Get the clients with lesser or equal to average error count
    #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
    let avg_err_count = (client_vec.iter().fold(0_u64, |acc, x| {
        acc.saturating_add(metrics.get(*x).unwrap().errors)
    }) as f32)
        / (client_vec.len() as f32);

    client_vec.sort_by(|a, b| {
        metrics
            .get(*a)
            .unwrap()
            .errors
            .cmp(&metrics.get(*b).unwrap().errors)
    });

    #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
    let client_vec = client_vec[..(client_vec
        .iter()
        .position(|item| (metrics.get(*item).unwrap().errors as f32) > avg_err_count)
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

/// Helperfunction to calculate cumulative average.
///
/// Cumulative average calculation is the following problem:
///
/// We want to calculate avarage of a sample of size `N + N_1`
/// where average for `N` is known.
///
/// To do so we need:
/// - old average value
/// - sum_{`i=1}^{N_1}{n_i`}
/// - `N`
/// - `N_1`
fn cumulative_avg(
    latency_avg_old: f32,
    cumulative_latency: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    latency_avg_old.mul_add(orig_size_f, cumulative_latency) / (orig_size_f + mod_size_f)
}

/// Helperfunction to calculate cumulative variance.
///
/// Cumulative variance calculation is the following problem:
///
/// We want to calculate variance of a sample of size `N + N_1`
/// where average for `N` is known.
///
/// To do so we need:
/// - old average value
/// - new average value
/// - old variance
/// - sum_{`i=1}^{N_1}{n_i`}
/// - sum_{`i=1}^{N_1}{n_i^2`}
/// - `N`
/// - `N_1`
fn cumulative_var(
    latency_avg_old: f32,
    latency_avg_new: f32,
    latency_var: f32,
    cumulative_latency: f32,
    cumulative_latency_squares: f32,
    orig_size_f: f32,
    mod_size_f: f32,
) -> f32 {
    // The formula was atrocious before.
    // `mul_add` function have less precision loss with drawback of being absolutely unreadable
    ((2_f32 * cumulative_latency).mul_add(
        -latency_avg_new,
        mod_size_f.mul_add(
            latency_avg_new * latency_avg_new,
            latency_var.mul_add(
                orig_size_f,
                (latency_avg_new - latency_avg_old)
                    * (latency_avg_new - latency_avg_old)
                    * orig_size_f,
            ),
        ),
    ) + cumulative_latency_squares)
        / (orig_size_f + mod_size_f)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sequencer_service_rpc::{SequencerClient, SequencerClientBuilder};
    use url::Url;

    use crate::multi_client::{
        CumulativeUpdates, Metrics, MetricsUpdate, choose_leader, cumulative_avg, cumulative_var,
    };

    fn update_metrics(
        metrics: &mut HashMap<Url, Metrics>,
        leader_url: &Url,
        metric_updates: &[MetricsUpdate],
    ) -> Result<(), anyhow::Error> {
        let leader_metric = metrics
            .get_mut(leader_url)
            .ok_or_else(|| anyhow::anyhow!("Leader URL is not present in metrics"))?;

        leader_metric.apply_updates(metric_updates);

        Ok(())
    }

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
        } = CumulativeUpdates::from_metric_updates(&metrics_updates_vec);

        let epsilon = 0.01_f32;

        let sum_squared_manual = 100_f32.mul_add(100_f32, 115_f32 * 115_f32);

        assert_eq!(additional_sample_size, 2);
        assert_eq!(failure_count, 1);
        assert_eq!(latest_block_id, Some(16));
        assert!((cumulative_latency - 215_f32).abs() < epsilon);
        assert!((cumulative_latency_squares - sum_squared_manual).abs() < epsilon);
    }

    #[test]
    fn cumulative_avg_test() {
        let mut sample = vec![100_f32; 40];

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let old_sample_size_f = sample.len() as f32;

        let old_avg = sample.iter().sum::<f32>() / old_sample_size_f;

        let new_samples = vec![
            101_f32, 110_f32, 112_f32, 97_f32, 78_f32, 25_f32, 75_f32, 189_f32, 120_f32, 50_f32,
        ];

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let mod_sample_size_f = new_samples.len() as f32;

        let cumulative = new_samples.iter().sum();

        sample.extend_from_slice(&new_samples);

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let new_sample_size_f = sample.len() as f32;

        let new_avg_1 = sample.iter().sum::<f32>() / new_sample_size_f;

        let new_avg_2 = cumulative_avg(old_avg, cumulative, old_sample_size_f, mod_sample_size_f);

        let epsilon = 0.01_f32;

        assert!((new_avg_1 - new_avg_2).abs() < epsilon);
    }

    #[test]
    fn cumulative_var_test() {
        let mut sample = vec![100_f32; 40];

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let old_sample_size_f = sample.len() as f32;
        let old_avg = sample.iter().sum::<f32>() / old_sample_size_f;

        let old_var = sample
            .iter()
            .fold(0_f32, |acc, x| (x - old_avg).mul_add(x - old_avg, acc))
            / old_sample_size_f;

        let new_samples = vec![
            101_f32, 110_f32, 112_f32, 97_f32, 78_f32, 25_f32, 75_f32, 189_f32, 120_f32, 50_f32,
        ];

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let mod_sample_size_f = new_samples.len() as f32;

        let cumulative = new_samples.iter().sum();
        let cumulative_squares = new_samples
            .iter()
            .fold(0_f32, |acc, x| (*x).mul_add(*x, acc));

        let new_avg = cumulative_avg(old_avg, cumulative, old_sample_size_f, mod_sample_size_f);

        sample.extend_from_slice(&new_samples);

        #[expect(clippy::as_conversions, reason = "int to float conversion is safe")]
        let new_var_1 = sample
            .iter()
            .fold(0_f32, |acc, x| (x - new_avg).mul_add(x - new_avg, acc))
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
        let cumulative_latency_squares = 100_f32.mul_add(100_f32, 115_f32 * 115_f32);

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
        } = metric_map[&addr_leader];

        let epsilon = 0.01_f32;

        assert_eq!(errors, 6);
        assert_eq!(latest_block_id, 106);
        assert_eq!(sample_size, 12);
        assert!((latency_avg - avg_manual).abs() < epsilon);
        assert!((latency_var - var_manual).abs() < epsilon);
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
