use crate::aggregator::Aggregator;
use crate::config::{Committee, Parameters};
use crate::error::{ConsensusError, ConsensusResult};
use crate::filter::FilterInput;
use crate::leader::LeaderElector;
use crate::mempool::MempoolDriver;
use crate::messages::{ABAProof, ABAVal, Block, RandomnessShare, Timeout, Vote, QC, TC};
use crate::synchronizer::Synchronizer;
use async_recursion::async_recursion;
use crypto::Hash as _;
use crypto::{Digest, PublicKey, SignatureService};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::cmp::max;
use std::collections::{HashMap, HashSet, VecDeque};
use store::Store;
use threshold_crypto::PublicKeySet;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::{sleep, Duration};
#[cfg(test)]
#[path = "tests/core_tests.rs"]
pub mod core_tests;

#[cfg(test)]
#[path = "tests/smvba_tests.rs"]
pub mod smvba_tests;

pub type SeqNumber = u64; 

const NEXT_EPOCH_HEIGHT: u64 = 10;
const LAMBDA_VAL: u64 = 10; // We can change timeout condition by changing this value.
pub const INIT_PHASE: u8 = 0;
pub const VAL_PHASE: u8 = 1;
pub const AUX_PHASE: u8 = 2;
pub const COIN_PHASE: u8 = 3;

pub const HOTSTUFF: u8 = 0;
pub const TCVBA: u8 = 1;

#[derive(Serialize, Deserialize, Debug)]
pub enum ConsensusMessage {
    Propose(Block),
    Vote(Vote),
    LoopBack(Block),
    SyncRequest(Digest, PublicKey),
    SyncReply(Block),
    Timeout(Timeout),
    TC(TC),
    ABAVal(ABAVal),
    ABAProof(ABAProof),
    ABACoinShare(RandomnessShare),
}

#[derive(Clone)]
pub struct Chain {
    name: PublicKey,
    height: SeqNumber,
    last_voted_height: SeqNumber,
    last_committed_height: SeqNumber,
    aggregator: Aggregator,
    high_qc: QC,
    height_to_digest: HashMap<SeqNumber, Digest>,
}

impl Chain {
    pub fn new(name: PublicKey, committee: Committee) -> Self {
        Self {
            name,
            height: 1,
            last_voted_height: 0,
            last_committed_height: 0,
            aggregator: Aggregator::new(committee),
            high_qc: QC::genesis(),
            height_to_digest: HashMap::new(),
        }
    }
}

pub struct Core {
    name: PublicKey,
    committee: Committee,
    parameters: Parameters,
    store: Store,
    signature_service: SignatureService,
    pk_set: PublicKeySet,
    leader_elector: LeaderElector,
    mempool_driver: MempoolDriver,
    synchronizer: Synchronizer,
    core_channel: Receiver<ConsensusMessage>,
    smvba_channel: Receiver<ConsensusMessage>,
    network_filter: Sender<FilterInput>,
    network_filter_smvba: Sender<FilterInput>,
    commit_channel: Sender<Block>,
    epoch: SeqNumber,
    total_epoch: SeqNumber,
    tc_processed: bool,
    val_value_broadcasted: bool, // val_1
    aux_value_broadcasted: bool, // val_2
    coin_share_broadcasted: bool,
    aux_proof_broadcasted: bool,
    is_view_change: bool,
    pubkey_to_chain: HashMap<PublicKey, Chain>,
    tc_cache: HashMap<SeqNumber, TC>, // epoch
    aggregator: Aggregator,
    bin_values: HashSet<u64>,
    aba_round: SeqNumber,
    phase: u8,
    aba_output_val: Option<u64>, 
    aba_input_val: HashMap<SeqNumber, u64>, // round -> value
    aba_val_phase_cache1: HashMap<(SeqNumber, SeqNumber), ABAProof>,
    aba_val_phase_cache2: HashMap<(SeqNumber, SeqNumber), ABAProof>,
    aba_aux_phase_cache: HashMap<(SeqNumber, SeqNumber), ABAProof>, // 2f+1 aux values
    aux_value_nums: HashMap<(SeqNumber, SeqNumber), u64>, // the number of values in set "values"
    aba_coin_cache: HashMap<(SeqNumber, SeqNumber), usize>,
    aba_proof_cache: HashMap<(SeqNumber, SeqNumber), ABAProof>,
}

impl Core {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: PublicKey,
        committee: Committee,
        parameters: Parameters,
        signature_service: SignatureService,
        pk_set: PublicKeySet,
        store: Store,
        leader_elector: LeaderElector,
        mempool_driver: MempoolDriver,
        synchronizer: Synchronizer,
        core_channel: Receiver<ConsensusMessage>,
        smvba_channel: Receiver<ConsensusMessage>,
        network_filter: Sender<FilterInput>,
        network_filter_smvba: Sender<FilterInput>,
        commit_channel: Sender<Block>,
    ) -> Self {
        let aggregator = Aggregator::new(committee.clone());
        let pubkey_to_chain = committee.authorities
            .keys()
            .map(|pk| {
            (*pk, Chain::new(*pk, committee.clone()))
        }).collect::<HashMap<_, _>>();
        Self {
            name,
            committee,
            parameters,
            store,
            signature_service,
            pk_set,
            leader_elector,
            mempool_driver,
            synchronizer,
            core_channel,
            smvba_channel,
            network_filter,
            network_filter_smvba,
            commit_channel,
            epoch: 0,
            total_epoch: 0,
            tc_processed: false,
            val_value_broadcasted: false, // val_1
            aux_value_broadcasted: false, // val_2
            coin_share_broadcasted: false,
            aux_proof_broadcasted: false,
            is_view_change: false,
            pubkey_to_chain: pubkey_to_chain,
            tc_cache: HashMap::new(), // epoch
            aggregator: aggregator,
            bin_values: HashSet::new(),
            aba_round: 1,
            phase: INIT_PHASE,
            aba_output_val: None, 
            aba_input_val: HashMap::new(), // round -> value
            aba_val_phase_cache1: HashMap::new(),
            aba_val_phase_cache2: HashMap::new(),
            aba_aux_phase_cache: HashMap::new(), // 2f+1 aux values
            aux_value_nums: HashMap::new(), // the number of values in set "values"
            aba_coin_cache: HashMap::new(),
            aba_proof_cache: HashMap::new(),
        }
    }

    async fn store_block(&mut self, block: &Block) {
        debug!("Storing block {:?}", block);
        let key = block.digest().to_vec();
        let value = bincode::serialize(block).expect("Failed to serialize block");
        self.store.write(key, value).await;
    }

    fn increase_last_voted_round(&mut self, target: SeqNumber, chain: &mut Chain) {
        chain.last_voted_height = max(chain.last_voted_height, target);
    }

    async fn make_vote(&mut self, block: &Block, chain: &mut Chain) -> Option<Vote> {
        // We can not vote for a block whose epoch smaller.
        if self.epoch > block.epoch {
            info!("FailVote 1, self.epoch: {}, block.epoch: {}", self.epoch, block.epoch);
            return None;
        }
        // If we are changing view, we can not vote for current leader chain blocks.
        if self.is_view_change && block.author == self.leader_elector.get_leader(self.epoch) {
            info!("FailVote 2, view-change: {}, block.author: {}, leader: {}", 
                self.is_view_change, block.author, self.leader_elector.get_leader(self.epoch));
            return None;
        }
        // Check if we can vote for this block.
        let safety_rule_1 = block.height > chain.last_voted_height;
        // Consider view-change.
        let safety_rule_2 = block.qc.height + 1 == block.height || block.qc == QC::genesis();

        if !(safety_rule_1 && safety_rule_2) {
            if !safety_rule_1 {
                info!("FailVote 3, block.height: {}, chain.last_voted_height: {}", block.height, chain.last_voted_height);
            } else {
                info!("FailVote 4, block.qc: {:?}, block.height: {}", block.qc, block.height);
            } 
            return None;
        }

        // Ensure we won't vote for contradicting blocks.
        self.increase_last_voted_round(block.height, chain);
        // TODO [issue #15]: Write to storage preferred_round and last_voted_round.
        Some(Vote::new(block, self.name, self.signature_service.clone()).await)
    }

    #[async_recursion]
    async fn commit_prev_block(&mut self, block: &Block, chain: &mut Chain, to_commit: &mut VecDeque<Block>) -> ConsensusResult<bool> {
        if chain.last_committed_height >= block.height {
            return Ok(true);
        }
 
        while chain.last_committed_height < block.height {
            if let None = chain.height_to_digest.get(&(chain.last_committed_height + 1)) {
                return Ok(false);
            }
            let current_digest = chain.height_to_digest.get(&(chain.last_committed_height + 1)).unwrap();
            if let None = self.synchronizer.get_block(self.name, current_digest, None).await? {
                return Ok(false);
            }
            let mut current_block = self.synchronizer.get_block(self.name, current_digest, None).await?.unwrap();
            if current_block.author == self.leader_elector.get_leader(self.epoch) {
                current_block.references.sort_by_key(|(pk, _)| *pk); // maybe no need.
                for (_, digest) in &current_block.references {
                    match self.synchronizer.get_block(self.name, digest, None).await? {
                        Some(block) => {
                            let mut target = self.pubkey_to_chain.get(&block.author).cloned().unwrap();
                            if !self.commit_prev_block(&block, &mut target, to_commit).await? {
                                return Ok(false);
                            }
                            self.pubkey_to_chain.insert(block.author, target);
                        },
                        None => return Ok(false),
                    }
                }
            }
            to_commit.push_front(current_block.clone());
            chain.last_committed_height += 1;
        }

        Ok(true)
    }

    #[async_recursion]
    async fn commit(&mut self, block: Block, chain: &mut Chain) -> ConsensusResult<()> {
        let mut to_commit = VecDeque::new();
        if !self.commit_prev_block(&block, chain, &mut to_commit).await? {
            debug!("Failed to commit block");
        }

        // Send all the newly committed blocks to the node's application layer.
        while let Some(block) = to_commit.pop_back() {
            if !block.payload.is_empty() {
                info!("Committed {}", block);

                // Cleanup the mempool.
                self.mempool_driver.cleanup(&block).await;

                #[cfg(feature = "benchmark")]
                for x in &block.payload {
                    // NOTE: This log entry is used to compute performance.
                    info!("Committed {} -> {:?}", block, x);
                }
            }
            debug!("Committed {:?}", block);
            if let Err(e) = self.commit_channel.send(block).await {
                warn!("Failed to send block through the commit channel: {}", e);
            }
        }
        Ok(())
    }

    fn update_high_qc(&mut self, qc: &QC, chain: &mut Chain) {
        chain.height_to_digest.insert(qc.height, qc.hash.clone());
        if qc.height > chain.high_qc.height {
            chain.high_qc = qc.clone();
        }
    }

    async fn local_timeout_round(&mut self, chain: &mut Chain) -> ConsensusResult<()> {
        warn!("Timeout reached for chain {}, height {}", chain.name, chain.height);

        // Increase the last voted round.
        self.increase_last_voted_round(chain.height, chain);

        // Make a timeout message.
        let timeout = Timeout::new(
            chain.high_qc.clone(),
            self.epoch,
            self.name,
            self.signature_service.clone(),
        )
        .await;
        debug!("Created {:?}", timeout);

        // Broadcast the timeout message.
        let message = ConsensusMessage::Timeout(timeout.clone());
        Synchronizer::transmit(
            message,
            &self.name,
            None,
            &self.network_filter_smvba,
            &self.committee,
            TCVBA,
        ).await?;

        // Process our message.
        self.handle_timeout(&timeout, chain).await
    }

    #[async_recursion]
    async fn handle_vote(&mut self, vote: &Vote, chain: &mut Chain) -> ConsensusResult<()> {
        debug!("Processing {:?}", vote);
        // Vote handle condition
        if vote.height < chain.height {
            return Ok(());
        }

        // Ensure the vote is well formed.
        vote.verify(&self.committee)?;

        // Add the new vote to our aggregator and see if we have a quorum.
        if let Some(qc) = chain.aggregator.add_vote(vote.clone())? {
            debug!("Assembled {:?}", qc);

            // Process the QC.
            self.process_qc(&qc, chain).await;

            // Make a new block.
            let block = self.generate_proposal(chain, false).await;
            self.broadcast_propose(block, chain).await?;
        }
        Ok(())
    }

    async fn handle_timeout(&mut self, timeout: &Timeout, chain: &mut Chain) -> ConsensusResult<()> {
        debug!("Processing {:?}", timeout);

        if timeout.epoch < self.epoch {
            return Ok(());
        }

        // Ensure the timeout is well formed.
        timeout.verify(&self.committee)?;

        // Process the QC embedded in the timeout.
        self.process_qc(&timeout.high_qc, chain).await;

        // Add the new vote to our aggregator and see if we have a quorum.
        if let Some(tc) = chain.aggregator.add_timeout(timeout.clone())? {
            debug!("Assembled {:?}", tc);

            // Save tc.
            self.tc_cache.insert(tc.epoch, tc.clone());
        }
        Ok(())
    }

    #[async_recursion]
    async fn advance_height(&mut self, height: SeqNumber, chain: &mut Chain) {
        if height < chain.height {
            return;
        }
        chain.height = height + 1;
        if chain.name == self.leader_elector.get_leader(self.epoch) {
            debug!("Leader Chain {} moved to height {}", chain.name, chain.height);
        } else {
            debug!("Chain {} moved to height {}", chain.name, chain.height);
        }

        // Cleanup the vote aggregator.
        chain.aggregator.cleanup(&self.epoch, &chain.height);
    }

    #[async_recursion]
    async fn generate_proposal(&mut self, chain: &Chain, tag: bool) -> Block {
        let references: Vec<(PublicKey, Digest)> = self
            .committee
            .authorities
            .keys()
            .filter(|name| *name != &chain.name)
            .filter_map(|name| {
                let chain = self.pubkey_to_chain.get(name)?;
                if chain.high_qc == QC::genesis() {
                    return None;
                }
                Some((*name, chain.high_qc.hash.clone()))
            })
            .collect();
        let mut qc = QC::genesis();
        let mut block_epoch = self.epoch;
        if !tag {
            qc = chain.high_qc.clone();
            block_epoch = qc.epoch;
        } 
        // Make block
        let payload = self
            .mempool_driver
            .get(self.parameters.max_payload_size)
            .await;
        let block = Block::new(
            qc,
            self.name,
            chain.height,
            block_epoch,
            payload,
            references,
            self.signature_service.clone(),
        ).await;
        // let qc = if tag {
        //     QC::genesis()
        // } else {
        //     chain.high_qc.clone()
        // };
        // // Make block
        // let payload = self
        //     .mempool_driver
        //     .get(self.parameters.max_payload_size)
        //     .await;
        // let block = Block::new(
        //     qc,
        //     self.name,
        //     chain.height,
        //     self.epoch,
        //     payload,
        //     references,
        //     self.signature_service.clone(),
        // ).await;

        if !block.payload.is_empty() {
            info!("Created {}", block);

            #[cfg(feature = "benchmark")]
            for x in &block.payload {
                // NOTE: This log entry is used to compute performance.
                info!("Created {} -> {:?}", block, x);
            }
        }
        debug!("Created {:?}", block);

        block
    }

    async fn broadcast_propose(&mut self, block: Block, chain: &mut Chain) -> ConsensusResult<()> {
        // Broadcast our new block.
        let message = ConsensusMessage::Propose(block.clone());
        Synchronizer::transmit(
            message,
            &self.name,
            None,
            &self.network_filter,
            &self.committee,
            HOTSTUFF,
        )
        .await?;

        self.process_block(&block, chain).await?;

        // Wait for the minimum block delay.
        sleep(Duration::from_millis(self.parameters.min_block_delay)).await;
        Ok(())
    }

    async fn process_qc(&mut self, qc: &QC, chain: &mut Chain) {
        self.advance_height(qc.height, chain).await;
        self.update_high_qc(qc, chain);
    }

    async fn process_block_vote(&mut self, block: &Block, chain: &mut Chain) -> ConsensusResult<()> {
        // Ensure the block's round is as expected.
        // This check is important: it prevents bad leaders from producing blocks
        // far in the future that may cause overflow on the round number.
        if block.height == chain.height || block.qc == QC::genesis() {
            // See if we can vote for this block.
            // Block author is the qc maker.
            if let Some(vote) = self.make_vote(block, chain).await {
                debug!("Created {:?}", vote);
                if vote.proposer == self.name {
                    self.handle_vote(&vote, chain).await?;
                } else {
                    let message = ConsensusMessage::Vote(vote.clone());
                    Synchronizer::transmit(
                        message,
                        &self.name,
                        Some(&vote.proposer),
                        &self.network_filter,
                        &self.committee,
                        HOTSTUFF,
                    ).await?;
                }
            }
        }
        Ok(())
    }

    async fn process_block_prepare(&mut self, block: &Block) -> ConsensusResult<Option<(Block, Block)>> {
        // Let's see if we have the last three ancestors of the block, that is:
        //      b0 <- |qc0; b1| <- |qc1; block|
        // If we don't, the synchronizer asks for them to other nodes. It will
        // then ensure we process both ancestors in the correct order, and
        // finally make us resume processing this block.
        let (b0, b1) = match self.synchronizer.get_ancestors(block).await? {
            Some(ancestors) => ancestors,
            None => {
                debug!("Processing of {} suspended: missing parent", block.digest());
                return Ok(None);
            }
        };

        Ok(Some((b0, b1)))
    }

    async fn process_block_commit(&mut self, b0: Block, b1: Block, chain: &mut Chain) -> ConsensusResult<()> {
        // Check if we can commit the head of the 2-chain.
        // Note that we commit blocks only if we have all its ancestors.
        let leader = self.leader_elector.get_leader(self.epoch);
        if leader == b0.author && b0.height + 1 == b1.height {
            self.commit(b0, chain).await?;
        }    
        Ok(())
    }

    #[async_recursion]
    async fn process_block(&mut self, block: &Block, chain: &mut Chain) -> ConsensusResult<()> {
        debug!("Processing {:?}", block);
        // Store block immediately.
        self.store_block(block).await;

        // self.cleanup_proposer(block).await;

        // If the block is not in the leader chain, no need to commit.
        // debug!("leader: {}, block.author: {}", self.leader_elector.get_leader(self.epoch), block.author);
        // We can not commit block whose epoch is not the current epoch.(or bigger)
        if block.author == self.leader_elector.get_leader(self.epoch) {
            if let Some((b0, b1)) = self.process_block_prepare(block).await? {
                if !self.is_view_change && block.epoch <= self.epoch {
                   self.process_block_commit(b0, b1, chain).await?; 
                }    
            }
        } else {
            self.process_block_prepare(block).await?;
        }
        self.process_block_vote(block, chain).await?;

        Ok(())
    }

    async fn handle_proposal(&mut self, block: &Block, chain: &mut Chain) -> ConsensusResult<()> {
        let digest = block.digest();

        // Ensure the block proposer leads the chain.
        ensure!(
            block.author == chain.name,
            ConsensusError::WrongAuthor {
                digest,
                author: block.author,
                height: block.height
            }
        );

        // Check the block is correctly formed.
        block.verify(&self.committee)?;

        // Process the QC. This may allow us to advance round.
        self.process_qc(&block.qc, chain).await;

        // Let's see if we have the block's data. If we don't, the mempool
        // will get it and then make us resume processing this block.
        if !self.mempool_driver.verify(block.clone()).await? {
            info!("Processing of {} suspended: missing payload", digest);
            return Ok(());
        }

        // All check pass, we can process this block.
        self.process_block(block, chain).await
    }

    async fn handle_tc(&mut self, tc: TC) -> ConsensusResult<()> {
        if tc.epoch < self.epoch {
            return Ok(());
        }
        tc.verify(&self.committee)?;

        // Save tc.
        self.tc_cache.insert(tc.epoch, tc.clone());

        Ok(())
    }

    async fn process_tc(&mut self, tc: TC) -> ConsensusResult<()> {
        // Broadcast the TC.
        let message = ConsensusMessage::TC(tc.clone()); 
        Synchronizer::transmit(
            message, 
            &self.name, 
            None, 
            &self.network_filter_smvba, 
            &self.committee,
            TCVBA,
        ).await?;
        // Make a new block if we are in the leader chain.
        // Do it after aba output.
        // if chain.name == self.name {
        //     self.generate_proposal(Some(tc.clone()), chain).await;
        // }
        // High qc is aba input.
        let input_val = *tc.high_qc_rounds().iter().max().unwrap();
        self.aba_input_val.insert(1, input_val);
        // Broadcast the input.
        let aba_val = ABAVal::new(
            self.name,
            self.epoch,
            1,
            input_val,
            VAL_PHASE,
            self.signature_service.clone(),
        ).await;
        let message = ConsensusMessage::ABAVal(aba_val.clone());
        Synchronizer::transmit(
            message, 
            &self.name, 
            None, 
            &self.network_filter_smvba, 
            &self.committee,
            TCVBA,
        ).await?;
        self.handle_aba_val(aba_val).await?;

        Ok(())
    }

    pub async fn check_timeout(&mut self, chain: &mut Chain) -> ConsensusResult<()> {
        // If we have tc, enter in view-change.
        if self.tc_cache.contains_key(&self.epoch) {
            self.is_view_change = true;
        }
        // If we are changing view, no need to send timeout.
        if self.is_view_change {
            return Ok(());
        }
        let current_round = chain.height;
        // If there is any chain's round greater than leader's, try to view change.
        for (_, other_chain) in self.pubkey_to_chain.clone() {
            if current_round + LAMBDA_VAL < other_chain.height {
                self.is_view_change = true;
                self.local_timeout_round(chain).await?;
                break;
            }
        }
        Ok(())
    }

    pub fn is_expired_aba_message(&mut self, epoch: SeqNumber, round: SeqNumber) -> bool {
        if self.aba_output_val.is_some() {
            return true;
        }
        epoch < self.epoch || (epoch == self.epoch && round < self.aba_round)
    }

    async fn handle_sync_request(
        &mut self,
        digest: Digest,
        sender: PublicKey,
    ) -> ConsensusResult<()> {
        if let Some(bytes) = self.store.read(digest.to_vec()).await? {
            let block = bincode::deserialize(&bytes)?;
            let message = ConsensusMessage::SyncReply(block);
            Synchronizer::transmit(
                message,
                &self.name,
                Some(&sender),
                &self.network_filter,
                &self.committee,
                HOTSTUFF,
            ).await?;
        }
        Ok(())
    }

    pub async fn handle_val_phase(&mut self, aba_val: ABAVal) -> ConsensusResult<()> {
        debug!("Received aba_val {:?}", aba_val);
        aba_val.verify()?;

        if let Some(aba_proof) = self.aggregator.add_aba_val(aba_val.clone())? {
            debug!("Assembled {:?}", aba_proof);
            if aba_proof.tag == true {
                self.aba_val_phase_cache1.insert((aba_proof.epoch, aba_proof.round), aba_proof);
            } else {
                self.aba_val_phase_cache2.insert((aba_proof.epoch, aba_proof.round), aba_proof);
            }
        }
        Ok(())
    }

    pub async fn process_val_phase_1(&mut self, proof: ABAProof) -> ConsensusResult<()> {
        if self.aba_input_val[&proof.round] == proof.val {
            return Ok(());
        }
        let aba_val = ABAVal::new(
            self.name,
            proof.epoch,
            proof.round,
            proof.val,
            proof.phase,
            self.signature_service.clone(),
        ).await;
        // only broadcast once
        if !self.val_value_broadcasted {
            self.val_value_broadcasted = true;
            let message = ConsensusMessage::ABAVal(aba_val.clone());
            Synchronizer::transmit(
                message,
                &self.name,
                None,
                &self.network_filter_smvba,
                &self.committee,
                TCVBA,
            ).await?;
            self.handle_aba_val(aba_val).await?;
        }
        Ok(())
    }

    pub async fn process_val_phase_2(&mut self, proof: ABAProof) -> ConsensusResult<()> {
        // only broadcast first value
        if !self.bin_values.is_empty() {
            let aba_val = ABAVal::new(
                self.name,
                proof.epoch,
                proof.round,
                proof.val,
                AUX_PHASE,
                self.signature_service.clone(),
            ).await;
            // only broadcast once
            if !self.aux_value_broadcasted {
                self.aux_value_broadcasted = true;
                let message = ConsensusMessage::ABAVal(aba_val.clone());
                Synchronizer::transmit(
                    message,
                    &self.name,
                    None,
                    &self.network_filter_smvba,
                    &self.committee,
                    TCVBA,
                ).await?;
                self.handle_aba_val(aba_val).await?;
            }
        }
        self.bin_values.insert(proof.val);
        Ok(())
    }

    pub async fn handle_aux_phase(&mut self, aba_val: ABAVal) -> ConsensusResult<()> {
        aba_val.verify()?;
        *self.aux_value_nums
            .entry((aba_val.epoch, aba_val.round))
            .or_insert(0) += 1;
        if let Some(aba_proof) = self.aggregator.add_aba_val(aba_val.clone())? {
            debug!("Assembled {:?}", aba_proof);
            self.aba_aux_phase_cache.insert((aba_proof.epoch, aba_proof.round), aba_proof);
        }
        Ok(())
    }

    pub async fn process_aux_phase(&mut self, epoch: SeqNumber, round: SeqNumber) -> ConsensusResult<()> {
        let share = RandomnessShare::new(
            epoch,
            round,
            self.name,
            self.signature_service.clone(),
        ).await;
        if !self.coin_share_broadcasted {
            self.coin_share_broadcasted = true;
            let message = ConsensusMessage::ABACoinShare(share.clone());
            Synchronizer::transmit(
                message,
                &self.name,
                None,
                &self.network_filter_smvba,
                &self.committee,
                TCVBA,
            ).await?;
            self.handle_coin_share(share).await?;
        }
        Ok(())
    }

    pub async fn handle_coin_share(&mut self, share: RandomnessShare) -> ConsensusResult<()> {
        if self.is_expired_aba_message(share.epoch, share.round) {
            return Ok(());
        }

        share.verify(&self.committee, &self.pk_set)?;

        if let Some(coin) = self.aggregator.add_aba_random(share.clone(), &self.pk_set)? {
            debug!("Assembled coin {}", coin);
            self.aba_coin_cache.insert((share.epoch, share.round), coin);
        }
        Ok(())
    }

    pub async fn advance_aba_round(&mut self, input_val: u64) -> ConsensusResult<()> {
        self.aba_round += 1;
        debug!("Moved to aba round {}", self.aba_round);
        self.phase = VAL_PHASE;
        self.aba_input_val.insert(self.aba_round, input_val);
        let aba_val = ABAVal::new(
            self.name,
            self.epoch,
            self.aba_round,
            input_val,
            VAL_PHASE,
            self.signature_service.clone(),
        ).await;
        // Clear round based data structures
        // Aggregator will be cleared later
        let current_epoch = self.epoch;
        let current_aba = self.aba_round;
        self.val_value_broadcasted = false;
        self.aux_value_broadcasted = false;
        self.coin_share_broadcasted = false;
        self.aux_proof_broadcasted = false;
        self.bin_values.clear();
        self.aba_val_phase_cache1.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba);
        self.aba_val_phase_cache2.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba);
        self.aba_aux_phase_cache.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba);
        // self.aba_coin_cache.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba); // coin is used for multiple rounds
        self.aux_value_nums.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba);
        self.aba_proof_cache.retain(|(k1, k2), _| k1 == &current_epoch && k2 >= &current_aba);
        
        let message = ConsensusMessage::ABAVal(aba_val.clone());
        Synchronizer::transmit(
            message,
            &self.name,
            None,
            &self.network_filter_smvba,
            &self.committee,
            TCVBA,
        ).await?;
        self.handle_aba_val(aba_val).await?;

        Ok(())
    }

    pub async fn process_coin_share(&mut self, epoch: SeqNumber, round: SeqNumber, coin: usize) -> ConsensusResult<()> {
        if self.bin_values.len() == 1 {
            let v = *self.bin_values.iter().next().unwrap();
            if v % 2 == coin as u64 {
                self.aba_output_val = Some(v);
                // aux proof must be Some
                let proof = self.aba_aux_phase_cache.get(&(epoch, round)).unwrap();
                if !self.aux_proof_broadcasted {
                    self.aux_proof_broadcasted = true;
                    // No need to handle ourself.
                    let message = ConsensusMessage::ABAProof(proof.clone());
                    Synchronizer::transmit(
                        message,
                        &self.name,
                        None,
                        &self.network_filter_smvba,
                        &self.committee,
                        TCVBA,
                    ).await?;
                }
                return Ok(());
            }
            self.advance_aba_round(v).await?;
        } else {
            if let Some(proof) = self.aba_aux_phase_cache.get(&(epoch, round)) {
                let v= proof.val;
                if v % 2 == coin as u64 {
                    self.aba_output_val = Some(v);
                    // aux proof must be Some
                    // let proof = self.aba_aux_phase_cache.get(&(epoch, round)).unwrap();
                    if !self.aux_proof_broadcasted {
                        self.aux_proof_broadcasted = true;
                        let message = ConsensusMessage::ABAProof(proof.clone());
                        Synchronizer::transmit(
                            message,
                            &self.name,
                            None,
                            &self.network_filter_smvba,
                            &self.committee,
                            TCVBA,
                        ).await?;
                    }
                    return Ok(());
                }
            }
            // advance aba round
            self.advance_aba_round(coin as u64).await?;
        }
        
        Ok(())
    }

    pub async fn handle_aba_val(&mut self, aba_val: ABAVal) -> ConsensusResult<()> {
        if self.is_expired_aba_message(aba_val.epoch, aba_val.round) {
            return Ok(());
        }

        let phase = aba_val.phase;
        match phase {
            VAL_PHASE => {
                self.handle_val_phase(aba_val.clone()).await?;
            },
            AUX_PHASE => {
                self.handle_aux_phase(aba_val.clone()).await?;
            },
            _ => return Err(ConsensusError::InvalidPhase(phase))
        }
        Ok(())
    }

    pub async fn handle_aba_proof(&mut self, proof: ABAProof) -> ConsensusResult<()> {
        if self.is_expired_aba_message(proof.epoch, proof.round) {
            return Ok(());
        }
        proof.verify(&self.committee)?; 
        self.aba_proof_cache.insert((proof.epoch, proof.round), proof);
        Ok(())
    }

    pub async fn process_aba_proof(&mut self, proof: ABAProof) -> ConsensusResult<()> {
        // We must have coin.
        if let Some(coin) = self.aba_coin_cache.get(&(proof.epoch, proof.round)) {
            if proof.val % 2 == *coin as u64 {
                self.aba_output_val = Some(proof.val);
            }
        }
        Ok(())
    }

    pub async fn process_aba_phase(&mut self) -> ConsensusResult<()> {
        // Always true, only process tc once.
        if self.phase >= INIT_PHASE && !self.tc_processed {
            if let Some(tc) = self.tc_cache.get(&self.epoch) {
                self.process_tc(tc.clone()).await?;
                self.tc_processed = true;
            }
        }

        if self.phase >= VAL_PHASE {
            if let Some(proof) = self.aba_val_phase_cache1.get(&(self.epoch, self.aba_round)) {
                self.process_val_phase_1(proof.clone()).await?;
            }
            if let Some(proof) = self.aba_val_phase_cache2.get(&(self.epoch, self.aba_round)) {
                self.process_val_phase_2(proof.clone()).await?;
            }
        }

        if self.phase >= AUX_PHASE {
            if let Some(proof) = self.aba_aux_phase_cache.get(&(self.epoch, self.aba_round)) {
                self.process_aux_phase(proof.epoch, proof.round).await?;
            }
        }

        if self.phase >= COIN_PHASE {
            if let Some(coin) = self.aba_coin_cache.get(&(self.epoch, self.aba_round)) {
                self.process_coin_share(self.epoch, self.aba_round, *coin).await?;
            }
        }

        // We can process HELP_PHASE if we are changing view.
        if self.is_view_change {
            if let Some(proof) = self.aba_proof_cache.get(&(self.epoch, self.aba_round)) {
                self.process_aba_proof(proof.clone()).await?;
            } 
        }

        Ok(())
    }

    pub async fn update_aba_phase(&mut self, chain: &mut Chain) -> ConsensusResult<()> {
        // If we are not changing view, return.
        if !self.is_view_change {
            return Ok(());
        }
        if self.phase == INIT_PHASE && self.tc_processed {
            info!("Epoch {}, enter VAL_PHASE", self.epoch);
            self.phase = VAL_PHASE;
        }

        if self.phase == VAL_PHASE && !self.bin_values.is_empty() {
            info!("Epoch {}, enter AUX_PHASE", self.epoch);
            self.phase = AUX_PHASE;
        }

        if self.phase == AUX_PHASE 
            && self.aba_coin_cache.contains_key(&(self.epoch, self.aba_round)) 
            && *self.aux_value_nums
                .entry((self.epoch, self.aba_round))
                .or_insert(0) >= self.committee.quorum_threshold() as u64
            && (self.bin_values.len() == 2 || self.aba_aux_phase_cache
                .contains_key(&(self.epoch, self.aba_round)))
        {
            info!("Epoch {}, enter COIN_PHASE", self.epoch);
            self.phase = COIN_PHASE;
        }

        // if self.phase == COIN_PHASE 
        //     && self.aba_aux_phase_cache.contains_key(&(self.epoch, self.aba_round)) 
        //     && self.aba_output_val.is_some()
        // {
        //     debug!("Epoch {}, enter HELP_PHASE", self.epoch);
        //     self.phase = HELP_PHASE;
        // }

        if self.aba_output_val.is_some() {
            info!("Epoch {}, enter OUTPUT_PHASE", self.epoch);
            // prepare block
            let output_height = self.aba_output_val.unwrap();
            debug!("output_height: {}", output_height);
            if !chain.height_to_digest.contains_key(&output_height) {
                debug!("No such height!");
                return Ok(());
            }  
            let digest = chain.height_to_digest.get(&output_height).unwrap();     
            let block= self.synchronizer.get_block(
                self.name, 
                digest,
                None
            ).await?;
            if block.is_none() {
                info!("No such block!");
                return Ok(());
            }
            // commit block 
            self.commit(block.unwrap(), chain).await?;
            // advance epoch
            self.epoch += 1;
            info!("advance epoch, epoch: {}", self.epoch);
            // reset previous leader chain
            // chain.reset(self.epoch);
            chain.height_to_digest.retain(|k, _| k > &output_height);
            chain.high_qc = QC::genesis();
            // clean all data structure, only base epoch
            let current_epoch = self.epoch;
            self.is_view_change = false;
            self.tc_processed = false;
            self.val_value_broadcasted = false;
            self.aux_value_broadcasted = false;
            self.coin_share_broadcasted = false;
            self.aux_proof_broadcasted = false;
            self.aggregator.cleanup_aba(&self.epoch);
            self.bin_values.clear();
            self.aba_round = 1;
            self.phase = INIT_PHASE;
            self.aba_output_val = None;
            self.aba_input_val.clear();
            self.tc_cache.retain(|k, _| k >= &current_epoch);
            self.aba_val_phase_cache1.retain(|(k, _), _| k >= &current_epoch);
            self.aba_val_phase_cache2.retain(|(k, _), _| k >= &current_epoch);
            self.aba_aux_phase_cache.retain(|(k, _), _| k >= &current_epoch);
            self.aba_coin_cache.retain(|(k, _), _| k >= &current_epoch);
            self.aux_value_nums.retain(|(k, _), _| k >= &current_epoch);
            self.aba_proof_cache.retain(|(k, _), _| k >= &current_epoch);
            // Broadcast new block.
            let mut our_chain = self.pubkey_to_chain.get(&self.name).cloned().unwrap();
            our_chain.height += NEXT_EPOCH_HEIGHT;
            let block = self.generate_proposal(&our_chain, true).await;
            self.broadcast_propose(block, &mut our_chain).await?;
            self.pubkey_to_chain.insert(self.name, our_chain);
        } else {
            self.process_aba_phase().await?;
        }

        Ok(())
    }

    pub async fn run(&mut self) {
        // Propose the first block for the chain led by us.
        let mut chain = self.pubkey_to_chain.get(&self.name).cloned().unwrap();
        let block = self.generate_proposal(&chain, false).await;
        let _ = self.broadcast_propose(block, &mut chain).await;
        self.pubkey_to_chain.insert(self.name, chain);
        
        loop {
            self.total_epoch += 1;
            info!("Epoch {}, leader: {}", self.total_epoch, self.leader_elector.get_leader(self.epoch));
            let result = tokio::select! {
                Some(message) = self.core_channel.recv() => {
                    match message {
                        ConsensusMessage::Propose(block) => {
                            info!("receive propose from {}, height {}", block.author, block.height);
                            let mut chain = self.pubkey_to_chain.get(&block.author).cloned().unwrap();
                            let result = self.handle_proposal(&block, &mut chain).await;
                            self.pubkey_to_chain.insert(block.author, chain);
                            result
                        },
                        ConsensusMessage::Vote(vote) => {
                            info!("receive vote from {}", vote.author);
                            let mut chain = self.pubkey_to_chain.get(&vote.proposer).cloned().unwrap(); // Use proposer instead of author.
                            let result = self.handle_vote(&vote, &mut chain).await;
                            self.pubkey_to_chain.insert(vote.proposer, chain);
                            result
                        },
                        ConsensusMessage::LoopBack(block) => {
                            info!("Epoch: {}, receive loopback block from {}", self.total_epoch, block.author);
                            let mut chain = self.pubkey_to_chain.get(&block.author).cloned().unwrap();
                            let result = self.process_block(&block, &mut chain).await;
                            self.pubkey_to_chain.insert(block.author, chain);
                            result
                        },                        
                        ConsensusMessage::SyncRequest(digest, sender) => {
                            info!("receive sync request {} from {}", digest, sender);
                            let result = self.handle_sync_request(digest, sender).await;
                            result
                        },
                        ConsensusMessage::SyncReply(block) => {
                            info!("receive sync reply from {}", block.author);
                            let mut chain = self.pubkey_to_chain.get(&block.author).cloned().unwrap();
                            let result = self.handle_proposal(&block, &mut chain).await;
                            self.pubkey_to_chain.insert(block.author, chain);
                            result
                        },
                        _ => panic!("Unexpected protocol message")
                    }
                },
                Some(message) = self.smvba_channel.recv() => {
                    match message {
                        ConsensusMessage::Timeout(timeout) => {
                            info!("receive timeout from {}, epoch {}", timeout.author, timeout.epoch);
                            let leader = self.leader_elector.get_leader(timeout.epoch);
                            let mut chain = self.pubkey_to_chain.get(&leader).cloned().unwrap();
                            let result = self.handle_timeout(&timeout, &mut chain).await;
                            self.pubkey_to_chain.insert(leader, chain);
                            result
                        },
                        ConsensusMessage::TC(tc) => {
                            info!("receive tc, epoch {}", tc.epoch);
                            let result = self.handle_tc(tc).await;
                            result
                        },
                        ConsensusMessage::ABAVal(aba_val) => {
                            info!("receive aba_val from {}, epoch {}, round {}, phase {}", aba_val.author, aba_val.epoch, aba_val.round, aba_val.phase);
                            let result = self.handle_aba_val(aba_val).await;
                            result
                        },
                        ConsensusMessage::ABAProof(proof) => {
                            info!("receive aba_proof, epoch {}, round {}", proof.epoch, proof.round);
                            let result = self.handle_aba_proof(proof).await;
                            result
                        },
                        ConsensusMessage::ABACoinShare(share) => {
                            info!("receive aba_coin_share from {}, epoch {}, round {}", share.author, share.epoch, share.round);
                            let result = self.handle_coin_share(share).await;
                            result
                        },
                        _ => panic!("Unexpected protocol message")
                    }
                },
            };
            match result {
                Ok(()) => (),
                Err(ConsensusError::StoreError(e)) => error!("{}", e),
                Err(ConsensusError::SerializationError(e)) => error!("Store corrupted. {}", e),
                Err(e) => warn!("{}", e),
            }

            let leader = self.leader_elector.get_leader(self.epoch);
            let mut chain = self.pubkey_to_chain.get(&leader).cloned().unwrap(); 
            let _ = self.update_aba_phase(&mut chain).await;
            self.pubkey_to_chain.insert(leader, chain);
            
            let leader = self.leader_elector.get_leader(self.epoch);
            let mut chain = self.pubkey_to_chain.get(&leader).cloned().unwrap(); // should be leader not self
            let _ = self.check_timeout(&mut chain).await;            
            self.pubkey_to_chain.insert(leader, chain);
        }
    }
}
