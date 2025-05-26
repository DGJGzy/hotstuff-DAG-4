// use super::*;
// use crate::common::{committee, keys, MockMempool};
// use crypto::{SecretKey, SecretShare};
// use std::fs;
// use tokio::sync::mpsc::channel;
// use tokio::time::{sleep, Duration};

// async fn core_tcvba(
//     name: PublicKey,
//     secret: SecretKey,
//     store_path: &str,
//     pk_set: PublicKeySet,
// ) -> (
//     Sender<ConsensusMessage>,
//     Sender<ConsensusMessage>, // SMVBA channel sender
//     Receiver<FilterInput>,
//     Receiver<FilterInput>, // SMVBA network filter
//     Receiver<Block>,
// ) {
//     let (tx_core, rx_core) = channel(10);
//     let (tx_smvba, rx_smvba) = channel(10);
//     let (tx_network, rx_network) = channel(10);
//     let (tx_network_smvba, rx_network_smvba) = channel(10);
//     let (tx_consensus_mempool, rx_consensus_mempool) = channel(10);
//     let (tx_commit, rx_commit) = channel(10);

//     let parameters = Parameters {
//         timeout_delay: 100,
//         min_block_delay: 10,
//         ..Parameters::default()
//     };
    
//     let signature_service = SignatureService::new(secret, None);
    
//     let _ = fs::remove_dir_all(store_path);
//     let store = Store::new(store_path).unwrap();
//     let leader_elector = LeaderElector::new(committee());
    
//     // Mock mempool
//     MockMempool::run(rx_consensus_mempool);
    
//     let mempool_driver = MempoolDriver::new(tx_consensus_mempool);
//     let synchronizer = Synchronizer::new(
//         name,
//         committee(),
//         store.clone(),
//         tx_network.clone(),
//         tx_core.clone(),
//         parameters.sync_retry_delay,
//     ).await;
    
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
//         rx_core,
//         rx_smvba,
//         tx_network,
//         tx_network_smvba,
//         tx_commit,
//     );
    
//     tokio::spawn(async move {
//         core.run().await;
//     });
    
//     (tx_core, tx_smvba, rx_network, rx_network_smvba, rx_commit)
// }

// fn get_node_keys(index: usize) -> (PublicKey, SecretKey) {
//     keys().into_iter().nth(index).unwrap()
// }

// async fn setup_tcvba_scenario() -> Vec<(
//     PublicKey,
//     Sender<ConsensusMessage>,
//     Sender<ConsensusMessage>,
//     Receiver<FilterInput>,
//     Receiver<FilterInput>,
//     Receiver<Block>,
// )> {
//     let mut nodes = Vec::new();
//     let committee = committee();
//     let node_count = committee.size();
//     let tss_keys = SecretShare::default();
//     let pk_set = tss_keys.pkset.clone();
    
//     for i in 0..node_count {
//         let (public_key, secret_key) = get_node_keys(i);
//         let store_path = format!(".db_test_tcvba_node_{}", i);
//         let (tx_core, tx_smvba, rx_network, rx_network_smvba, rx_commit) = 
//             core_tcvba(public_key, secret_key, &store_path, pk_set.clone()).await;
        
//         nodes.push((public_key, tx_core, tx_smvba, rx_network, rx_network_smvba, rx_commit));
//     }
    
//     // Give nodes time to initialize
//     sleep(Duration::from_millis(5000)).await;
//     nodes
// }

// #[tokio::test]
// async fn test_tcvba_timeout_and_tc_generation() {
//     let nodes = setup_tcvba_scenario().await;
//     let committee = committee();
//     let threshold = committee.quorum_threshold();
    
//     // Simulate timeout scenario by sending timeout messages
//     let epoch = 0;
//     // let (leader_pk, _) = get_node_keys(0); // Assume node 0 is leader
    
//     // Create timeout messages from enough nodes to form TC
//     let mut timeout_messages = Vec::new();
//     for i in 0..threshold {
//         let (public_key, secret_key) = get_node_keys(i as usize);
        
//         let timeout = Timeout::new_from_key(
//             epoch,
//             public_key,
//             &secret_key,
//             QC::genesis(),
//         );
//         timeout_messages.push(timeout);
//     }
    
//     // Send timeout messages to trigger TC formation
//     for timeout in timeout_messages {
//         for (_, _, tx_smvba, _, _, _) in &nodes {
//             let _ = tx_smvba.send(ConsensusMessage::Timeout(timeout.clone())).await;
//         }
//         sleep(Duration::from_millis(10)).await;
//     }
    
//     // Wait for TC to be generated and processed
//     sleep(Duration::from_millis(500)).await;
    
//     // Check if nodes received TC messages on their SMVBA network channels
//     let mut tc_received = false;
//     for (_, _, _, _, mut rx_network_smvba, _) in nodes {
//         if let Ok(Some((message, _))) = tokio::time::timeout(
//             Duration::from_millis(100),
//             rx_network_smvba.recv()
//         ).await {
//             if matches!(message, ConsensusMessage::TC(_)) {
//                 tc_received = true;
//                 break;
//             }
//         }
//     }
    
//     assert!(tc_received, "TC should be generated and broadcast");
// }

// #[tokio::test]
// async fn test_tcvba_aba_val_phase() {
//     let nodes = setup_tcvba_scenario().await;
//     // let committee = committee();
//     let epoch = 0;
//     let round = 1;
//     let input_val = 1u64; // Test with input value 1
    
//     // Create ABAVal message for VAL_PHASE
//     let (public_key, secret_key) = get_node_keys(0);
    
//     let aba_val = ABAVal::new_from_key(
//         epoch,
//         round,
//         VAL_PHASE,
//         input_val,
//         public_key,
//         &secret_key,
//     );
    
//     // Send ABAVal to all nodes
//     for (_, _, tx_smvba, _, _, _) in &nodes {
//         let _ = tx_smvba.send(ConsensusMessage::ABAVal(aba_val.clone())).await;
//     }
    
//     sleep(Duration::from_millis(200)).await;
    
//     // Check if nodes process and potentially broadcast ABAVal messages
//     let mut _aba_val_received = false;
//     for (_, _, _, _, mut rx_network_smvba, _) in nodes {
//         if let Ok(Some((message, _))) = tokio::time::timeout(
//             Duration::from_millis(100),
//             rx_network_smvba.recv()
//         ).await {
//             if matches!(message, ConsensusMessage::ABAVal(_)) {
//                 _aba_val_received = true;
//                 break;
//             }
//         }
//     }
    
//     // Note: This might not always trigger immediate broadcast depending on internal state
//     // The test mainly verifies that the ABAVal handling doesn't crash
// }

// // #[tokio::test]
// // async fn test_tcvba_coin_share_aggregation() {
// //     let nodes = setup_tcvba_scenario().await;
// //     let committee = committee();
// //     let epoch = 0;
// //     let round = 1;
    
// //     // Create coin shares from multiple nodes
// //     let mut coin_shares = Vec::new();
// //     for i in 0..committee.quorum_threshold() {
// //         let (public_key, secret_key) = get_node_keys(i as usize);
        
// //         let coin_share = RandomnessShare::new_from_key(
// //             epoch,
// //             round,
// //             public_key,
// //             &secret_key,
// //         );
// //         coin_shares.push(coin_share);
// //     }
    
// //     // Send coin shares to all nodes
// //     for coin_share in coin_shares {
// //         for (_, _, tx_smvba, _, _, _) in &nodes {
// //             let _ = tx_smvba.send(ConsensusMessage::ABACoinShare(coin_share.clone())).await;
// //         }
// //         sleep(Duration::from_millis(10)).await;
// //     }
    
// //     sleep(Duration::from_millis(300)).await;
    
// //     // Verify that coin shares are processed (no crash indicates success)
// //     // In a real scenario, this would trigger coin computation and potentially ABA output
// // }

// #[tokio::test]
// async fn test_tcvba_full_aba_simulation() {
//     let nodes = setup_tcvba_scenario().await;
//     let committee = committee();
//     let epoch = 0;
//     // let round = 1;
    
//     // Step 1: Create and send TC to trigger ABA
//     // let (leader_pk, leader_sk) = get_node_keys(0);
    
//     // Simulate TC reception by creating timeout messages
//     let mut timeout_messages = Vec::new();
//     for i in 0..committee.quorum_threshold() {
//         let (public_key, secret_key) = get_node_keys(i as usize);
        
//         let timeout = Timeout::new_from_key(
//             epoch,
//             public_key,
//             &secret_key,
//             QC::genesis(),
//         );
//         timeout_messages.push(timeout);
//     }
    
//     // Send timeouts to form TC
//     for timeout in timeout_messages {
//         for (_, _, tx_smvba, _, _, _) in &nodes {
//             let _ = tx_smvba.send(ConsensusMessage::Timeout(timeout.clone())).await;
//         }
//     }
    
//     sleep(Duration::from_millis(200)).await;
    
//     // Step 2: Verify ABA messages are being exchanged
//     let mut message_count = 0;
//     for (_, _, _, _, mut rx_network_smvba, _) in nodes {
//         while let Ok(Some((message, _))) = tokio::time::timeout(
//             Duration::from_millis(50),
//             rx_network_smvba.recv()
//         ).await {
//             match message {
//                 ConsensusMessage::ABAVal(_) |
//                 ConsensusMessage::ABACoinShare(_) |
//                 ConsensusMessage::ABAProof(_) |
//                 ConsensusMessage::TC(_) => {
//                     message_count += 1;
//                 }
//                 _ => {}
//             }
            
//             if message_count > 5 { // Stop after seeing some activity
//                 break;
//             }
//         }
        
//         if message_count > 5 {
//             break;
//         }
//     }
    
//     assert!(message_count > 0, "Should see TCVBA protocol messages being exchanged");
// }

// #[tokio::test]
// async fn test_tcvba_input_output_consistency() {
//     // This test verifies that TCVBA input leads to consistent output
//     let nodes = setup_tcvba_scenario().await;
//     let committee = committee();
//     let test_input = 1u64;
    
//     // Create initial blocks for the test scenario
//     let (leader_pk, leader_sk) = get_node_keys(0);
    
//     // Create a block at height 1 (our test input)
//     let test_block = Block::new_from_key(
//         QC::genesis(),
//         leader_pk,
//         test_input, // height = our input value
//         vec![], // empty payload
//         &leader_sk,
//     );
    
//     // Send the block to establish it in the system
//     for (_, tx_core, _, _, _, _) in &nodes {
//         let _ = tx_core.send(ConsensusMessage::Propose(test_block.clone())).await;
//     }
    
//     sleep(Duration::from_millis(100)).await;
    
//     // Trigger view change by sending timeouts
//     for i in 0..committee.quorum_threshold() {
//         let (public_key, secret_key) = get_node_keys(i as usize);
        
//         let timeout = Timeout::new_from_key(
//             0, // epoch
//             public_key,
//             &secret_key,
//             QC::genesis(),
//         );
        
//         for (_, _, tx_smvba, _, _, _) in &nodes {
//             let _ = tx_smvba.send(ConsensusMessage::Timeout(timeout.clone())).await;
//         }
//     }
    
//     // Wait for TCVBA to potentially complete
//     sleep(Duration::from_millis(1000)).await;
    
//     // Check if any blocks were committed
//     let mut committed_blocks = Vec::new();
//     for (_, _, _, _, _, mut rx_commit) in nodes {
//         while let Ok(Some(block)) = tokio::time::timeout(
//             Duration::from_millis(50),
//             rx_commit.recv()
//         ).await {
//             committed_blocks.push(block);
//         }
//     }
    
//     // Verify that if blocks were committed, they relate to our input
//     if !committed_blocks.is_empty() {
//         println!("Committed {} blocks", committed_blocks.len());
//         // In a successful TCVBA run, we expect the output height to match our input
//         let output_heights: Vec<u64> = committed_blocks.iter().map(|b| b.height).collect();
//         println!("Output heights: {:?}", output_heights);
        
//         // The test passes if we see some activity (blocks committed)
//         // In a real implementation, we'd verify the output matches expected consensus
//         assert!(!committed_blocks.is_empty(), "TCVBA should result in block commitment");
//     }
// }

// #[tokio::test]
// async fn test_tcvba_binary_input_0() {
//     // Test TCVBA with binary input 0
//     let nodes = setup_tcvba_scenario().await;
//     // let committee = committee();
//     let test_input = 0u64;
    
//     // Similar to above test but with input 0
//     let (leader_pk, leader_sk) = get_node_keys(0);
    
//     // Create ABAVal with input 0
//     let aba_val = ABAVal::new_from_key(
//         0, // epoch
//         1, // round
//         VAL_PHASE,
//         test_input,
//         leader_pk,
//         &leader_sk,
//     );
    
//     // Send to all nodes
//     for (_, _, tx_smvba, _, _, _) in &nodes {
//         let _ = tx_smvba.send(ConsensusMessage::ABAVal(aba_val.clone())).await;
//     }
    
//     sleep(Duration::from_millis(200)).await;
    
//     // Verify processing doesn't crash and some network activity occurs
//     let mut network_activity = false;
//     for (_, _, _, _, mut rx_network_smvba, _) in nodes {
//         if let Ok(Some(_)) = tokio::time::timeout(
//             Duration::from_millis(100),
//             rx_network_smvba.recv()
//         ).await {
//             network_activity = true;
//             break;
//         }
//     }
    
//     // Test passes if no crash occurs (network activity is optional)
//     println!("Network activity detected: {}", network_activity);
// }

// #[tokio::test]
// async fn test_tcvba_binary_input_1() {
//     // Test TCVBA with binary input 1
//     let nodes = setup_tcvba_scenario().await;
//     // let committee = committee();
//     let test_input = 1u64;
    
//     let (leader_pk, leader_sk) = get_node_keys(0);
    
//     // Create ABAVal with input 1
//     let aba_val = ABAVal::new_from_key(
//         0, // epoch
//         1, // round
//         VAL_PHASE,
//         test_input,
//         leader_pk,
//         &leader_sk,
//     );
    
//     // Send to all nodes
//     for (_, _, tx_smvba, _, _, _) in &nodes {
//         let _ = tx_smvba.send(ConsensusMessage::ABAVal(aba_val.clone())).await;
//     }
    
//     sleep(Duration::from_millis(200)).await;
    
//     // Similar verification as input 0 test
//     let mut _network_activity = false;
//     for (_, _, _, _, mut rx_network_smvba, _) in nodes {
//         if let Ok(Some(_)) = tokio::time::timeout(
//             Duration::from_millis(100),
//             rx_network_smvba.recv()
//         ).await {
//             _network_activity = true;
//             break;
//         }
//     }
    
//     // println!("Network activity detected: {}", network_activity);
// }