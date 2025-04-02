#![allow(clippy::disallowed_types)]
use crate::common::local_replica::{self, icrc_ledger_wasm};
use crate::common::local_replica::{create_and_install_icrc_ledger, test_identity, create_and_install_custom_icrc_ledger};
use ic_agent::Identity;
use candid::{Encode, Nat};
use ic_icrc1_ledger::{LedgerArgument};
use ic_base_types::PrincipalId;
use ic_icrc1_ledger::InitArgsBuilder;
use ic_icrc1_test_utils::{transfer_args_with_sender, DEFAULT_TRANSFER_FEE};
use ic_icrc_rosetta::common::storage::storage_client::StorageClient;
use ic_icrc_rosetta::ledger_blocks_synchronization::blocks_synchronizer::{self, blocks_verifier, RecurrencyMode};
use ic_ledger_canister_core::archive::ArchiveOptions;
use icrc_ledger_agent::Icrc1Agent;
use icrc_ledger_types::icrc1::account::Account;
use lazy_static::lazy_static;
use pocket_ic::PocketIcBuilder;
use proptest::prelude::*;
use std::sync::Arc;
use rusqlite::{Connection, OpenFlags};
use icrc_ledger_types::icrc1::transfer::TransferArg;
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;
use crate::integration_test_components::blocks_synchronizer::fetching_blocks_interval_test::local_replica::icrc_ledger_old_certificate_wasm;

lazy_static! {
    pub static ref TEST_ACCOUNT: Account = test_identity().sender().unwrap().into();
    pub static ref MAX_NUM_GENERATED_BLOCKS: usize = 20;
    pub static ref NUM_TEST_CASES: u32 = 2;
}

fn check_storage_validity(storage_client: Arc<StorageClient>, highest_index: u64) {
    // Get the tip of the blockchain from the storage client
    let tip_block = storage_client.get_block_with_highest_block_idx().unwrap();

    // Get the genesis block from the blockchain
    let genesis_block = storage_client.get_block_with_lowest_block_idx().unwrap();

    // Get the the entire blockchain
    let blocks_stored = storage_client
        .get_blocks_by_index_range(0, highest_index)
        .unwrap();

    // The index of the tip of the chain should be the number of generated blocks
    assert_eq!(tip_block.unwrap().index, highest_index.clone());

    // The index of the genesis block should be 0
    assert_eq!(genesis_block.unwrap().index, 0);

    // The number of stored blocks should be the number of generated blocks generated in total plus the genesis block
    assert_eq!(blocks_stored.len() as u64, highest_index + 1);

    // Make sure the blocks that are stored are valid
    assert!(blocks_verifier::is_valid_blockchain(
        &blocks_stored,
        &blocks_stored.last().unwrap().clone().get_block_hash()
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(*NUM_TEST_CASES))]
    #[test]
    fn test_simple_start_of_synchronizing_blocks(transfer_args_batch1 in transfer_args_with_sender(*MAX_NUM_GENERATED_BLOCKS, *TEST_ACCOUNT),transfer_args_batch2 in transfer_args_with_sender(*MAX_NUM_GENERATED_BLOCKS, *TEST_ACCOUNT)) {
        // Create a tokio environment to conduct async calls
        let rt = Runtime::new().unwrap();
        let mut pocket_ic = PocketIcBuilder::new().with_nns_subnet().with_sns_subnet().build();
        let init_args = InitArgsBuilder::for_tests()
            .with_minting_account(*TEST_ACCOUNT)
            .with_initial_balance(*TEST_ACCOUNT, 1_000_000_000_000u64)
            .with_transfer_fee(DEFAULT_TRANSFER_FEE)
            .with_archive_options(ArchiveOptions {
                trigger_threshold: 10_000,
                num_blocks_to_archive: 10_000,
                node_max_memory_size_bytes: None,
                max_message_size_bytes: None,
                controller_id: PrincipalId::new_user_test_id(100),
                more_controller_ids: None,
                cycles_for_archive_creation: None,
                max_transactions_per_response: None,
            })
            .build();
        let icrc_ledger_canister_id = create_and_install_icrc_ledger(&pocket_ic, init_args, None);
        let endpoint = pocket_ic.make_live(None);
        let port = endpoint.port().unwrap();

        // Wrap async calls in a blocking Block
        rt.block_on(async {
            // Create a testing agent
            let agent = Arc::new(Icrc1Agent {
                agent: local_replica::get_testing_agent(port).await,
                ledger_canister_id: icrc_ledger_canister_id,
            });

            // Create some blocks to be fetched later
            for transfer_arg in transfer_args_batch1.iter() {
                agent.transfer(transfer_arg.clone()).await.unwrap().unwrap();
            }

            // Create the storage client where blocks will be stored
            let storage_client = Arc::new(StorageClient::new_in_memory().unwrap());

            // Start the synching process
            // Conduct a full sync from the tip of the blockchain to genesis block
            blocks_synchronizer::start_synching_blocks(agent.clone(), storage_client.clone(),2,Arc::new(AsyncMutex::new(vec![])), RecurrencyMode::OneShot, Box::new(|| {})).await.unwrap();

            // Check that the full sync of all blocks generated by the first batch of blocks is valid
            check_storage_validity(storage_client.clone(),transfer_args_batch1.len() as u64);

            // Create some more blocks to be fetched later
            for transfer_arg in transfer_args_batch2.iter() {
                agent.transfer(transfer_arg.clone()).await.unwrap().unwrap();
            }

            // Sync between the tip of the chain and the stored blocks
            // The blocksynchronizer now sync the blocks between the current tip of the chain and the most recently stored block
            blocks_synchronizer::sync_from_the_tip(agent.clone(), storage_client.clone(),2,Arc::new(AsyncMutex::new(vec![]))).await.unwrap();

            // Check that the sync of all blocks generated by the second batch of blocks is valid
            check_storage_validity(storage_client.clone(),(transfer_args_batch1.len()+transfer_args_batch2.len()) as u64);

            // If we do another synchronization where there are no new blocks the synchronizer should be able to handle that
            blocks_synchronizer::start_synching_blocks(agent.clone(), storage_client.clone(),2,Arc::new(AsyncMutex::new(vec![])), RecurrencyMode::OneShot, Box::new(|| {})).await.unwrap();

            // Storage should still be valid
            check_storage_validity(storage_client.clone(),(transfer_args_batch1.len()+transfer_args_batch2.len()) as u64);
        });
    }

    #[test]
    fn test_fetching_from_archive(transfer_args in transfer_args_with_sender(*MAX_NUM_GENERATED_BLOCKS, *TEST_ACCOUNT)) {
        // Create a tokio environment to conduct async calls
        let rt = Runtime::new().unwrap();
        let mut pocket_ic = PocketIcBuilder::new().with_nns_subnet().with_sns_subnet().build();
        let init_args = InitArgsBuilder::for_tests()
            .with_minting_account(*TEST_ACCOUNT)
            .with_initial_balance(*TEST_ACCOUNT, 1_000_000_000_000u64)
            .with_transfer_fee(DEFAULT_TRANSFER_FEE)
            .with_archive_options(ArchiveOptions {
                // Create archive after every ten blocks
                trigger_threshold: 10,
                num_blocks_to_archive: 5,
                node_max_memory_size_bytes: None,
                max_message_size_bytes: None,
                controller_id: PrincipalId::new_user_test_id(100),
                more_controller_ids: None,
                cycles_for_archive_creation: None,
                max_transactions_per_response: None,
            })
            .build();
        let icrc_ledger_canister_id = create_and_install_icrc_ledger(&pocket_ic, init_args, None);
        let endpoint = pocket_ic.make_live(None);
        let port = endpoint.port().unwrap();

        // Wrap async calls in a blocking Block
        rt.block_on(async {
            // Create a testing agent
            let agent = Arc::new(Icrc1Agent {
                agent: local_replica::get_testing_agent(port).await,
                ledger_canister_id: icrc_ledger_canister_id,
            });


            // Create some blocks to be fetched later
            // An archive is created after 10 blocks
            for transfer_arg in transfer_args.iter() {
                agent.transfer(transfer_arg.clone()).await.unwrap().unwrap();
            }

            // Create the storage client where blocks will be stored
            let storage_client = Arc::new(StorageClient::new_in_memory().unwrap());

            // Start the synching process
            // Conduct a full sync from the tip of the blockchain to genesis block
            // Fetched blocks from the ledger and the archive
            blocks_synchronizer::start_synching_blocks(agent.clone(), storage_client.clone(),10,Arc::new(AsyncMutex::new(vec![])), RecurrencyMode::OneShot, Box::new(|| {})).await.unwrap();

            // Check that the full sync of all blocks generated is valid
            check_storage_validity(storage_client.clone(),transfer_args.len() as u64);

        });
    }

    #[test]
    fn test_icrc3_certificate(transfer_args in transfer_args_with_sender(*MAX_NUM_GENERATED_BLOCKS, *TEST_ACCOUNT).no_shrink()) {
        // Create a tokio environment to conduct async calls
        let rt = Runtime::new().unwrap();
        let mut pocket_ic = PocketIcBuilder::new().with_nns_subnet().with_sns_subnet().build();
        let init_args = InitArgsBuilder::for_tests()
        .with_minting_account(*TEST_ACCOUNT)
        .with_transfer_fee(DEFAULT_TRANSFER_FEE)
        .build();
        let ledger_wasm = icrc_ledger_old_certificate_wasm();
        let icrc_ledger_canister_id = create_and_install_custom_icrc_ledger(&pocket_ic, init_args.clone(), ledger_wasm, None);
        let endpoint = pocket_ic.make_live(None);
        let port = endpoint.port().unwrap();

        async fn check_blocks_synchronization_and_certificate(agent: Arc<Icrc1Agent>, transfer_args: Vec<TransferArg>) {
            for transfer_arg in transfer_args.iter() {
                agent.transfer(transfer_arg.clone()).await.unwrap().unwrap();
            }

            let storage_client = Arc::new(StorageClient::new_in_memory().unwrap());
            blocks_synchronizer::start_synching_blocks(agent.clone(), storage_client.clone(),10,Arc::new(AsyncMutex::new(vec![])), RecurrencyMode::OneShot, Box::new(|| {})).await.unwrap();
            check_storage_validity(storage_client.clone(),transfer_args.len().saturating_sub(1) as u64);

            // Now we check the certificate of the ledger
            let (hash,tip_index) = agent.get_certified_chain_tip().await.unwrap().unwrap();
            assert_eq!(tip_index,transfer_args.len().saturating_sub(1) as u64);
            let tip_block = storage_client.get_block_with_highest_block_idx().unwrap().unwrap();
            assert_eq!(tip_block.get_block_hash(),hash);
        }

        // We are only interested in the scenario when there are blocks to be fetched
        if transfer_args.is_empty() {
            return Ok(());
        }
        rt.block_on(async {
            let agent = Arc::new(Icrc1Agent {
                agent: local_replica::get_testing_agent(port).await,
                ledger_canister_id: icrc_ledger_canister_id,
            });

            // If we fetch the certificate now we should get an empty certificate
            let certificate = agent.get_certified_chain_tip().await.unwrap();
            assert!(certificate.is_none());

            check_blocks_synchronization_and_certificate(agent.clone(),transfer_args.clone()).await;
        });

        // Now we install the newer version of the ledger
        let ledger_wasm = icrc_ledger_wasm();
        pocket_ic.reinstall_canister(icrc_ledger_canister_id, ledger_wasm,Encode!(&(LedgerArgument::Init(init_args.clone()))).unwrap(),None).unwrap();
        rt.block_on(async {
            let agent = Arc::new(Icrc1Agent {
                agent: local_replica::get_testing_agent(port).await,
                ledger_canister_id: icrc_ledger_canister_id,
            });
            // Now we check the blocks synchronizer again
            check_blocks_synchronization_and_certificate(agent.clone(),transfer_args.clone()).await;
        });
    }
}

#[test]
#[should_panic(expected = "is larger than highest_block_idx.saturating_add(1)")]
fn test_gaps_handling() {
    fn assert_block_count(connection: &Connection, block_count: u64) {
        let counter_value: Option<u64> = connection
            .query_row(
                "SELECT value FROM counters WHERE name = 'SyncedBlocks'",
                [],
                |row| row.get(0),
            )
            .expect("Getting the counter value should succeed");
        let num_blocks: Option<u64> = connection
            .query_row("SELECT COUNT(*) FROM blocks", [], |row| row.get(0))
            .expect("Getting the block count should succeed");
        assert_eq!(
            counter_value.expect("Should return a counter value"),
            block_count
        );
        assert_eq!(
            num_blocks.expect("Should return a block count"),
            block_count
        );
    }

    // Create a tokio environment to conduct async calls
    const DB_NAME: &str = "test_gaps_handling";
    const NUM_BLOCKS: u64 = 10;

    let rt = Runtime::new().unwrap();
    let mut pocket_ic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_sns_subnet()
        .build();
    let mut init_args = InitArgsBuilder::for_tests()
        .with_minting_account(*TEST_ACCOUNT)
        .with_transfer_fee(DEFAULT_TRANSFER_FEE)
        .with_archive_options(ArchiveOptions {
            trigger_threshold: 10_000,
            num_blocks_to_archive: 10_000,
            node_max_memory_size_bytes: None,
            max_message_size_bytes: None,
            controller_id: PrincipalId::new_user_test_id(100),
            more_controller_ids: None,
            cycles_for_archive_creation: None,
            max_transactions_per_response: None,
        })
        .build();
    let mut initial_balances: Vec<(Account, Nat)> = vec![];
    for i in 0..NUM_BLOCKS {
        initial_balances.push((
            Account {
                owner: PrincipalId::new_user_test_id(i).0,
                subaccount: None,
            },
            1_000_000_000_000u64.into(),
        ));
    }
    init_args.initial_balances = initial_balances;
    let icrc_ledger_canister_id = create_and_install_icrc_ledger(&pocket_ic, init_args, None);
    let endpoint = pocket_ic.make_live(None);
    let port = endpoint.port().unwrap();

    // Wrap async calls in a blocking Block
    rt.block_on(async {
        // Create a testing agent
        let agent = Arc::new(Icrc1Agent {
            agent: local_replica::get_testing_agent(port).await,
            ledger_canister_id: icrc_ledger_canister_id,
        });

        // Create the storage client where blocks will be stored
        let storage_client = Arc::new(StorageClient::new_named_in_memory(DB_NAME).unwrap());

        // Start the synching process
        // Conduct a full sync from the tip of the blockchain to genesis block
        blocks_synchronizer::start_synching_blocks(
            agent.clone(),
            storage_client.clone(),
            2,
            Arc::new(AsyncMutex::new(vec![])),
            RecurrencyMode::OneShot,
            Box::new(|| {}),
        )
        .await
        .unwrap();

        // Check that the full sync of all blocks generated by the first batch of blocks is valid
        check_storage_validity(storage_client.clone(), NUM_BLOCKS - 1);

        // Sync between the tip of the chain and the stored blocks
        // The blocksynchronizer now sync the blocks between the current tip of the chain and the most recently stored block
        blocks_synchronizer::sync_from_the_tip(
            agent.clone(),
            storage_client.clone(),
            2,
            Arc::new(AsyncMutex::new(vec![])),
        )
        .await
        .unwrap();

        // Check that the sync of all blocks generated by the second batch of blocks is valid
        check_storage_validity(storage_client.clone(), NUM_BLOCKS - 1);

        // Create a connection to the database
        let connection = Connection::open_with_flags(
            format!("'file:{}?mode=memory&cache=shared', uri=True", DB_NAME),
            OpenFlags::default(),
        )
        .unwrap();

        // The database should hold all the expected blocks.
        assert_block_count(&connection, NUM_BLOCKS);

        // Delete a block and decrement the counter in the database.
        let deleted_rows = connection
            .execute(r#"DELETE FROM blocks WHERE idx = 1"#, [])
            .unwrap();
        assert_eq!(deleted_rows, 1);
        let counters_updated = connection
            .execute(
                r#"UPDATE counters SET value = value - 1 WHERE name = "SyncedBlocks""#,
                [],
            )
            .unwrap();
        assert_eq!(counters_updated, 1);

        // The database should reflect the deleted block and decremented counter.
        assert_block_count(&connection, NUM_BLOCKS - 1);

        // Block syncing should succeed.
        blocks_synchronizer::start_synching_blocks(
            agent.clone(),
            storage_client.clone(),
            2,
            Arc::new(AsyncMutex::new(vec![])),
            RecurrencyMode::OneShot,
            Box::new(|| {}),
        )
        .await
        .unwrap();

        // Storage should still be valid
        check_storage_validity(storage_client.clone(), NUM_BLOCKS - 1);

        // Database should be updated
        assert_block_count(&connection, NUM_BLOCKS);

        // Only decrement the counter in the database.
        let counters_updated = connection
            .execute(
                r#"UPDATE counters SET value = value - 1 WHERE name = "SyncedBlocks""#,
                [],
            )
            .unwrap();
        assert_eq!(counters_updated, 1);

        // Block syncing should succeed.
        blocks_synchronizer::start_synching_blocks(
            agent.clone(),
            storage_client.clone(),
            2,
            Arc::new(AsyncMutex::new(vec![])),
            RecurrencyMode::OneShot,
            Box::new(|| {}),
        )
        .await
        .unwrap();

        // Storage should still be valid
        check_storage_validity(storage_client.clone(), NUM_BLOCKS - 1);

        // The database should have been updated.
        assert_block_count(&connection, NUM_BLOCKS);

        // Increment the block count counter in the database.
        let counters_updated = connection
            .execute(
                r#"UPDATE counters SET value = value + 1 WHERE name = "SyncedBlocks""#,
                [],
            )
            .unwrap();
        assert_eq!(counters_updated, 1);

        // Block syncing should panic.
        blocks_synchronizer::start_synching_blocks(
            agent.clone(),
            storage_client.clone(),
            2,
            Arc::new(AsyncMutex::new(vec![])),
            RecurrencyMode::OneShot,
            Box::new(|| {}),
        )
        .await
        .unwrap();
    });
}
