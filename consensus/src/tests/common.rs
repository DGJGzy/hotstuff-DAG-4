use crate::config::Committee;
use crate::core::{SeqNumber, VAL_PHASE};
use crate::mempool::{ConsensusMempoolMessage, PayloadStatus};
use crate::messages::{ABAVal, Block, Timeout, Vote, QC};
use crypto::{Hash as _};
use crypto::{generate_keypair, Digest, PublicKey, SecretKey, Signature};
use rand::rngs::StdRng;
use rand::RngCore as _;
use rand::SeedableRng as _;
use tokio::sync::mpsc::Receiver;

const NODES: usize = 4;
// Fixture.
pub fn keys() -> Vec<(PublicKey, SecretKey)> {
    let mut rng = StdRng::from_seed([0; 32]);
    (0..NODES).map(|_| generate_keypair(&mut rng)).collect()
}

// Fixture.
pub fn committee() -> Committee {
    Committee::new(
        keys()
            .into_iter()
            .enumerate()
            .map(|(i, (name, _))| {
                let address = format!("0.0.0.0:{}", i).parse().unwrap();
                let smvba_address = format!("0.0.0.0:{}", 100 + i).parse().unwrap();
                let stake = 1;
                (name, 0, stake, address, smvba_address)
            })
            .collect(),
        /* epoch */ 1,
    )
}

impl Committee {
    pub fn increment_base_port(&mut self, base_port: u16) {
        for authority in self.authorities.values_mut() {
            let port = authority.address.port();
            let port_s = authority.smvba_address.port();
            authority.address.set_port(base_port + port);
            authority.smvba_address.set_port(base_port + port_s);
        }
    }
}

impl Block {
    pub fn new_from_key(
        qc: QC,
        author: PublicKey,
        height: SeqNumber,
        payload: Vec<Digest>,
        secret: &SecretKey,
    ) -> Self {
        let block = Block {
            qc,
            author,
            height,
            epoch: 0,
            payload,
            references: Vec::new(),
            signature: Signature::default(),
        };
        let signature = Signature::new(&block.digest(), secret);
        Self { signature, ..block }
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.digest() == other.digest()
    }
}

impl Vote {
    pub fn new_from_key(
        hash: Digest,
        height: SeqNumber,
        proposer: PublicKey,
        author: PublicKey,
        secret: &SecretKey,
    ) -> Self {
        let vote = Self {
            hash,
            height,
            epoch: 0,
            proposer,
            author,
            signature: Signature::default(),
        };
        let signature = Signature::new(&vote.digest(), &secret);
        Self { signature, ..vote }
    }

    pub fn new_with_height(
        height: SeqNumber,
    ) -> Self {
        let (public_key, secret_key) = keys().pop().unwrap();
        let vote = Self {
            hash: Block::genesis().digest(),
            height,
            epoch: 0,
            proposer: public_key,
            author: public_key,
            signature: Signature::default(),
        };
        let signature = Signature::new(&vote.digest(), &secret_key);
        Self { signature, ..vote }
    }
}

impl PartialEq for Vote {
    fn eq(&self, other: &Self) -> bool {
        self.digest() == other.digest()
    }
}

impl Timeout {
    pub fn new_from_key(
        epoch: SeqNumber,
        author: PublicKey,
        secret: &SecretKey,
        high_qc: QC,
    ) -> Self {
        let timeout = Self {
            epoch,
            author,
            high_qc,
            signature: Signature::default(),
        };
        let signature = Signature::new(&timeout.digest(), &secret);
        Self { signature, ..timeout }
    }

    pub fn new_with_epoch(
        epoch: SeqNumber,
    ) -> Self {
        let (public_key, secret_key) = keys().pop().unwrap();
        let timeout = Self {
            epoch,
            author: public_key,
            high_qc: QC::default(),
            signature: Signature::default(),
        };
        let signature = Signature::new(&timeout.digest(), &secret_key);
        Self { signature, ..timeout }
    }
}

impl PartialEq for Timeout {
    fn eq(&self, other: &Self) -> bool {
        self.digest() == other.digest()
    }
}

// Fixture.
pub fn block() -> Block {
    let (public_key, secret_key) = keys().pop().unwrap();
    Block::new_from_key(QC::genesis(), public_key, 1, Vec::new(), &secret_key)
}

// Fixture.
pub fn vote() -> Vote {
    let (public_key, secret_key) = keys().pop().unwrap();
    Vote::new_from_key(block().digest(), 1, block().author, public_key, &secret_key)
}

// Fixture.
pub fn qc() -> QC {
    let mut keys = keys();
    let (public_key, _) = keys.pop().unwrap();
    let qc = QC {
        hash: Digest::default(),
        height: 1,
        epoch: 0,
        proposer: public_key,
        votes: Vec::new(),
    };
    let digest = qc.digest();
    let votes: Vec<_> = (0..3)
        .map(|_| {
            let (public_key, secret_key) = keys.pop().unwrap();
            (public_key, Signature::new(&digest, &secret_key))
        })
        .collect();
    QC { votes, ..qc }
}

pub fn timeout() -> Timeout {
    let mut keys = keys();
    let high_qc = qc(); // 使用已有的 qc() 函数创建 high_qc
    
    // 创建一个基础的 timeout 结构
    let timeout = Timeout {
        epoch: 1,
        author: keys[0].0, // 临时使用第一个公钥
        high_qc,
        signature: Signature::default(), // 临时签名，稍后会替换
    };
    
    // 计算需要签名的消息
    let digest = timeout.digest();
    
    // 使用第一个密钥对进行签名
    let (public_key, secret_key) = keys.pop().unwrap();
    let signature = Signature::new(&digest, &secret_key);
    
    Timeout {
        author: public_key,
        signature,
        ..timeout
    }
}

// pub fn tc() -> TC {
//     let mut keys = keys();
//     let epoch = 1;
//     let high_qc = qc();
    
//     // 创建多个 timeout 投票
//     let votes: Vec<_> = (0..3)
//         .map(|_| {
//             let (public_key, secret_key) = keys.pop().unwrap();
//             // 为每个作者创建一个临时 timeout 来计算签名
//             let temp_timeout = Timeout {
//                 epoch,
//                 author: public_key,
//                 high_qc: high_qc.clone(),
//                 signature: Signature::default(),
//             };
//             let digest = temp_timeout.digest();
//             let signature = Signature::new(&digest, &secret_key);
            
//             (public_key, signature, high_qc.height)
//         })
//         .collect();
    
//     TC {
//         epoch,
//         votes,
//     }
// }

pub fn aba_val() -> ABAVal {
    let mut keys = keys();
    let (public_key, secret_key) = keys.pop().unwrap();
    
    ABAVal::new_from_key(
        1,              // epoch
        1,              // round
        VAL_PHASE,      // phase
        1,              // val
        public_key,
        &secret_key,
    )
}

impl ABAVal {
    pub fn new_from_key(
        epoch: SeqNumber,
        round: SeqNumber,
        phase: u8,
        val: u64,
        author: PublicKey,
        secret: &SecretKey,
    ) -> Self {
        let aba_val = Self {
            epoch,
            round,
            phase,
            val,
            author,
            signature: Signature::default(),
        };
        let signature = Signature::new(&aba_val.digest(), &secret);
        Self { signature, ..aba_val }
    }

    pub fn new_with_epoch(
        epoch: SeqNumber,
    ) -> Self {
        let (public_key, secret_key) = keys().pop().unwrap();
        let aba_val = Self {
            epoch,
            round: 0,
            phase: VAL_PHASE,
            val: 0,
            author: public_key,
            signature: Signature::default(),
        };
        let signature = Signature::new(&aba_val.digest(), &secret_key);
        Self { signature, ..aba_val }
    }
}

impl PartialEq for ABAVal {
    fn eq(&self, other: &Self) -> bool {
        self.digest() == other.digest()
    }
}

// Fixture.
pub fn chain(keys: Vec<(PublicKey, SecretKey)>) -> Vec<Block> {
    let mut latest_qc = QC::genesis();
    keys.iter()
        .enumerate()
        .map(|(i, key)| {
            // Make a block.
            let (public_key, secret_key) = key;
            let block = Block::new_from_key(
                latest_qc.clone(),
                *public_key,
                1 + i as SeqNumber,
                Vec::new(),
                secret_key,
            );

            // Make a qc for that block (it will be used for the next block).
            let qc = QC {
                epoch: 0,
                hash: block.digest(),
                height: block.height,
                proposer: block.author,
                votes: Vec::new(),
            };
            let digest = qc.digest();
            let votes: Vec<_> = keys
                .iter()
                .map(|(public_key, secret_key)| (*public_key, Signature::new(&digest, secret_key)))
                .collect();
            latest_qc = QC { votes, ..qc };

            // Return the block.
            block
        })
        .collect()
}

// Fixture
pub struct MockMempool;

impl MockMempool {
    pub fn run(mut consensus_mempool_channel: Receiver<ConsensusMempoolMessage>) {
        tokio::spawn(async move {
            while let Some(message) = consensus_mempool_channel.recv().await {
                match message {
                    ConsensusMempoolMessage::Get(_max, sender) => {
                        let mut rng = StdRng::from_seed([0; 32]);
                        let mut payload = [0u8; 32];
                        rng.fill_bytes(&mut payload);
                        sender.send(vec![Digest(payload)]).unwrap();
                    }
                    ConsensusMempoolMessage::Verify(_block, sender) => {
                        sender.send(PayloadStatus::Accept).unwrap()
                    }
                    ConsensusMempoolMessage::Cleanup(_digests, _round) => (),
                }
            }
        });
    }
}
