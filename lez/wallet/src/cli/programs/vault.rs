use anyhow::Result;
use clap::Subcommand;
use common::transaction::LeeTransaction;
use lee::AccountId;

use crate::{
    AccDecodeData::Decode,
    WalletCore,
    account::AccountIdWithPrivacy,
    cli::{CliAccountMention, SubcommandReturnValue, WalletSubcommand},
    program_facades::vault::Vault,
};

/// Represents generic CLI subcommand for a wallet working with vault program.
#[derive(Subcommand, Debug, Clone)]
pub enum VaultSubcommand {
    /// Transfer native tokens from sender to recipient's vault account.
    Transfer {
        /// Either 32 byte base58 account id string with privacy prefix or a label.
        #[arg(long)]
        from: CliAccountMention,
        /// Either 32 byte base58 account id string with privacy prefix or a label.
        #[arg(long)]
        to: CliAccountMention,
        /// Amount of native tokens to transfer.
        #[arg(long)]
        amount: u128,
    },
    /// Claim native tokens from account's vault account.
    Claim {
        /// Either 32 byte base58 account id string with privacy prefix or a label.
        ///
        /// The owner's vault account id is computed from this account id.
        #[arg(long)]
        account_id: CliAccountMention,
        /// Amount of native tokens to claim.
        #[arg(long)]
        amount: u128,
    },
}

impl WalletSubcommand for VaultSubcommand {
    async fn handle_subcommand(
        self,
        wallet_core: &mut WalletCore,
    ) -> Result<SubcommandReturnValue> {
        match self {
            Self::Transfer { from, to, amount } => {
                let from = from.resolve(wallet_core.storage())?;
                let recipient = to.resolve(wallet_core.storage())?;
                let recipient_id = account_id_without_privacy(recipient);

                match from {
                    AccountIdWithPrivacy::Public(sender_id) => {
                        let tx_hash = Vault(wallet_core)
                            .send_transfer(sender_id, recipient_id, amount)
                            .await?;

                        println!("Transaction hash is {tx_hash}");

                        let transfer_tx = wallet_core.poll_native_token_transfer(tx_hash).await?;

                        println!("Transaction data is {transfer_tx:?}");

                        Ok(SubcommandReturnValue::Empty)
                    }
                    AccountIdWithPrivacy::Private(sender_id) => {
                        let (tx_hash, secret_sender) = Vault(wallet_core)
                            .send_transfer_private_sender(sender_id, recipient_id, amount)
                            .await?;

                        println!("Transaction hash is {tx_hash}");

                        let transfer_tx = wallet_core.poll_native_token_transfer(tx_hash).await?;

                        println!("Transaction data is {transfer_tx:?}");

                        if let LeeTransaction::PrivacyPreserving(tx) = transfer_tx {
                            wallet_core.decode_insert_privacy_preserving_transaction_results(
                                &tx,
                                &[Decode(secret_sender, sender_id)],
                            )?;
                        }

                        wallet_core.store_persistent_data()?;

                        Ok(SubcommandReturnValue::PrivacyPreservingTransfer { tx_hash })
                    }
                }
            }
            Self::Claim { account_id, amount } => {
                let account_id = account_id.resolve(wallet_core.storage())?;

                match account_id {
                    AccountIdWithPrivacy::Public(owner_id) => {
                        let tx_hash = Vault(wallet_core).send_claim(owner_id, amount).await?;

                        println!("Transaction hash is {tx_hash}");

                        let transfer_tx = wallet_core.poll_native_token_transfer(tx_hash).await?;

                        println!("Transaction data is {transfer_tx:?}");

                        Ok(SubcommandReturnValue::Empty)
                    }
                    AccountIdWithPrivacy::Private(owner_id) => {
                        let (tx_hash, secret_owner) = Vault(wallet_core)
                            .send_claim_private_owner(owner_id, amount)
                            .await?;

                        println!("Transaction hash is {tx_hash}");

                        let transfer_tx = wallet_core.poll_native_token_transfer(tx_hash).await?;

                        println!("Transaction data is {transfer_tx:?}");

                        if let LeeTransaction::PrivacyPreserving(tx) = transfer_tx {
                            wallet_core.decode_insert_privacy_preserving_transaction_results(
                                &tx,
                                &[Decode(secret_owner, owner_id)],
                            )?;
                        }

                        wallet_core.store_persistent_data()?;

                        Ok(SubcommandReturnValue::PrivacyPreservingTransfer { tx_hash })
                    }
                }
            }
        }
    }
}

const fn account_id_without_privacy(account_id: AccountIdWithPrivacy) -> AccountId {
    match account_id {
        AccountIdWithPrivacy::Public(account_id) | AccountIdWithPrivacy::Private(account_id) => {
            account_id
        }
    }
}
