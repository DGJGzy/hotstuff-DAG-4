use crate::config::{Committee, Stake};
use crate::core::{SeqNumber, VAL_PHASE};
use crate::error::{ConsensusError, ConsensusResult};
use crate::messages::{ABAProof, ABAVal, RandomnessShare, Timeout, Vote, QC, TC};
use crypto::{Digest, Hash as _, PublicKey, Signature};
use std::collections::{BTreeMap, HashMap, HashSet};
use threshold_crypto::PublicKeySet;
// use std::convert::TryInto;

#[cfg(test)]
#[path = "tests/aggregator_tests.rs"]
pub mod aggregator_tests;

#[derive(Clone)]
pub struct Aggregator {
    committee: Committee,
    votes_aggregators: HashMap<SeqNumber, HashMap<Digest, Box<QCMaker>>>,
    timeouts_aggregators: HashMap<SeqNumber, Box<TCMaker>>,
    aba_vals_aggregators: HashMap<(SeqNumber, SeqNumber, u8, u64), Box<ABAProofMaker>>,
    aba_randomcoin_aggregators: HashMap<(SeqNumber, SeqNumber), Box<ABARandomCoinMaker>>,
}

impl Aggregator {
    pub fn new(committee: Committee) -> Self {
        Self {
            committee,
            votes_aggregators: HashMap::new(),
            timeouts_aggregators: HashMap::new(),
            aba_vals_aggregators: HashMap::new(),
            aba_randomcoin_aggregators: HashMap::new(),
        }
    }

    pub fn add_vote(&mut self, vote: Vote) -> ConsensusResult<Option<QC>> {
        // TODO [issue #7]: A bad node may make us run out of memory by sending many votes
        // with different round numbers or different digests.

        // Add the new vote to our aggregator and see if we have a QC.
        self.votes_aggregators
            .entry(vote.height)
            .or_insert_with(HashMap::new)
            .entry(vote.digest())
            .or_insert_with(|| Box::new(QCMaker::new()))
            .append(vote, &self.committee)
    }

    pub fn add_timeout(&mut self, timeout: Timeout) -> ConsensusResult<Option<TC>> {
        // TODO: A bad node may make us run out of memory by sending many timeouts
        // with different round numbers.

        // Add the new timeout to our aggregator and see if we have a TC.
        self.timeouts_aggregators
            .entry(timeout.epoch)
            .or_insert_with(|| Box::new(TCMaker::new()))
            .append(timeout, &self.committee)
    }

    pub fn add_aba_val(&mut self, aba_val: ABAVal) -> ConsensusResult<Option<ABAProof>> {
        self.aba_vals_aggregators
            .entry((aba_val.epoch, aba_val.round, aba_val.phase, aba_val.val))
            .or_insert_with(|| Box::new(ABAProofMaker::new()))
            .append(aba_val, &self.committee)
    }

    pub fn add_aba_random(
        &mut self,
        share: RandomnessShare,
        pk_set: &PublicKeySet,
    ) -> ConsensusResult<Option<usize>> {
        self.aba_randomcoin_aggregators
            .entry((share.epoch, share.round))
            .or_insert_with(|| Box::new(ABARandomCoinMaker::new()))
            .append(share, &self.committee, pk_set)
    }

    pub fn cleanup(&mut self, epoch: &SeqNumber, round: &SeqNumber) {
        self.votes_aggregators.retain(|k, _| k >= round);
        self.timeouts_aggregators.retain(|k, _| k >= epoch);
    }

    pub fn cleanup_aba(&mut self, epoch: &SeqNumber) {
        self.aba_vals_aggregators.retain(|(k, _, _, _), _| k >= epoch);
        self.aba_randomcoin_aggregators.retain(|(k, _), _| k >= epoch);
    }
}

#[derive(Clone)]
struct QCMaker {
    weight: Stake,
    votes: Vec<(PublicKey, Signature)>,
    used: HashSet<PublicKey>,
}

impl QCMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            votes: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(&mut self, vote: Vote, committee: &Committee) -> ConsensusResult<Option<QC>> {
        let author = vote.author;

        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuseinQC(author)
        );

        self.votes.push((author, vote.signature));
        self.weight += committee.stake(&author);

        if self.weight >= committee.quorum_threshold() {
            self.weight = 0; // Ensures QC is only made once.
            return Ok(Some(QC {
                hash: vote.hash.clone(),
                epoch: vote.epoch,
                height: vote.height,
                votes: self.votes.clone(),
            }));
        }
        Ok(None)
    }
}

#[derive(Clone)]
struct TCMaker {
    weight: Stake,
    votes: Vec<(PublicKey, Signature, SeqNumber)>,
    used: HashSet<PublicKey>,
}

impl TCMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            votes: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(
        &mut self,
        timeout: Timeout,
        committee: &Committee,
    ) -> ConsensusResult<Option<TC>> {
        let author = timeout.author;

        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuseinTC(author)
        );

        // Add the timeout to the accumulator.
        self.votes
            .push((author, timeout.signature, timeout.high_qc.height));
        self.weight += committee.stake(&author);

        if self.weight >= committee.quorum_threshold() {
            self.weight = 0; // Ensures TC is only created once.
            return Ok(Some(TC {
                epoch: timeout.epoch,
                votes: self.votes.clone(),
            }));
        }
        Ok(None)
    }
}

#[derive(Clone)]
struct ABAProofMaker {
    weight: Stake,
    votes: Vec<(PublicKey, Signature)>,
    used: HashSet<PublicKey>,
}

impl ABAProofMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            votes: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(&mut self, aba_val: ABAVal, committee: &Committee) -> ConsensusResult<Option<ABAProof>> {
        let author = aba_val.author;

        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuseinABA(author, aba_val.phase)
        );

        self.votes.push((author, aba_val.signature));
        self.weight += committee.stake(&author);

        if self.weight >= committee.quorum_threshold() {
            self.weight = 0; 
            return Ok(Some(ABAProof {
                epoch: aba_val.epoch,
                round: aba_val.round,
                val: aba_val.val,
                phase: aba_val.phase,
                votes: self.votes.clone(),
                tag: false, 
            }));
        }

        if self.weight == committee.random_coin_threshold() && aba_val.phase == VAL_PHASE {
            return Ok(Some(ABAProof {
                epoch: aba_val.epoch,
                round: aba_val.round,
                val: aba_val.val,
                phase: aba_val.phase,
                votes: self.votes.clone(),
                tag: true, 
            }));
        }
        Ok(None)
    }
}

#[derive(Clone)]
struct ABARandomCoinMaker {
    weight: Stake,
    shares: Vec<RandomnessShare>,
    used: HashSet<PublicKey>,
}

impl ABARandomCoinMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            shares: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(
        &mut self,
        share: RandomnessShare,
        committee: &Committee,
        pk_set: &PublicKeySet,
    ) -> ConsensusResult<Option<usize>> {
        let author = share.author;
        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuseinCoin(author)
        );
        self.shares.push(share.clone());
        self.weight += committee.stake(&author);
        if self.weight == committee.random_coin_threshold() {
            // self.weight = 0; // Ensures QC is only made once.
            let mut sigs = BTreeMap::new();
            // Check the random shares.
            for share in self.shares.clone() {
                sigs.insert(
                    committee.id(share.author.clone()),
                    share.signature_share.clone(),
                );
            }
            if let Ok(sig) = pk_set.combine_signatures(sigs.iter()) {
                let id = usize::from_be_bytes((&sig.to_bytes()[0..8]).try_into().unwrap()) % 2;

                return Ok(Some(id));
            }
        }
        Ok(None)
    }
}
