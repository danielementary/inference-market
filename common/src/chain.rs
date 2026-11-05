use anyhow::{Context, Result};
use inference_escrow_client::{
    accounts::{Config, Request, Worker, REQUEST_DISCRIMINATOR, WORKER_DISCRIMINATOR},
    ID,
};
use solana_address::Address;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::{
    config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    filter::{Memcmp, RpcFilterType},
    response::UiAccountEncoding,
};
use solana_signer::Signer;
use solana_transaction::{Instruction, Transaction};

use crate::ix;

pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";

/// RPC access to the program at `confirmed` commitment.
pub struct Chain {
    pub rpc: RpcClient,
}

impl Chain {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            rpc: RpcClient::new_with_commitment(url.into(), CommitmentConfig::confirmed()),
        }
    }

    /// Uses `$RPC_URL`, or the local validator.
    pub fn from_env() -> Self {
        Self::new(std::env::var("RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.into()))
    }

    /// Signs with `payer` (fee payer) and `signers`, sends, and waits for confirmation.
    pub async fn send(
        &self,
        ixs: &[Instruction],
        payer: &Keypair,
        signers: &[&Keypair],
    ) -> Result<String> {
        let mut all: Vec<&Keypair> = vec![payer];
        all.extend(signers.iter().filter(|k| k.pubkey() != payer.pubkey()));
        let blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &all, blockhash);
        let signature = self.rpc.send_and_confirm_transaction(&tx).await?;
        Ok(signature.to_string())
    }

    pub async fn balance(&self, address: &Address) -> Result<u64> {
        Ok(self.rpc.get_balance(address).await?)
    }

    async fn data(&self, address: &Address) -> Result<Option<Vec<u8>>> {
        let account = self
            .rpc
            .get_account_with_commitment(address, self.rpc.commitment())
            .await?
            .value;
        Ok(account.map(|a| a.data))
    }

    pub async fn config(&self) -> Result<Option<Config>> {
        self.data(&ix::config_pda())
            .await?
            .map(|d| Config::from_bytes(&d).context("decoding Config"))
            .transpose()
    }

    pub async fn worker(&self, owner: &Address) -> Result<Option<Worker>> {
        self.worker_at(&ix::worker_pda(owner)).await
    }

    /// Worker account by its PDA (e.g. `Request::worker`).
    pub async fn worker_at(&self, address: &Address) -> Result<Option<Worker>> {
        self.data(address)
            .await?
            .map(|d| Worker::from_bytes(&d).context("decoding Worker"))
            .transpose()
    }

    pub async fn request(&self, client: &Address, nonce: u64) -> Result<Option<Request>> {
        self.data(&ix::request_pda(client, nonce))
            .await?
            .map(|d| Request::from_bytes(&d).context("decoding Request"))
            .transpose()
    }

    /// All registered worker offers, keyed by their `Worker` PDA.
    pub async fn workers(&self) -> Result<Vec<(Address, Worker)>> {
        self.program_accounts(&WORKER_DISCRIMINATOR, Worker::from_bytes)
            .await
    }

    /// All open or assigned requests (settled and cancelled ones are closed), keyed by PDA.
    pub async fn requests(&self) -> Result<Vec<(Address, Request)>> {
        self.program_accounts(&REQUEST_DISCRIMINATOR, Request::from_bytes)
            .await
    }

    /// Program accounts whose data starts with `discriminator`, decoded with `decode`.
    async fn program_accounts<T>(
        &self,
        discriminator: &[u8; 8],
        decode: fn(&[u8]) -> std::io::Result<T>,
    ) -> Result<Vec<(Address, T)>> {
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                0,
                discriminator.to_vec(),
            ))]),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..Default::default()
            },
            ..Default::default()
        };
        let accounts = self
            .rpc
            .get_program_ui_accounts_with_config(&ID, config)
            .await?;
        accounts
            .into_iter()
            .map(|(address, account)| {
                let data = account
                    .data
                    .decode()
                    .context("decoding base64 account data")?;
                Ok((address, decode(&data).context("decoding account")?))
            })
            .collect()
    }
}
