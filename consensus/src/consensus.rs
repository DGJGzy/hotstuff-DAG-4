use crate::config::{Committee, Parameters};
use crate::core::{ConsensusMessage, Core};
use crate::error::ConsensusResult;
use crate::filter::Filter;
use crate::leader::LeaderElector;
use crate::mempool::{ConsensusMempoolMessage, MempoolDriver};
use crate::messages::Block;
use crate::synchronizer::Synchronizer;
use crypto::{PublicKey, SignatureService};
use log::{info};
use network::{NetReceiver, NetSender};
use store::Store;
use threshold_crypto::PublicKeySet;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::time::{sleep, Duration};

#[cfg(test)]
#[path = "tests/consensus_tests.rs"]
pub mod consensus_tests;

pub struct Consensus;

impl Consensus {
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        name: PublicKey,
        committee: Committee,
        parameters: Parameters,
        store: Store,
        signature_service: SignatureService,
        pk_set: PublicKeySet, // The set of tss public keys
        tx_core: Sender<ConsensusMessage>,
        rx_core: Receiver<ConsensusMessage>,
        tx_smvba: Sender<ConsensusMessage>,
        rx_smvba: Receiver<ConsensusMessage>,
        tx_consensus_mempool: Sender<ConsensusMempoolMessage>,
        tx_commit: Sender<Block>,
    ) -> ConsensusResult<()> {
        info!(
            "Consensus timeout delay set to {} ms",
            parameters.timeout_delay
        );
        info!(
            "Consensus synchronizer retry delay set to {} ms",
            parameters.sync_retry_delay
        );
        info!(
            "Consensus max payload size set to {} B",
            parameters.max_payload_size
        );
        info!(
            "Consensus min block delay set to {} ms",
            parameters.min_block_delay
        );

        let (tx_network, rx_network) = channel(10000);
        let (tx_net_smvba, rx_net_smvba) = channel(10000);
        let (tx_filter, rx_filter) = channel(10000);
        let (tx_filter_smvba, rx_filter_smvba) = channel(10000);

        // Make the network sender and receiver.
        let address = committee.address(&name).map(|mut x| {
            x.set_ip("0.0.0.0".parse().unwrap());
            x
        })?;
        let network_receiver = NetReceiver::new(address, tx_core.clone());
        tokio::spawn(async move {
            network_receiver.run().await;
        });

        let smvba_address = committee.smvba_address(&name).map(|mut x| {
            x.set_ip("0.0.0.0".parse().unwrap());
            x
        })?;
        let smvba_receiver = NetReceiver::new(smvba_address, tx_smvba.clone());
        tokio::spawn(async move {
            smvba_receiver.run().await;
        });

        let mut network_sender = NetSender::new(rx_network);
        tokio::spawn(async move {
            network_sender.run().await;
        });

        let mut network_sender_smvba = NetSender::new(rx_net_smvba);
        tokio::spawn(async move {
            network_sender_smvba.run().await;
        });

        // The leader elector algorithm.
        let leader_elector = LeaderElector::new(committee.clone());

        // Make the mempool driver which will mediate our requests to the mempool.
        let mempool_driver = MempoolDriver::new(tx_consensus_mempool);

        // Custom filter to arbitrary delay network messages.
        Filter::run(
            rx_filter,
            rx_filter_smvba,
            tx_network,
            tx_net_smvba,
            parameters.clone(),
            leader_elector.clone(),
        ); //对消息进行延迟

        // Make the synchronizer. This instance runs in a background thread
        // and asks other nodes for any block that we may be missing.
        let synchronizer = Synchronizer::new(
            name,
            committee.clone(),
            store.clone(),
            /* network_filter */ tx_filter.clone(),
            /* core_channel */ tx_core.clone(),
            parameters.sync_retry_delay,
        )
        .await;
        sleep(Duration::from_millis(parameters.node_sync_delay)).await;

        tokio::spawn(async move {
            Core::new(
                name,
                committee,
                parameters,
                signature_service,
                pk_set,
                store,
                leader_elector,
                mempool_driver,
                synchronizer,
                /* core_channel */ rx_core,
                rx_smvba,
                /* network_filter */ tx_filter,
                tx_filter_smvba,
                /* commit_channel */ tx_commit,
            ).run().await;
        });

        Ok(())
    }
}
