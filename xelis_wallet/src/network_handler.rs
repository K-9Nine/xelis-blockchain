use std::{fmt::Display, sync::Arc, time::Duration};

use async_recursion::async_recursion;
use thiserror::Error;
use anyhow::Error;
use log::{debug, error};
use tokio::{task::JoinHandle, sync::Mutex, time::interval};
use xelis_common::{crypto::{hash::Hash, address::Address}, block::Block, transaction::TransactionType};

use crate::{api::DaemonAPI, wallet::Wallet, entry::{EntryData, Transfer, TransactionEntry}};

// NetworkHandler must be behind a Arc to be accessed from Wallet (to stop it) or from tokio task
pub type SharedNetworkHandler = Arc<NetworkHandler>;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("network handler is already running")]
    AlreadyRunning
}

pub struct NetworkHandler {
    // tokio task
    task: Mutex<Option<JoinHandle<Result<(), Error>>>>,
    // wallet where we can save every data from chain
    wallet: Arc<Wallet>,
    // api to communicate with daemon
    api: DaemonAPI,
}

impl NetworkHandler {
    pub async fn new<S: Display>(wallet: Arc<Wallet>, daemon_address: S) -> Result<SharedNetworkHandler, Error> {
        let api = DaemonAPI::new(format!("{}/json_rpc", daemon_address));
        // check that we can correctly get version from daemon
        let version = api.get_version().await?;
        debug!("Connected to daemon running version {}", version);

        Ok(Arc::new(Self {
            task: Mutex::new(None),
            wallet,
            api
        }))
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), NetworkError> {
        if self.is_running().await {
            return Err(NetworkError::AlreadyRunning)
        }

        let zelf = Arc::clone(&self);
        *self.task.lock().await = Some(tokio::spawn(zelf.start_syncing()));

        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(handle) = self.task.lock().await.take() {
            if handle.is_finished() {
                if let Err(e) = handle.await {
                    debug!("Network handler was finished with error: {}", e);
                }
            } else {
                handle.abort();
            }
        }
    }

    pub fn get_api(&self) -> &DaemonAPI {
        &self.api
    }

    // check if the network handler is running (that we have a task and its not finished)
    pub async fn is_running(&self) -> bool {
        let task = self.task.lock().await;
        if let Some(handle) = task.as_ref() {
            !handle.is_finished()
        } else {
            false
        }
    }

    #[async_recursion]
    async fn get_balance_and_transactions(&self, address: &Address<'_>, asset: &Hash, min_topoheight: u64, topoheight: Option<u64>) -> Result<(), Error> {
        let (topoheight, balance) = match topoheight {
            Some(topoheight) => (topoheight, self.api.get_balance_at_topoheight(address, asset, topoheight).await?),
            None => { // try to get last balance
                let res = match self.api.get_last_balance(&address, asset).await {
                    Ok(res) => res,
                    Err(e) => { // balance doesn't exist on chain for this asset
                        debug!("Error while getting last balance: {}", e);
                        return Ok(())
                    }
                };
                let balance = res.balance;

                // lets write the final balance
                let storage = self.wallet.get_storage().write().await;
                storage.set_balance_for(asset, balance.get_balance())?;

                (res.topoheight, balance)
            }
        };

        // don't sync already synced blocks
        if min_topoheight > topoheight {
            return Ok(())
        }

        let response = self.api.get_block_at_topoheight(topoheight).await?;
        let block: Block = response.data.data.into_owned();
        
        // create Coinbase entry
        if *block.get_miner() == *address.get_public_key() {
            let coinbase = EntryData::Coinbase(response.reward);
            let entry = TransactionEntry::new(response.data.hash.into_owned(), topoheight, None, None, coinbase);
            let storage = self.wallet.get_storage().write().await;
            storage.save_transaction(entry.get_hash(), &entry)?;
        }

        for tx_hash in block.get_transactions() {
            let tx = self.api.get_transaction(tx_hash).await?;
            let is_owner = *tx.get_owner() == *address.get_public_key();

            let fee = if is_owner { Some(tx.get_fee()) } else { None };
            let nonce = if is_owner { Some(tx.get_nonce()) } else { None };

            let (owner, data) = tx.consume();
            let entry: Option<EntryData> = match data {
                TransactionType::Burn(asset, amount) => {
                    if is_owner {
                        Some(EntryData::Burn { asset, amount })
                    } else {
                        None
                    }
                },
                TransactionType::Transfer(txs) => {
                    let mut transfers: Vec<Transfer> = Vec::new();
                    for tx in txs {
                        if is_owner || tx.to == *address.get_public_key() {
                            let transfer = Transfer::new(tx.to, tx.asset, tx.amount, tx.extra_data);
                            transfers.push(transfer);
                        }
                    }

                    if is_owner {
                        Some(EntryData::Outgoing(transfers))
                    } else {
                        Some(EntryData::Incoming(owner, transfers))
                    }
                },
                _ => {
                    error!("Transaction type not supported");
                    None
                }
            };

            if let Some(entry) = entry {
                let entry = TransactionEntry::new(tx_hash.clone(), topoheight, fee, nonce, entry);
                let storage = self.wallet.get_storage().write().await;
                storage.save_transaction(entry.get_hash(), &entry)?;
            }
        }

        if let Some(previous_topo) = balance.get_previous_topoheight() {
            self.get_balance_and_transactions(address, asset, min_topoheight, Some(previous_topo)).await?;
        }

        Ok(())
    }

    // start syncing the wallet with data from daemon API
    // we get all assets registered on chain and check their balance
    // we also check if there is a balance change at a previous topoheight
    // if there is, we get the balance at this topoheight and check if there is a previous topoheight
    // we do this until we reach the first topoheight
    async fn start_syncing(self: Arc<Self>) -> Result<(), Error> {
        let mut current_topoheight = {
            let storage = self.wallet.get_storage().read().await;
            storage.get_daemon_topoheight().unwrap_or(0)
        };
        let mut interval = interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            // get infos from chain
            // TODO compare them with already stored to not resync fully each time
            let info = self.api.get_info().await?;
            if info.topoheight == current_topoheight {
                continue;
            }
            debug!("New height detected for chain: {}", info.topoheight);
            
            
            if let Err(e) = self.sync_new_blocks(current_topoheight).await {
                error!("Error while syncing new blocks: {}", e);
            }

            // save current topoheight in daemon
            {
                let storage = self.wallet.get_storage().write().await;
                storage.set_daemon_topoheight(current_topoheight)?;
                storage.set_top_block_hash(&info.top_hash)?;
            }
            current_topoheight = info.topoheight;
        }
    }

    async fn sync_new_blocks(&self, current_topoheight: u64) -> Result<(), Error> {
        // TODO detect new changes in assets
        let mut assets = {
            let storage = self.wallet.get_storage().read().await;
            storage.get_assets()?
        };

        if assets.is_empty() {
            debug!("No assets registered on disk, fetching from chain...");
            assets = self.api.get_assets().await?;
            debug!("Found {} assets", assets.len());
            let storage = self.wallet.get_storage().write().await;
            for asset in &assets {
                storage.add_asset(asset)?;
            }
        }

        let address = self.wallet.get_address();
        for asset in assets {
            if let Err(e) = self.get_balance_and_transactions(&address, &asset, current_topoheight, None).await {
                error!("Error while syncing balance for asset {}: {}", asset, e);
            }
        }
        Ok(())
    }
}