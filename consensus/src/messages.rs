use crate::config::Committee;
use crate::core::{SeqNumber};
use crate::error::{ConsensusError, ConsensusResult};
use crypto::{Digest, Hash, PublicKey, Signature, SignatureService};
use ed25519_dalek::Digest as _;
use ed25519_dalek::Sha512;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::convert::TryInto;
use std::fmt;
use threshold_crypto::{PublicKeySet, SignatureShare};

#[cfg(test)]
#[path = "tests/messages_tests.rs"]
pub mod messages_tests;

// daniel: Add view, height, fallback in Block, Vote and QC
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Block {
    pub qc: QC, 
    pub author: PublicKey,
    pub height: SeqNumber,
    pub epoch: SeqNumber,
    pub payload: Vec<Digest>,
    pub references: Vec<(PublicKey, Digest)>,
    pub signature: Signature,
}

impl Block {
    pub async fn new(
        qc: QC,
        author: PublicKey,
        height: SeqNumber,
        epoch: SeqNumber,
        payload: Vec<Digest>,
        references: Vec<(PublicKey, Digest)>,
        mut signature_service: SignatureService,
    ) -> Self {
        let block = Self {
            qc,
            author,
            height,
            epoch,
            payload,
            references,
            signature: Signature::default(),
        };

        let signature = signature_service.request_signature(block.digest()).await;
        Self { signature, ..block }
    }

    pub fn genesis() -> Self {
        Block::default()
    }

    pub fn parent(&self) -> &Digest {
        &self.qc.hash
    }

    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        // Ensure the authority has voting rights.
        let voting_rights = committee.stake(&self.author);
        ensure!(
            voting_rights > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature.verify(&self.digest(), &self.author)?;

        // Check the embedded QC.
        if self.qc != QC::genesis() {
            self.qc.verify(committee)?;
        }

        Ok(())
    }
}

impl Hash for Block {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.author.0);
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.epoch.to_le_bytes());
        for x in &self.payload {
            hasher.update(x);
        }
        for x in &self.references {
            hasher.update(&x.0);
            hasher.update(&x.1);
        }
        hasher.update(&self.qc.hash);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}: B(author {}, height {}, epoch {}, qc {:?}, payload_len {}, references_len {})",
            self.digest(),
            self.author,
            self.height,
            self.epoch,
            self.qc,
            self.payload.iter().map(|x| x.size()).sum::<usize>(),
            self.references.len(),
        )
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "B{}", self.height)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Vote {
    pub hash: Digest,
    pub epoch: SeqNumber,
    pub height: SeqNumber,
    pub author: PublicKey, // vote author
    pub proposer: PublicKey, // block author
    pub signature: Signature,
}

impl Vote {
    pub async fn new(
        block: &Block,
        author: PublicKey,
        mut signature_service: SignatureService,
    ) -> Self {
        let vote = Self {
            hash: block.digest(),
            epoch: block.epoch,
            height: block.height,
            author,
            proposer: block.author,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(vote.digest()).await;
        Self { signature, ..vote }
    }

    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        // Ensure the authority has voting rights.
        ensure!(
            committee.stake(&self.author) > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature.verify(&self.digest(), &self.author)?;
        Ok(())
    }
}

impl Hash for Vote {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(&self.hash);
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        // hasher.update(self.proposer.0);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Vote {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "Vote(blockhash {}, proposer {}, height {}, epoch {}, voter {})",
            self.hash, self.proposer, self.height, self.epoch, self.author
        )
    }
}

impl fmt::Display for Vote {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Vote hash {}", self.hash)
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct QC {
    pub hash: Digest,
    pub epoch: SeqNumber,
    pub height: SeqNumber,
    pub votes: Vec<(PublicKey, Signature)>,
}

impl QC {
    pub fn genesis() -> Self {
        QC::default()
    }

    pub fn timeout(&self) -> bool {
        self.hash == Digest::default() && self.height != 0
    }

    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        // Ensure the QC has a quorum.
        let mut weight = 0; //票数
        let mut used = HashSet::new(); //防止重复统计
        for (name, _) in self.votes.iter() {
            ensure!(
                !used.contains(name),
                ConsensusError::AuthorityReuseinQC(*name)
            );
            let voting_rights = committee.stake(name);
            ensure!(voting_rights > 0, ConsensusError::UnknownAuthority(*name));
            used.insert(*name);
            weight += voting_rights;
        }
        ensure!(
            weight >= committee.quorum_threshold(),
            ConsensusError::QCRequiresQuorum
        );

        // Check the signatures.
        Signature::verify_batch(&self.digest(), &self.votes).map_err(ConsensusError::from)
    }
}

impl Hash for QC {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(&self.hash);
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for QC {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "QC(hash {}, epoch {}, height {})",
            self.hash, self.epoch, self.height
        )
    }
}

impl PartialEq for QC {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.height == other.height
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Timeout {
    pub high_qc: QC,
    pub epoch: SeqNumber,
    pub author: PublicKey,
    pub signature: Signature,
}

impl Timeout {
    pub async fn new(
        high_qc: QC,
        epoch: SeqNumber,
        author: PublicKey,
        mut signature_service: SignatureService,
    ) -> Self {
        let timeout = Self {
            high_qc,
            epoch,
            author,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(timeout.digest()).await;
        Self {
            signature,
            ..timeout
        }
    }

    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        // Ensure the authority has voting rights.
        ensure!(
            committee.stake(&self.author) > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature.verify(&self.digest(), &self.author)?;

        // Check the embedded QC.
        if self.high_qc != QC::genesis() {
            self.high_qc.verify(committee)?;
        }
        Ok(())
    }
}

impl Hash for Timeout {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.high_qc.height.to_le_bytes());
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "TV(author {}, epoch {}, high_qc {:?})", self.author, self.epoch, self.high_qc)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TC {
    pub epoch: SeqNumber,
    pub votes: Vec<(PublicKey, Signature, SeqNumber)>,
}

impl TC {
    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        // Ensure the QC has a quorum.
        let mut weight = 0;
        let mut used = HashSet::new();
        for (name, _, _) in self.votes.iter() {
            ensure!(!used.contains(name), ConsensusError::AuthorityReuseinTC(*name));
            let voting_rights = committee.stake(name);
            ensure!(voting_rights > 0, ConsensusError::UnknownAuthority(*name));
            used.insert(*name);
            weight += voting_rights;
        }
        ensure!(
            weight >= committee.quorum_threshold(),
            ConsensusError::TCRequiresQuorum
        );

        // Check the signatures.
        for (author, signature, high_qc_round) in &self.votes {
            let mut hasher = Sha512::new();
            hasher.update(self.epoch.to_le_bytes());
            hasher.update(high_qc_round.to_le_bytes());
            let digest = Digest(hasher.finalize().as_slice()[..32].try_into().unwrap());
            signature.verify(&digest, author)?;
        }
        Ok(())
    }

    pub fn high_qc_rounds(&self) -> Vec<SeqNumber> {
        self.votes.iter().map(|(_, _, r)| r).cloned().collect()
    }
}

impl fmt::Debug for TC {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "TC(epoch {}, high_qc_rounds {:?})", self.epoch, self.high_qc_rounds())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ABAVal {
    pub author: PublicKey,
    pub epoch: SeqNumber,
    pub round: SeqNumber,
    pub phase: u8,
    pub val: u64,
    pub signature: Signature,
}

impl ABAVal {
    pub async fn new(
        author: PublicKey,
        epoch: SeqNumber,
        round: SeqNumber, // ABA round
        val: u64,
        phase: u8,
        mut signature_service: SignatureService,
    ) -> Self {
        let aba_val = Self {
            author,
            epoch,
            round,
            val,
            phase,
            signature: Signature::default(),
        };
        let signature= signature_service.request_signature(aba_val.digest()).await;
        Self {
            signature,
            ..aba_val
        }
    }

    pub fn verify(&self) -> ConsensusResult<()> {
        self.signature.verify(&self.digest(), &self.author)?;
        Ok(())
    }
}

impl Hash for ABAVal {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.val.to_le_bytes());
        hasher.update(self.phase.to_le_bytes());
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for ABAVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ABAVal(epoch {}, round {}, phase {}, val {})",
            self.epoch, self.round, self.phase, self.val
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ABAProof {
    pub epoch: SeqNumber,
    pub round: SeqNumber,
    pub val: u64,
    pub phase: u8,
    pub votes: Vec<(PublicKey, Signature)>,
    pub tag: bool, // tag is true if f+1
}

impl ABAProof {
    pub fn verify(&self, committee: &Committee) -> ConsensusResult<()> {
        let mut weight = 0;
        let mut used = HashSet::new();
        for (name, _) in self.votes.iter() {
            ensure!(!used.contains(name), ConsensusError::AuthorityReuseinABA(*name, self.phase));
            let voting_rights = committee.stake(name);
            ensure!(voting_rights > 0, ConsensusError::UnknownAuthority(*name));
            used.insert(*name);
            weight += voting_rights;
        }
        ensure!(
            weight >= committee.quorum_threshold(),
            ConsensusError::QCRequiresQuorum
        );

        // Check the signatures.
        Signature::verify_batch(&self.digest(), &self.votes).map_err(ConsensusError::from)
    }
}

impl Hash for ABAProof {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.val.to_le_bytes());
        hasher.update(self.phase.to_le_bytes());
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for ABAProof {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f, 
            "ABAProof(epoch {}, round {}, phase {}, val {})", 
            self.epoch, self.round, self.phase, self.val,
        )
    }
}

// leader选举时 每个发送自己的randomshare
#[derive(Clone, Serialize, Deserialize)]
pub struct RandomnessShare {
    pub epoch: SeqNumber,
    pub round: SeqNumber,
    pub author: PublicKey,
    pub signature_share: SignatureShare,
}

impl RandomnessShare {
    pub async fn new(
        epoch: SeqNumber,
        round: SeqNumber,
        author: PublicKey,
        mut signature_service: SignatureService,
    ) -> Self {
        let mut hasher = Sha512::new();
        hasher.update(epoch.to_le_bytes());
        hasher.update(round.to_le_bytes());
        let digest = Digest(hasher.finalize().as_slice()[..32].try_into().unwrap());
        let signature_share = signature_service
            .request_tss_signature(digest)
            .await
            .unwrap();
        Self {
            epoch,
            round,
            author,
            signature_share,
        }
    }

    pub fn verify(&self, committee: &Committee, pk_set: &PublicKeySet) -> ConsensusResult<()> {
        // Ensure the authority has voting rights.
        ensure!(
            committee.stake(&self.author) > 0,
            ConsensusError::UnknownAuthority(self.author)
        );
        let tss_pk = pk_set.public_key_share(committee.id(self.author));
        // Check the signature.
        ensure!(
            tss_pk.verify(&self.signature_share, &self.digest()),
            ConsensusError::InvalidThresholdSignature(self.author)
        );

        Ok(())
    }
}

impl Hash for RandomnessShare {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.round.to_le_bytes());
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for RandomnessShare {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "RandomnessShare(author {}, epoch {}, round {})",
            self.author, self.epoch, self.round,
        )
    }
}

// f+1 个 RandomnessShare 合成的
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct RandomCoin {
    pub epoch: SeqNumber,
    pub round: SeqNumber,
    pub shares: Vec<RandomnessShare>,
}

impl RandomCoin {}

impl fmt::Debug for RandomCoin {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "RandomCoin(epoch {}, round {})",
            self.epoch, self.round,
        )
    }
}
