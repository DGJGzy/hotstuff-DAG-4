use crate::SeqNumber;
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
use std::net::SocketAddr;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use store::Store;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::time::{sleep, Duration, Instant};

#[cfg(test)]
#[path = "tests/synchronizer_tests.rs"]
pub mod synchronizer_tests;

const TIMER_ACCURACY: u64 = 5_000;

static START_TIME: OnceLock<Instant> = OnceLock::new();

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

    pub fn get_idx(key: &PublicKey, committee: &Committee) -> SeqNumber {
        let mut keys: Vec<_> = committee.authorities.keys().cloned().collect();
        keys.sort();
        keys.iter().position(|k| k == key).unwrap() as SeqNumber
    }
    
    pub async fn transmit(
        message: ConsensusMessage,
        from: &PublicKey,
        to: Option<&PublicKey>,
        network_filter: &Sender<FilterInput>,
        committee: &Committee,
        tag: u8,
    ) -> ConsensusResult<()> {
        START_TIME.set(Instant::now()).unwrap_or(());

        let mut addresses = if let Some(to) = to {
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

        let mut deleted_addresses = Vec::new();
        if let Some(start_time) = START_TIME.get() {
            let elapsed = start_time.elapsed().as_secs();
            let cycle_position = elapsed % 90;
            if cycle_position >= 60 {
                let from_id = Self::get_idx(from, committee);
                debug!("DDoS attack active. from_id: {}", from_id);
                // extract all nodes address (id to address hashmap)
                let all_addresses: HashMap<usize, SocketAddr> = committee.authorities
                    .iter()
                    .map(|(_, authority)| {
                        (authority.id, authority.address) 
                    })
                    .collect();
                
                if from_id >= 0 && from_id <= 3 {
                    // delete addresses of nodes 4, 5, 6
                    for id in 4..=6 {
                        if let Some(addr) = all_addresses.get(&id) {
                            if let Some(pos) = addresses.iter().position(|x| x == addr) {
                                debug!("DDoS attack: removing address of node {}", id);
                                // remove the address from addresses
                                let _ = addresses.remove(pos);
                                deleted_addresses.push(*addr);
                            }
                        }
                    }
                }

                if from_id >= 4 && from_id <= 6 {
                    // delete addresses of nodes 0, 1, 2, 3
                    for id in 0..=3 {
                        if let Some(addr) = all_addresses.get(&id) {
                            if let Some(pos) = addresses.iter().position(|x| x == addr) {
                                debug!("DDoS attack: removing address of node {}", id);
                                // remove the address from addresses
                                let _ = addresses.remove(pos);
                                deleted_addresses.push(*addr);
                            }
                        }
                    }
                }
            }
        }

        if let Err(e) = network_filter.send((message.clone(), addresses, false)).await {
            panic!("Failed to send block through network channel: {}", e);
        }
        if let Err(e) = network_filter.send((message, deleted_addresses, true)).await {
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
                debug!("not ok");
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
