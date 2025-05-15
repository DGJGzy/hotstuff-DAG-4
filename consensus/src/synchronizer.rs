use crate::config::Committee;
use crate::core::{ConsensusMessage, HOTSTUFF};
use crate::error::ConsensusResult;
use crate::filter::FilterInput;
use crate::messages::{Block, QC};
use crypto::Hash as _;
use crypto::{Digest, PublicKey};
use futures::stream::futures_unordered::FuturesUnordered;
use futures::stream::StreamExt as _;
use log::{debug, error};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};
use store::Store;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::time::{sleep, Duration, Instant};

#[cfg(test)]
#[path = "tests/synchronizer_tests.rs"]
pub mod synchronizer_tests;

const TIMER_ACCURACY: u64 = 5_000;

pub struct Synchronizer {
    store: Store,
    inner_channel: Sender<(Vec<(PublicKey, Digest)>, Option<Block>)>,
}

impl Synchronizer {
    pub async fn new(
        name: PublicKey,
        committee: Committee,
        store: Store,
        network_filter: Sender<FilterInput>,
        core_channel: Sender<ConsensusMessage>,
        sync_retry_delay: u64,
    ) -> Self {
        let (tx_inner, mut rx_inner)
            : (_, Receiver<(Vec<(PublicKey, Digest)>, Option<Block>)>) = channel(10000); 
        let mut store_copy = store.clone();

        tokio::spawn(async move {
            let mut waiting = FuturesUnordered::new();
            let mut pending = HashMap::new();
            let mut demands: HashMap<Digest, HashSet<Digest>> = HashMap::new();

            let timer = sleep(Duration::from_millis(TIMER_ACCURACY));
            tokio::pin!(timer);
            loop {
                tokio::select! {
                    Some((requests, block)) = rx_inner.recv() => {
                        for (author, digest) in requests {
                            // avoid repeated requests
                            if let Ok(Some(_)) = store_copy.read(digest.to_vec()).await {
                                continue;
                            }

                            if !pending.contains_key(&digest) {
                                let fut = Self::waiter(store_copy.clone(), digest.clone(), block.clone());
                                waiting.push(fut);

                                debug!("Requesting sync for block {}", digest);
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .expect("Failed to measure time")
                                    .as_millis();

                                pending.insert(digest.clone(), (now, block.clone()));
                                if block.is_some() {
                                    demands.entry(block.clone().unwrap().digest())
                                        .or_insert_with(HashSet::new)
                                        .insert(digest.clone());
                                }

                                let message = ConsensusMessage::SyncRequest(digest, name);
                                Self::transmit(
                                    message, 
                                    &name, 
                                    Some(&author), 
                                    &network_filter, 
                                    &committee,
                                    HOTSTUFF,
                                ).await.unwrap();
                            }
                        }
                    },
                    Some(result) = waiting.next() => match result {
                        Ok((request_block, block)) => {
                            debug!("Received sync response for block {}", request_block.digest());
                            let _ = pending.remove(&request_block.digest());
                            if block.is_some() {
                                let digest = block.clone().unwrap().digest();
                                let demands_set = demands.get_mut(&digest).unwrap();
                                demands_set.remove(&request_block.digest());
                                if demands_set.is_empty() {
                                    demands.remove(&digest);
                                    let message = ConsensusMessage::LoopBack(block.unwrap());
                                    if let Err(e) = core_channel.send(message).await {
                                        panic!("Failed to send message through core channel: {}", e);
                                    }
                                }
                            }
                        },
                        Err(e) => error!("{}", e)
                    },
                    () = &mut timer => {
                        // This implements the 'perfect point to point link' abstraction.
                        for (digest, (timestamp, _)) in &pending {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .expect("Failed to measure time")
                                .as_millis();
                            if timestamp + (sync_retry_delay as u128) < now {
                                debug!("Requesting sync for block {} (retry)", digest);
                                let message = ConsensusMessage::SyncRequest(digest.clone(), name);
                                Self::transmit(
                                    message, 
                                    &name, 
                                    None, 
                                    &network_filter, 
                                    &committee,
                                    HOTSTUFF,
                                ).await.unwrap();
                            }
                        }
                        timer.as_mut().reset(Instant::now() + Duration::from_millis(TIMER_ACCURACY));
                    },
                    else => break,
                }
            }
        });
        Self {
            store,
            inner_channel: tx_inner,
        }
    }

    async fn waiter(mut store: Store, wait_on: Digest, block: Option<Block>) -> ConsensusResult<(Block, Option<Block>)> {
        let bytes = store.notify_read(wait_on.to_vec()).await?;
        Ok((bincode::deserialize(&bytes)?, block))
    }

    pub async fn transmit(
        message: ConsensusMessage,
        from: &PublicKey,
        to: Option<&PublicKey>,
        network_filter: &Sender<FilterInput>,
        committee: &Committee,
        tag: u8,
    ) -> ConsensusResult<()> {
        let addresses = if let Some(to) = to {
            debug!("Sending {:?} to {}", message, to);
            if tag == HOTSTUFF {
                vec![committee.address(to)?]
            } else {
                vec![committee.smvba_address(to)?]
            }
        } else {
            debug!("Broadcasting {:?}", message);
            if tag == HOTSTUFF {
                committee.broadcast_addresses(from)
            } else {
                committee.smvba_broadcast_addresses(from)
            }
        };
        if let Err(e) = network_filter.send((message, addresses)).await {
            panic!("Failed to send block through network channel: {}", e);
        }
        Ok(())
    }

    pub async fn get_parent_block(&mut self, block: &Block) -> ConsensusResult<Option<Block>> {
        Ok(self.get_block(block.author, block.parent(), Some(block.clone())).await?)
    }

    pub async fn get_ancestors(
        &mut self,
        block: &Block,
    ) -> ConsensusResult<Option<(Block, Block)>> {
        let b1 = match self.get_parent_block(block).await? {
            Some(b) => b,
            None => return Ok(None),
        };
        let b0 = match self.get_parent_block(&b1).await? {
            Some(b) => b,
            None => return Ok(None),
        };
        Ok(Some((b0, b1)))
    }

    // If the block is not in the store, it will be requested from the network.
    pub async fn get_block(&mut self, author: PublicKey, digest: &Digest, block: Option<Block>) -> ConsensusResult<Option<Block>> {
        if digest.clone() == QC::genesis().hash {
            return Ok(Some(Block::genesis()));
        }

        match self.store.read(digest.to_vec()).await? {
            Some(bytes) => Ok(Some(bincode::deserialize(&bytes)?)),
            None => {
                if let Err(e) = self
                    .inner_channel      
                    .send((vec![(author, digest.clone())], block))
                    .await 
                {
                    panic!("Failed to send request to synchronizer: {}", e);
                }
                Ok(None)
            }
        }
    }
}
