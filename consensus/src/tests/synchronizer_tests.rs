use super::*;
use crate::common::{block, chain, committee, keys};
use std::fs;

#[tokio::test]
async fn get_existing_parent_block() {
    let mut chain = chain(keys());
    let block = chain.pop().unwrap();
    let b2 = chain.pop().unwrap();

    // Add the block b2 to the store.
    let path = ".db_test_get_existing_parent_block";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();
    let key = b2.digest().to_vec();
    let value = bincode::serialize(&b2).unwrap();
    let _ = store.write(key, value).await;

    // Make a new synchronizer.
    let (name, _) = keys().pop().unwrap();
    let (tx_network, _) = channel(10);
    let (tx_core, _) = channel(10);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store,
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask the predecessor of 'block' to the synchronizer.
    match synchronizer.get_parent_block(&block).await {
        Ok(Some(b)) => assert_eq!(b, b2),
        _ => assert!(false),
    }
}

#[tokio::test]
async fn get_genesis_parent_block() {
    // Make a new synchronizer.
    let path = ".db_test_get_genesis_parent_block";
    let _ = fs::remove_dir_all(path);
    let store = Store::new(path).unwrap();
    let (name, _) = keys().pop().unwrap();
    let (tx_network, _) = channel(1);
    let (tx_core, _) = channel(1);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store,
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask the predecessor of 'block' to the synchronizer.
    match synchronizer.get_parent_block(&block()).await {
        Ok(Some(b)) => assert_eq!(b, Block::genesis()),
        _ => assert!(false),
    }
}

#[tokio::test]
async fn get_missing_parent_block() {
    let mut chain = chain(keys());
    let block = chain.pop().unwrap();
    let parent_block = chain.pop().unwrap();

    // Make a new synchronizer.
    let path = ".db_test_get_missing_parent_block";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();
    let (name, _) = keys().pop().unwrap();
    let (tx_network, mut rx_network) = channel(1);
    let (tx_core, mut rx_core) = channel(1);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store.clone(),
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask for the parent of a block to the synchronizer.
    // The store does not have the parent yet.
    let copy = block.clone();
    let handle = tokio::spawn(async move {
        match synchronizer.get_parent_block(&copy).await {
            Ok(None) => assert!(true),
            _ => assert!(false),
        }
    });

    // Ensure the synchronizer sends a sync request
    // asking for the parent block.
    match rx_network.recv().await {
        Some((message, _)) => {
            match message {
                ConsensusMessage::SyncRequest(b, s) => {
                    assert_eq!(b, parent_block.digest());
                    assert_eq!(s, name);
                }
                _ => assert!(false),
            }
        }
        _ => assert!(false),
    }

    // Ensure the synchronizer returns None, thus suspending
    // the processing of the block.
    assert!(handle.await.is_ok());

    // Add the parent to the store.
    let key = parent_block.digest().to_vec();
    let value = bincode::serialize(&parent_block).unwrap();
    let _ = store.write(key, value).await;

    // Now that we have the parent, ensure the synchronizer
    // loops back the block to the core to resume processing.
    match rx_core.recv().await {
        Some(ConsensusMessage::LoopBack(b)) => assert_eq!(b, block.clone()),
        _ => assert!(false),
    }
}

#[tokio::test]
async fn get_existing_block() {
    let mut chain = chain(keys());
    let block = chain.pop().unwrap();

    // Add the block to the store.
    let path = ".db_test_get_existing_block";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();
    let key = block.digest().to_vec();
    let value = bincode::serialize(&block).unwrap();
    let _ = store.write(key, value).await;

    // Make a new synchronizer.
    let (name, _) = keys().pop().unwrap();
    let (tx_network, _) = channel(10);
    let (tx_core, _) = channel(10);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store,
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask for the block by digest from the synchronizer.
    match synchronizer.get_block(block.author, &block.digest(), None).await {
        Ok(Some(b)) => assert_eq!(b, block),
        _ => assert!(false),
    }
}

#[tokio::test]
async fn get_genesis_block() {
    // Make a new synchronizer.
    let path = ".db_test_get_genesis_block";
    let _ = fs::remove_dir_all(path);
    let store = Store::new(path).unwrap();
    let (name, _) = keys().pop().unwrap();
    let (tx_network, _) = channel(1);
    let (tx_core, _) = channel(1);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store,
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask for genesis block by digest.
    let genesis_digest = QC::genesis().hash;
    match synchronizer.get_block(name, &genesis_digest, None).await {
        Ok(Some(b)) => assert_eq!(b, Block::genesis()),
        _ => assert!(false),
    }
}

#[tokio::test]
async fn get_missing_block() {
    let mut chain = chain(keys());
    let block = chain.pop().unwrap();

    // Make a new synchronizer.
    let path = ".db_test_get_missing_block";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();
    let (name, _) = keys().pop().unwrap();
    let (tx_network, mut rx_network) = channel(1);
    let (tx_core, _) = channel(1);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store.clone(),
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask for a block that doesn't exist in the store.
    let copy_block = block.clone();
    let copy_digest = block.digest();
    let handle = tokio::spawn(async move {
        match synchronizer.get_block(copy_block.author, &copy_digest, None).await {
            Ok(None) => assert!(true),
            _ => assert!(false),
        }
    });

    // Ensure the synchronizer sends a sync request
    // asking for the missing block.
    match rx_network.recv().await {
        Some((message, _)) => {
            match message {
                ConsensusMessage::SyncRequest(b, s) => {
                    assert_eq!(b, block.digest());
                    assert_eq!(s, name);
                }
                _ => assert!(false),
            }
        }
        _ => assert!(false),
    }

    // Ensure the synchronizer returns None, indicating
    // the block is not available yet.
    assert!(handle.await.is_ok());

    // Add the block to the store.
    let key = block.digest().to_vec();
    let value = bincode::serialize(&block).unwrap();
    let _ = store.write(key, value).await;

    // Since we didn't pass a 'block' parameter, there should be no loopback message.
    // The synchronizer should just wait for the block to be available.
}

#[tokio::test]
async fn get_missing_block_with_loopback() {
    let mut chain = chain(keys());
    let block = chain.pop().unwrap();
    let requesting_block = chain.pop().unwrap(); // Some other block that triggered this request

    // Make a new synchronizer.
    let path = ".db_test_get_missing_block_with_loopback";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();
    let (name, _) = keys().pop().unwrap();
    let (tx_network, mut rx_network) = channel(1);
    let (tx_core, mut rx_core) = channel(1);
    let mut synchronizer = Synchronizer::new(
        name,
        committee(),
        store.clone(),
        tx_network,
        tx_core,
        /* sync_retry_delay */ 10_000,
    )
    .await;

    // Ask for a block that doesn't exist in the store, with a requesting block.
    let copy_block = block.clone();
    let copy_digest = block.digest();
    let copy_requesting = requesting_block.clone();
    let handle = tokio::spawn(async move {
        match synchronizer.get_block(copy_block.author, &copy_digest, Some(copy_requesting)).await {
            Ok(None) => assert!(true),
            _ => assert!(false),
        }
    });

    // Ensure the synchronizer sends a sync request.
    match rx_network.recv().await {
        Some((message, _)) => {
            match message {
                ConsensusMessage::SyncRequest(b, s) => {
                    assert_eq!(b, block.digest());
                    assert_eq!(s, name);
                }
                _ => assert!(false),
            }
        }
        _ => assert!(false),
    }

    // Ensure the synchronizer returns None.
    assert!(handle.await.is_ok());

    // Add the requested block to the store.
    let key = block.digest().to_vec();
    let value = bincode::serialize(&block).unwrap();
    let _ = store.write(key, value).await;

    // Now that we have the block, ensure the synchronizer
    // loops back the requesting block to the core for processing.
    match rx_core.recv().await {
        Some(ConsensusMessage::LoopBack(b)) => assert_eq!(b, requesting_block),
        _ => assert!(false),
    }
}