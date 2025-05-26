use super::*;
use crate::common::{aba_val, committee, keys, qc, timeout, vote};
use crypto::Hash;

#[test]
fn add_vote() {
    let mut aggregator = Aggregator::new(committee());
    let result = aggregator.add_vote(vote());
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn make_qc() {
    let mut aggregator = Aggregator::new(committee());
    let mut keys = keys();
    let qc = qc();
    let hash = qc.digest();
    let height = qc.height;
    let proposer = qc.proposer;

    // Add 2f+1 votes to the aggregator and ensure it returns the cryptographic
    // material to make a valid QC.
    let (public_key, secret_key) = keys.pop().unwrap();
    let vote = Vote::new_from_key(hash.clone(), height, proposer, public_key, &secret_key);
    let result = aggregator.add_vote(vote);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let (public_key, secret_key) = keys.pop().unwrap();
    let vote = Vote::new_from_key(hash.clone(), height, proposer, public_key, &secret_key);
    let result = aggregator.add_vote(vote);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let (public_key, secret_key) = keys.pop().unwrap();
    let vote = Vote::new_from_key(hash.clone(), height, proposer, public_key, &secret_key);
    match aggregator.add_vote(vote) {
        Ok(Some(qc)) => assert!(qc.verify(&committee()).is_ok()),
        _ => assert!(false),
    }
}

#[test]
fn duplicate_vote_error() {
    let mut aggregator = Aggregator::new(committee());
    let vote = vote();
    
    // First vote should succeed
    let result = aggregator.add_vote(vote.clone());
    assert!(result.is_ok());
    
    // Duplicate vote should return error
    let result = aggregator.add_vote(vote);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConsensusError::AuthorityReuseinQC(_)));
}

#[test]
fn add_timeout() {
    let mut aggregator = Aggregator::new(committee());
    let result = aggregator.add_timeout(timeout());
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn make_tc() {
    let mut aggregator = Aggregator::new(committee());
    let mut keys = keys();
    let timeout_template = timeout();
    let epoch = timeout_template.epoch;

    // Add 2f+1 timeouts to the aggregator and ensure it returns a valid TC
    let (public_key, secret_key) = keys.pop().unwrap();
    let timeout = Timeout::new_from_key(epoch, public_key, &secret_key, timeout_template.high_qc.clone());
    let result = aggregator.add_timeout(timeout);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let (public_key, secret_key) = keys.pop().unwrap();
    let timeout = Timeout::new_from_key(epoch, public_key, &secret_key, timeout_template.high_qc.clone());
    let result = aggregator.add_timeout(timeout);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let (public_key, secret_key) = keys.pop().unwrap();
    let timeout = Timeout::new_from_key(epoch, public_key, &secret_key, timeout_template.high_qc.clone());
    match aggregator.add_timeout(timeout) {
        Ok(Some(tc)) => {
            assert_eq!(tc.epoch, epoch);
            assert_eq!(tc.votes.len(), 3);
        },
        _ => assert!(false),
    }
}

#[test]
fn duplicate_timeout_error() {
    let mut aggregator = Aggregator::new(committee());
    let timeout = timeout();
    
    // First timeout should succeed
    let result = aggregator.add_timeout(timeout.clone());
    assert!(result.is_ok());
    
    // Duplicate timeout should return error
    let result = aggregator.add_timeout(timeout);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConsensusError::AuthorityReuseinTC(_)));
}

#[test]
fn add_aba_val() {
    let mut aggregator = Aggregator::new(committee());
    let result = aggregator.add_aba_val(aba_val());
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn make_aba_proof() {
    let mut aggregator = Aggregator::new(committee());
    let mut keys = keys();
    let aba_template = aba_val();
    let epoch = aba_template.epoch;
    let round = aba_template.round;
    let phase = aba_template.phase;
    let val = aba_template.val;

    // Add 2f+1 ABA vals to get a proof
    let (public_key, secret_key) = keys.pop().unwrap();
    let aba_val = ABAVal::new_from_key(epoch, round, phase, val, public_key, &secret_key);
    let result = aggregator.add_aba_val(aba_val);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let (public_key, secret_key) = keys.pop().unwrap();
    let aba_val = ABAVal::new_from_key(epoch, round, phase, val, public_key, &secret_key);
    let result = aggregator.add_aba_val(aba_val);
    assert!(result.is_ok());
    assert!(result.unwrap().is_some());

    let (public_key, secret_key) = keys.pop().unwrap();
    let aba_val = ABAVal::new_from_key(epoch, round, phase, val, public_key, &secret_key);
    match aggregator.add_aba_val(aba_val) {
        Ok(Some(proof)) => {
            assert_eq!(proof.epoch, epoch);
            assert_eq!(proof.round, round);
            assert_eq!(proof.val, val);
            assert_eq!(proof.phase, phase);
            assert_eq!(proof.votes.len(), 3);
            assert!(!proof.tag); // Should be false for quorum threshold
        },
        _ => assert!(false),
    }
}

#[test]
fn make_aba_proof_with_tag() {
    let mut aggregator = Aggregator::new(committee());
    let mut keys = keys();
    let epoch = 1;
    let round = 1;
    let phase = VAL_PHASE; // Important: using VAL_PHASE to trigger tag logic
    let val = 1;

    // Add exactly random_coin_threshold votes to trigger tag=true
    let committee = committee();
    let threshold = committee.random_coin_threshold();
    let mut added_weight = 0;
    
    for _ in 0..keys.len() {
        let (public_key, secret_key) = keys.pop().unwrap();
        let aba_val = ABAVal::new_from_key(epoch, round, phase, val, public_key, &secret_key);
        added_weight += committee.stake(&public_key);
        
        let result = aggregator.add_aba_val(aba_val);
        assert!(result.is_ok());
        
        if added_weight == threshold {
            // Should get proof with tag=true
            match result.unwrap() {
                Some(proof) => {
                    assert!(proof.tag);
                    break;
                },
                None => continue,
            }
        } else if added_weight < threshold {
            assert!(result.unwrap().is_none());
        }
    }
}

#[test]
fn duplicate_aba_val_error() {
    let mut aggregator = Aggregator::new(committee());
    let aba_val = aba_val();
    
    // First ABA val should succeed
    let result = aggregator.add_aba_val(aba_val.clone());
    assert!(result.is_ok());
    
    // Duplicate ABA val should return error
    let result = aggregator.add_aba_val(aba_val);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConsensusError::AuthorityReuseinABA(_, _)));
}

// #[test]
// fn add_aba_random() {
//     let mut aggregator = Aggregator::new(committee());
//     let share = randomness_share();
//     let tss_keys = SecretShare::default();
//     let pk_set = tss_keys.pkset.clone();
    
//     let result = aggregator.add_aba_random(share, &pk_set);
//     assert!(result.is_ok());
//     assert!(result.unwrap().is_none());
// }

// #[test]
// fn make_random_coin() {
//     let mut aggregator = Aggregator::new(committee());
//     let mut keys = keys();
//     let epoch = 1;
//     let round = 1;
    
//     // Create threshold crypto setup
//     let mut rng = rand::thread_rng();
//     let sk_set = threshold_crypto::SecretKeySet::random(2, &mut rng); // threshold = 2
//     let pk_set = sk_set.public_keys();
    
//     let committee = committee();
//     let threshold = committee.random_coin_threshold();
//     let mut added_weight = 0;
    
//     // Add random shares until we reach threshold
//     for i in 0..keys.len() {
//         let (public_key, _) = &keys[i];
//         let sk_share = sk_set.secret_key_share(committee.id(public_key.clone()));
//         let sig_share = sk_share.sign(&format!("{}:{}", epoch, round));
        
//         let share = RandomnessShare {
//             epoch,
//             round,
//             author: public_key.clone(),
//             signature_share: sig_share,
//         };
        
//         added_weight += committee.stake(public_key);
//         let result = aggregator.add_aba_random(share, &pk_set);
//         assert!(result.is_ok());
        
//         if added_weight == threshold {
//             // Should get random coin result
//             assert!(result.unwrap().is_some());
//             break;
//         } else {
//             assert!(result.unwrap().is_none());
//         }
//     }
// }

// #[test]
// fn duplicate_random_share_error() {
//     let mut aggregator = Aggregator::new(committee());
//     let share = randomness_share();
//     let pk_set = threshold_crypto::PublicKeySet::from(threshold_crypto::SecretKeySet::random(2, &mut rand::thread_rng()));
    
//     // First share should succeed
//     let result = aggregator.add_aba_random(share.clone(), &pk_set);
//     assert!(result.is_ok());
    
//     // Duplicate share should return error
//     let result = aggregator.add_aba_random(share, &pk_set);
//     assert!(result.is_err());
//     assert!(matches!(result.unwrap_err(), ConsensusError::AuthorityReuseinCoin(_)));
// }

#[test]
fn cleanup_votes_and_timeouts() {
    let mut aggregator = Aggregator::new(committee());
    
    // Add votes and timeouts with different heights/epochs
    let vote1 = Vote::new_with_height(1);
    let vote2 = Vote::new_with_height(5);
    let timeout1 = Timeout::new_with_epoch(1);
    let timeout2 = Timeout::new_with_epoch(5);
    
    aggregator.add_vote(vote1).ok();
    aggregator.add_vote(vote2).ok();
    aggregator.add_timeout(timeout1).ok();
    aggregator.add_timeout(timeout2).ok();
    
    assert_eq!(aggregator.votes_aggregators.len(), 2);
    assert_eq!(aggregator.timeouts_aggregators.len(), 2);
    
    // Cleanup with epoch=3, height=3 should remove items with lower values
    aggregator.cleanup(&3, &3);
    
    assert_eq!(aggregator.votes_aggregators.len(), 1);
    assert!(aggregator.votes_aggregators.contains_key(&5));
    assert_eq!(aggregator.timeouts_aggregators.len(), 1);
    assert!(aggregator.timeouts_aggregators.contains_key(&5));
}

#[test]
fn cleanup_aba() {
    let mut aggregator = Aggregator::new(committee());
    
    // Add ABA vals and random shares with different epochs
    let aba_val1 = ABAVal::new_with_epoch(1);
    let aba_val2 = ABAVal::new_with_epoch(5);
    // let share1 = RandomnessShare::new_with_epoch(1);
    // let share2 = RandomnessShare::new_with_epoch(5);
    // let pk_set = threshold_crypto::PublicKeySet::from(threshold_crypto::SecretKeySet::random(2, &mut rand::thread_rng()));
    
    aggregator.add_aba_val(aba_val1).ok();
    aggregator.add_aba_val(aba_val2).ok();
    // aggregator.add_aba_random(share1, &pk_set).ok();
    // aggregator.add_aba_random(share2, &pk_set).ok();
    
    assert_eq!(aggregator.aba_vals_aggregators.len(), 2);
    // assert_eq!(aggregator.aba_randomcoin_aggregators.len(), 2);
    
    // Cleanup with epoch=3 should remove items with lower epoch values
    aggregator.cleanup_aba(&3);
    
    assert_eq!(aggregator.aba_vals_aggregators.len(), 1);
    // assert_eq!(aggregator.aba_randomcoin_aggregators.len(), 1);
}

#[test]
fn qc_maker_weight_accumulation() {
    let committee = committee();
    let mut qc_maker = QCMaker::new();
    let vote = vote();
    
    // Initial weight should be 0
    assert_eq!(qc_maker.weight, 0);
    
    // After adding one vote, weight should increase
    let result = qc_maker.append(vote.clone(), &committee);
    assert!(result.is_ok());
    assert!(qc_maker.weight > 0);
    assert_eq!(qc_maker.votes.len(), 1);
    assert_eq!(qc_maker.used.len(), 1);
}

#[test]
fn tc_maker_weight_accumulation() {
    let committee = committee();
    let mut tc_maker = TCMaker::new();
    let timeout = timeout();
    
    // Initial weight should be 0
    assert_eq!(tc_maker.weight, 0);
    
    // After adding one timeout, weight should increase
    let result = tc_maker.append(timeout.clone(), &committee);
    assert!(result.is_ok());
    assert!(tc_maker.weight > 0);
    assert_eq!(tc_maker.votes.len(), 1);
    assert_eq!(tc_maker.used.len(), 1);
}

#[test]
fn aba_proof_maker_weight_accumulation() {
    let committee = committee();
    let mut proof_maker = ABAProofMaker::new();
    let aba_val = aba_val();
    
    // Initial weight should be 0
    assert_eq!(proof_maker.weight, 0);
    
    // After adding one ABA val, weight should increase
    let result = proof_maker.append(aba_val.clone(), &committee);
    assert!(result.is_ok());
    assert!(proof_maker.weight > 0);
    assert_eq!(proof_maker.votes.len(), 1);
    assert_eq!(proof_maker.used.len(), 1);
}

// #[test]
// fn random_coin_maker_weight_accumulation() {
//     let committee = committee();
//     let mut coin_maker = ABARandomCoinMaker::new();
//     let share = randomness_share();
//     let pk_set = threshold_crypto::PublicKeySet::from(threshold_crypto::SecretKeySet::random(2, &mut rand::thread_rng()));
    
//     // Initial weight should be 0
//     assert_eq!(coin_maker.weight, 0);
    
//     // After adding one share, weight should increase
//     let result = coin_maker.append(share.clone(), &committee, &pk_set);
//     assert!(result.is_ok());
//     assert!(coin_maker.weight > 0);
//     assert_eq!(coin_maker.shares.len(), 1);
//     assert_eq!(coin_maker.used.len(), 1);
// }

// Test to ensure weight is reset after successful aggregation
#[test]
fn weight_reset_after_qc_creation() {
    let mut aggregator = Aggregator::new(committee());
    let mut keys = keys();
    let qc = qc();
    let hash = qc.digest();
    let height = qc.height;
    let proposer = qc.proposer;

    // Add enough votes to create QC
    for _ in 0..3 {
        let (public_key, secret_key) = keys.pop().unwrap();
        let vote = Vote::new_from_key(hash.clone(), height, proposer, public_key, &secret_key);
        aggregator.add_vote(vote).ok();
    }

    // Check that the QC maker's weight is reset to 0 after QC creation
    if let Some(qc_maker) = aggregator.votes_aggregators.get(&height) {
        assert_eq!(qc_maker.weight, 0);
    }
}