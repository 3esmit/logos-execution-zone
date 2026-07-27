use std::{io::Write as _, path::Path, time::Duration};

use anyhow::{Context as _, Result};
use common::config::BasicAuth;
use humantime_serde;
use log::warn;
use serde::{Deserialize, Serialize};
use url::Url;

// A wallet without persisted statistics calibrates synchronously during open.
// Keep the default small enough for interactive module hosts; callers that need
// a deeper sample can still set `calibration_limit` explicitly.
const DEFAULT_CALLIBRATION_LIMIT: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerConnectionData {
    /// Connection data of all known sequencers.
    pub sequencer_addr: Url,
    /// Basic authentication credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<BasicAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasConfig {
    /// Gas spent per deploying one byte of data.
    pub gas_fee_per_byte_deploy: u64,
    /// Gas spent per reading one byte of data in VM.
    pub gas_fee_per_input_buffer_runtime: u64,
    /// Gas spent per one byte of contract data in runtime.
    pub gas_fee_per_byte_runtime: u64,
    /// Cost of one gas of runtime in public balance.
    pub gas_cost_runtime: u64,
    /// Cost of one gas of deployment in public balance.
    pub gas_cost_deploy: u64,
    /// Gas limit for deployment.
    pub gas_limit_deploy: u64,
    /// Gas limit for runtime.
    pub gas_limit_runtime: u64,
}

#[optfield::optfield(pub WalletConfigOverrides, rewrap, attrs = (derive(Debug, Default, Clone)))]
#[derive(Debug, Clone, Serialize)]
pub struct WalletConfig {
    /// Connection data of all known sequencers.
    pub sequencers: Vec<SequencerConnectionData>,
    /// Sequencer polling duration for new blocks.
    #[serde(with = "humantime_serde")]
    pub seq_poll_timeout: Duration,
    /// Sequencer polling max number of blocks to find transaction.
    pub seq_tx_poll_max_blocks: usize,
    /// Sequencer polling max number error retries.
    pub seq_poll_max_retries: u64,
    /// Max amount of blocks to poll in one request.
    pub seq_block_poll_max_amount: u64,
    /// Limit number of sequencer polls during calibration, should not be zero
    #[serde(default = "default_calibration_limit")]
    pub calibration_limit: usize,
}

#[derive(Debug, Deserialize)]
struct WalletConfigFile {
    #[serde(default, alias = "sequencers_conn_data")]
    sequencers: Option<Vec<SequencerConnectionData>>,
    #[serde(default)]
    sequencer_addr: Option<Url>,
    #[serde(default)]
    basic_auth: Option<BasicAuth>,
    #[serde(with = "humantime_serde")]
    seq_poll_timeout: Duration,
    seq_tx_poll_max_blocks: usize,
    seq_poll_max_retries: u64,
    seq_block_poll_max_amount: u64,
    #[serde(default = "default_calibration_limit", alias = "callibration_limit")]
    calibration_limit: usize,
}

impl<'de> Deserialize<'de> for WalletConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let file = WalletConfigFile::deserialize(deserializer)?;
        Self::try_from(file).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<WalletConfigFile> for WalletConfig {
    type Error = &'static str;

    fn try_from(file: WalletConfigFile) -> std::result::Result<Self, Self::Error> {
        let WalletConfigFile {
            sequencers,
            sequencer_addr,
            basic_auth,
            seq_poll_timeout,
            seq_tx_poll_max_blocks,
            seq_poll_max_retries,
            seq_block_poll_max_amount,
            calibration_limit,
        } = file;

        let sequencers = match (sequencers, sequencer_addr) {
            (Some(_), Some(_)) => {
                return Err(
                    "wallet config cannot contain both `sequencers` and legacy `sequencer_addr`",
                );
            }
            (Some(sequencers), None) => {
                if basic_auth.is_some() {
                    return Err(
                        "top-level legacy `basic_auth` cannot be combined with `sequencers`",
                    );
                }
                sequencers
            }
            (None, Some(sequencer_addr)) => {
                vec![SequencerConnectionData {
                    sequencer_addr,
                    basic_auth,
                }]
            }
            (None, None) => {
                return Err("wallet config must contain `sequencers` or legacy `sequencer_addr`");
            }
        };

        Ok(Self {
            sequencers,
            seq_poll_timeout,
            seq_tx_poll_max_blocks,
            seq_poll_max_retries,
            seq_block_poll_max_amount,
            calibration_limit,
        })
    }
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            sequencers: vec![SequencerConnectionData {
                sequencer_addr: "http://127.0.0.1:3040".parse().unwrap(),
                basic_auth: None,
            }],
            seq_poll_timeout: Duration::from_secs(12),
            seq_tx_poll_max_blocks: 5,
            seq_poll_max_retries: 5,
            seq_block_poll_max_amount: 100,
            calibration_limit: DEFAULT_CALLIBRATION_LIMIT,
        }
    }
}

impl WalletConfig {
    pub fn from_path_or_initialize_default(config_path: &Path) -> Result<Self> {
        match std::fs::File::open(config_path) {
            Ok(file) => {
                let reader = std::io::BufReader::new(file);
                Ok(serde_json::from_reader(reader)?)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("Config not found, setting up default config");

                let config_home = config_path.parent().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Could not get parent directory of config file at {}",
                        config_path.display()
                    )
                })?;
                std::fs::create_dir_all(config_home)?;

                println!("Created configs dir at path {}", config_home.display());

                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(config_path)?;

                let config = Self::default();
                let default_config_serialized = serde_json::to_vec_pretty(&config).unwrap();

                file.write_all(&default_config_serialized)?;

                println!("Configs set up");
                Ok(config)
            }
            Err(err) => Err(err).context("IO error"),
        }
    }

    pub fn apply_overrides(&mut self, overrides: WalletConfigOverrides) {
        let Self {
            sequencers,
            seq_poll_timeout,
            seq_tx_poll_max_blocks,
            seq_poll_max_retries,
            seq_block_poll_max_amount,
            calibration_limit,
        } = self;

        let WalletConfigOverrides {
            sequencers: o_sequencers,
            seq_poll_timeout: o_seq_poll_timeout,
            seq_tx_poll_max_blocks: o_seq_tx_poll_max_blocks,
            seq_poll_max_retries: o_seq_poll_max_retries,
            seq_block_poll_max_amount: o_seq_block_poll_max_amount,
            calibration_limit: o_calibration_limit,
        } = overrides;

        if let Some(v) = o_sequencers {
            warn!("Overriding wallet config 'sequencers' to {v:?}");
            *sequencers = v;
        }
        if let Some(v) = o_seq_poll_timeout {
            warn!("Overriding wallet config 'seq_poll_timeout' to {v:?}");
            *seq_poll_timeout = v;
        }
        if let Some(v) = o_seq_tx_poll_max_blocks {
            warn!("Overriding wallet config 'seq_tx_poll_max_blocks' to {v}");
            *seq_tx_poll_max_blocks = v;
        }
        if let Some(v) = o_seq_poll_max_retries {
            warn!("Overriding wallet config 'seq_poll_max_retries' to {v}");
            *seq_poll_max_retries = v;
        }
        if let Some(v) = o_seq_block_poll_max_amount {
            warn!("Overriding wallet config 'seq_block_poll_max_amount' to {v}");
            *seq_block_poll_max_amount = v;
        }
        if let Some(v) = o_calibration_limit {
            warn!("Overriding wallet config 'calibration_limit' to {v}");
            *calibration_limit = v;
        }
    }
}

const fn default_calibration_limit() -> usize {
    DEFAULT_CALLIBRATION_LIMIT
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{DEFAULT_CALLIBRATION_LIMIT, WalletConfig};

    fn polling_fields() -> serde_json::Value {
        json!({
            "seq_poll_timeout": "7s",
            "seq_tx_poll_max_blocks": 11,
            "seq_poll_max_retries": 13,
            "seq_block_poll_max_amount": 17
        })
    }

    #[test]
    fn loads_legacy_single_sequencer_configuration_from_path() {
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let config_path = temp_dir.path().join("wallet_config.json");
        let mut value = polling_fields();
        value["sequencer_addr"] = json!("https://legacy.example.test");
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&value).expect("legacy configuration should serialize"),
        )
        .expect("legacy configuration should be written");

        let config = WalletConfig::from_path_or_initialize_default(&config_path)
            .expect("legacy configuration should load");

        assert_eq!(config.sequencers.len(), 1);
        assert_eq!(
            config.sequencers[0].sequencer_addr.as_str(),
            "https://legacy.example.test/"
        );
        assert!(config.sequencers[0].basic_auth.is_none());
        assert_eq!(config.seq_poll_timeout, Duration::from_secs(7));
        assert_eq!(config.seq_tx_poll_max_blocks, 11);
        assert_eq!(config.seq_poll_max_retries, 13);
        assert_eq!(config.seq_block_poll_max_amount, 17);
        assert_eq!(config.calibration_limit, DEFAULT_CALLIBRATION_LIMIT);
    }

    #[test]
    fn omitted_calibration_limit_uses_interactive_default() {
        let mut value = polling_fields();
        value["sequencers"] = json!([{"sequencer_addr": "https://first.example.test"}]);

        let config: WalletConfig =
            serde_json::from_value(value).expect("configuration should load");

        assert_eq!(config.calibration_limit, 3);
    }

    #[test]
    fn current_multi_sequencer_configuration_is_unchanged() {
        let mut value = polling_fields();
        value["sequencers"] = json!([
            {"sequencer_addr": "https://first.example.test"},
            {
                "sequencer_addr": "https://second.example.test",
                "basic_auth": {"username": "operator", "password": "test-only"}
            }
        ]);
        value["calibration_limit"] = json!(19);

        let config: WalletConfig =
            serde_json::from_value(value).expect("current configuration should load");

        assert_eq!(config.sequencers.len(), 2);
        assert_eq!(
            config.sequencers[0].sequencer_addr.as_str(),
            "https://first.example.test/"
        );
        assert_eq!(
            config.sequencers[1].sequencer_addr.as_str(),
            "https://second.example.test/"
        );
        let auth = config.sequencers[1]
            .basic_auth
            .as_ref()
            .expect("basic authentication should be preserved");
        assert_eq!(auth.username, "operator");
        assert_eq!(auth.password.as_deref(), Some("test-only"));
        assert_eq!(config.calibration_limit, 19);
    }

    #[test]
    fn transitional_multi_sequencer_configuration_loads_and_serializes_as_current_schema() {
        let mut value = polling_fields();
        value["sequencers_conn_data"] = json!([
            {"sequencer_addr": "https://first.example.test"},
            {"sequencer_addr": "https://second.example.test"}
        ]);
        value["callibration_limit"] = json!(23);

        let config: WalletConfig =
            serde_json::from_value(value).expect("transitional configuration should load");

        assert_eq!(config.sequencers.len(), 2);
        assert_eq!(
            config.sequencers[0].sequencer_addr.as_str(),
            "https://first.example.test/"
        );
        assert_eq!(
            config.sequencers[1].sequencer_addr.as_str(),
            "https://second.example.test/"
        );
        assert_eq!(config.calibration_limit, 23);

        let serialized =
            serde_json::to_value(config).expect("migrated configuration should serialize");
        assert!(serialized.get("sequencers_conn_data").is_none());
        assert!(serialized.get("sequencer_addr").is_none());
        assert!(serialized.get("callibration_limit").is_none());
        assert!(serialized.get("sequencers").is_some());
        assert_eq!(serialized["calibration_limit"], 23);
    }

    #[test]
    fn legacy_configuration_serializes_as_current_schema() {
        let mut value = polling_fields();
        value["sequencer_addr"] = json!("https://legacy.example.test");
        let config: WalletConfig =
            serde_json::from_value(value).expect("legacy configuration should load");

        let serialized =
            serde_json::to_value(config).expect("migrated configuration should serialize");

        assert!(serialized.get("sequencer_addr").is_none());
        assert_eq!(
            serialized["sequencers"][0]["sequencer_addr"],
            "https://legacy.example.test/"
        );
    }

    #[test]
    fn preserves_legacy_basic_authentication() {
        let mut value = polling_fields();
        value["sequencer_addr"] = json!("https://legacy.example.test");
        value["basic_auth"] = json!({"username": "operator", "password": "test-only"});

        let config: WalletConfig =
            serde_json::from_value(value).expect("authenticated legacy configuration should load");
        let auth = config.sequencers[0]
            .basic_auth
            .as_ref()
            .expect("legacy basic authentication should be preserved");

        assert_eq!(auth.username, "operator");
        assert_eq!(auth.password.as_deref(), Some("test-only"));
    }

    #[test]
    fn rejects_conflicting_current_and_legacy_sequencer_fields() {
        let mut value = polling_fields();
        value["sequencers"] = json!([{"sequencer_addr": "https://current.example.test"}]);
        value["sequencer_addr"] = json!("https://legacy.example.test");

        let error = serde_json::from_value::<WalletConfig>(value)
            .expect_err("conflicting configuration should be rejected");

        assert!(
            error
                .to_string()
                .contains("cannot contain both `sequencers` and legacy `sequencer_addr`")
        );
    }

    #[test]
    fn rejects_conflicting_current_and_transitional_sequencer_fields() {
        let mut value = polling_fields();
        value["sequencers"] = json!([{"sequencer_addr": "https://current.example.test"}]);
        value["sequencers_conn_data"] =
            json!([{"sequencer_addr": "https://transitional.example.test"}]);

        let error = serde_json::from_value::<WalletConfig>(value)
            .expect_err("conflicting configuration should be rejected");

        assert!(error.to_string().contains("duplicate field `sequencers`"));
    }

    #[test]
    fn rejects_conflicting_transitional_and_legacy_sequencer_fields() {
        let mut value = polling_fields();
        value["sequencers_conn_data"] =
            json!([{"sequencer_addr": "https://transitional.example.test"}]);
        value["sequencer_addr"] = json!("https://legacy.example.test");

        let error = serde_json::from_value::<WalletConfig>(value)
            .expect_err("conflicting configuration should be rejected");

        assert!(
            error
                .to_string()
                .contains("cannot contain both `sequencers` and legacy `sequencer_addr`")
        );
    }

    #[test]
    fn reports_missing_sequencer_configuration() {
        let error = serde_json::from_value::<WalletConfig>(polling_fields())
            .expect_err("missing sequencer configuration should be rejected");

        assert!(
            error
                .to_string()
                .contains("must contain `sequencers` or legacy `sequencer_addr`")
        );
    }

    #[test]
    fn reports_invalid_current_sequencer_field_type() {
        let mut value = polling_fields();
        value["sequencers"] = json!("https://not-an-array.example.test");

        let error = serde_json::from_value::<WalletConfig>(value)
            .expect_err("malformed current configuration should be rejected");

        assert!(error.to_string().contains("invalid type"));
        assert!(error.to_string().contains("a sequence"));
    }

    #[test]
    fn rejects_conflicting_current_and_transitional_calibration_fields() {
        let mut value = polling_fields();
        value["sequencers"] = json!([{"sequencer_addr": "https://current.example.test"}]);
        value["calibration_limit"] = json!(19);
        value["callibration_limit"] = json!(23);

        let error = serde_json::from_value::<WalletConfig>(value)
            .expect_err("duplicate calibration fields should be rejected");

        assert!(
            error
                .to_string()
                .contains("duplicate field `calibration_limit`")
        );
    }
}
