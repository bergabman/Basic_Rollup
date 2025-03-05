## SVM rollup
As part of the Turbin3 SVM cohort, our team built our own SVM rollup.
It fetches transactions, delegates funds via a Solana program, sends transactions to a sequencer, and processes them by locking accounts, loads and executes transactions, updating local state, and bundling similar transactions. Once the batch threshold is met (10 transactions), the rollup bundles them into one and settles the changes back on-chain.

## Overview
A rollup is a Layer-2 scaling solution that processes and bundles transactions off-chain before settling them on the main chain. This reduces congestion and fees while keeping the security of the underlying blockchain.

## Why would Solana need a rollup?
Rollups can enhance Solana by:
- **Increasing Throughput:** Offload transactions from the main chain.
- **Lower fees:** Batch transactions to lower costs.
- **Flexibility:** Allow for customized transaction processing without changing Solana’s core.

## Currently building:
- **Support for SPL tokens**
- **Support for all types of transactions**

## Flow
1. **Fetches Transactions** 
2. **Delegates Funds:** Solana program
3. **Sends to the Sequencer**
4. **Locks accounts, Loads, and executes Transactions** 
5. **Updates Local State** 
6. **Bundles Similar Transactions:** Groups similar transactions into one.
7. **Batch(10) Bundling:** After 10 transactions, bundles them into a single transaction.
8. **Settles Changes to the Chain:** Commits batched changes back to Solana.

## Module Overview

**rollup_client.rs**  
CLI that interacts with the rollup server.
  - Creating and signing a Solana transfer transaction.
  - Sends the transaction to the rollup via `/submit_transaction`.
  - Hashes the transaction, via `/get_transaction` for status.

**frontend.rs**  
  Actix Web
  - A submission endpoint (`/submit_transaction`) that accepts and forwards transactions to the sequencer.
  - A query endpoint (`/get_transaction`) that retrieves processed transactions from the rollup database.
  - A test endpoint to verify server functionality.

**loader.rs**  
  Implements the account loader for the rollup. This module:
  - Fetches account data from Solana using RPC client.
  - Caches account data locally.
  - Implements the `TransactionProcessingCallback` required by SVM API
    
**main.rs**  
  Entry point for the application. It:
  - Sets up communication channels, using crossbeam and async channels.
  - Creates threads for the sequencer and rollup database.

**processor.rs**  
  Provides helper functions to configure and initialize the SVM API’s transaction batch processor. 
It:
  - Implements a fork graph (required by the processor).
  - Sets up the processor’s program cache with built-in programs (system and BPF loader).

**rollupdb.rs**  
  Implements an in-memory database that manages:
  - Account states and locked accounts.
  - Processed transactions.
  - Communication with the frontend by retrieving transactions based on requests.  
  It handles locking and unlocking accounts as transactions are processed.

**sequencer.rs**  
  Acts as the transaction sequencer and processor. It:
  - Receives transactions via a crossbeam channel.
  - Locks accounts for parallel execution.
  - Uses Solana’s SVM API to process and validate transactions.
  - Batches transactions (every 10 transactions) and settles when the threshold is reached.

**settle.rs**  
  Contains the functionality to settle state changes on Solana. Creates and sends a proof transaction via Solana’s RPC, comitting updates to SVM.
