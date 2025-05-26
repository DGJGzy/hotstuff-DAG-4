// use super::*;
// use crate::common::{chain, committee, keys, MockMempool};
// use crypto::{SecretKey, SecretShare};
// use std::fs;
// use tokio::sync::mpsc::channel;

// async fn core(
//     name: PublicKey,
//     secret: SecretKey,
//     store_path: &str,
//     pk_set: PublicKeySet,
// ) -> (
//     Sender<ConsensusMessage>,
//     Receiver<FilterInput>,
//     Receiver<FilterInput>,
//     Receiver<Block>,
// ) {
//     let (tx_core, rx_core) = channel(1);
//     let (_, rx_smvba) = channel(1);
//     let (tx_network, rx_network) = channel(3);
//     let (tx_network_smvba, rx_network_smvba) = channel(3);
//     let (tx_consensus_mempool, rx_consensus_mempool) = channel(1);
//     let (tx_commit, rx_commit) = channel(1);

//     let parameters = Parameters {
//         timeout_delay: 100,
//         ..Parameters::default()
//     };
//     let signature_service = SignatureService::new(secret, None);
//     let _ = fs::remove_dir_all(store_path);
//     let store = Store::new(store_path).unwrap();
//     let leader_elector = LeaderElector::new(committee());
//     MockMempool::run(rx_consensus_mempool);
//     let mempool_driver = MempoolDriver::new(tx_consensus_mempool);
//     let synchronizer = Synchronizer::new(
//         name,
//         committee(),
//         store.clone(),
//         /* network_channel */ tx_network.clone(),
//         /* core_channel */ tx_core.clone(),
//         parameters.sync_retry_delay,
//     )
//     .await;
//     let mut core = Core::new(
//         name,
//         committee(),
//         parameters,
//         signature_service,
//         pk_set,
//         store,
//         leader_elector,
//         mempool_driver,
//         synchronizer,
//         /* core_channel */ rx_core,
//         rx_smvba,
//         /* network_channel */ tx_network,
//         tx_network_smvba,
//         /* commit_channel */ tx_commit,
//     );
//     tokio::spawn(async move {
//         core.run().await;
//     });
//     (tx_core, rx_network, rx_network_smvba, rx_commit)
// }

// fn leader_keys(height: SeqNumber) -> (PublicKey, SecretKey) {
//     let leader_elector = LeaderElector::new(committee());
//     let leader = leader_elector.get_leader(height);
//     keys()
//         .into_iter()
//         .find(|(public_key, _)| *public_key == leader)
//         .unwrap()
// }

// #[tokio::test]
// async fn handle_proposal() {
//     // Make a block and the vote we expect to receive.
//     let block = chain(vec![leader_keys(1)]).pop().unwrap();
//     let (public_key, secret_key) = keys().pop().unwrap();
//     let vote = Vote::new_from_key(
//         block.digest(),
//         block.height,
//         block.author,
//         public_key,
//         &secret_key,
//     );
//     let tss_keys = SecretShare::default();
//     let pk_set = tss_keys.pkset.clone();

//     // Run a core instance.
//     let store_path = ".db_test_handle_proposal";
//     let (tx_core, mut rx_network, mut _rx_network_smvba, _rx_commit) =
//         core(public_key, secret_key, store_path, pk_set).await;

//     // Send a block to the core.
//     let message = ConsensusMessage::Propose(block.clone());
//     tx_core.send(message).await.unwrap();

//     // Ensure we get a vote back.
//     match rx_network.recv().await {
//         Some((message, _)) => {
//             match message {
//                 ConsensusMessage::Vote(v) => assert_eq!(v, vote),
//                 _ => assert!(false),
//             }
//         }
//         _ => assert!(false),
//     }
// }

// #[tokio::test]
// async fn generate_proposal() {
//     // Get the keys of the leaders of this round and the next.
//     let (leader, leader_key) = leader_keys(1);
//     let (next_leader, next_leader_key) = leader_keys(2);

//     // Make a block, votes, and QC.
//     let block = Block::new_from_key(QC::genesis(), leader, 1, Vec::new(), &leader_key);
//     let hash = block.digest();
//     let votes: Vec<_> = keys()
//         .iter()
//         .map(|(public_key, secret_key)| {
//             Vote::new_from_key(
//                 hash.clone(),
//                 block.height,
//                 block.author,
//                 *public_key,
//                 &secret_key,
//             )
//         })
//         .collect();
//     let qc = QC {
//         hash,
//         epoch: 0,
//         height: block.height,
//         proposer: block.author,
//         votes: votes
//             .iter()
//             .cloned()
//             .map(|x| (x.author, x.signature))
//             .collect(),
//     };
//     let tss_keys = SecretShare::default();
//     let pk_set = tss_keys.pkset.clone();
//     // Run a core instance.
//     let store_path = ".db_test_generate_proposal";
//     let (tx_core, mut rx_network, mut _rx_network_smvba, _rx_commit) =
//         core(next_leader, next_leader_key, store_path, pk_set).await;

//     // Send all votes to the core.
//     for vote in votes.clone() {
//         let message = ConsensusMessage::Vote(vote);
//         tx_core.send(message).await.unwrap();
//     }

//     // Ensure the core sends a new block.
//     match rx_network.recv().await {
//         Some((message, _)) => {
//             match message {
//                 ConsensusMessage::Propose(b) => {
//                     assert_eq!(b.height, 2, "height");
//                     assert_eq!(b.qc, qc, "qc");
//                 }
//                 _ => assert!(false),
//             }
//         }
//         _ => assert!(false),
//     }
// }

// #[tokio::test]
// async fn commit_block() {
//     // Get enough distinct leaders to form a quorum.
//     let leaders = vec![leader_keys(1), leader_keys(2), leader_keys(3)];
//     let chain = chain(leaders);

//     let tss_keys = SecretShare::default();
//     let pk_set = tss_keys.pkset.clone();
//     // Run a core instance.
//     let store_path = ".db_test_commit_block";
//     let (public_key, secret_key) = keys().pop().unwrap();
//     let (tx_core, _rx_network, mut _rx_network_smvba, mut rx_commit) =
//         core(public_key, secret_key, store_path, pk_set).await;

//     // Send a the blocks to the core.
//     let committed = chain[0].clone();
//     for block in chain {
//         let message = ConsensusMessage::Propose(block);
//         tx_core.send(message).await.unwrap();
//     }

//     // Ensure the core commits the head.
//     match rx_commit.recv().await {
//         Some(b) => assert_eq!(b, committed, "commit"),
//         _ => assert!(false),
//     }
// }
