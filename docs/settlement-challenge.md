# Settlement challenge

The rollup executes SOL transfers and can pull accounts from a Solana cluster, but
nothing connects its result back to that cluster. A wallet whose balance was pulled
can still spend the same lamports on the cluster, and what it earned in the rollup
exists nowhere else.

Your task is to design and build settlement: an on-chain program and the rollup code
that uses it, so that lamports used in the rollup are committed on the cluster, and a
wallet can leave the rollup with the balance it ended up with.

The sequencer may be trusted. Proofs are out of scope.

## Where it connects

- `rollup_core/src/settle.rs` is the settlement boundary. It only holds the status
  that receipts report today.
- `RollupDB::pull` in `rollup_core/src/rollupdb.rs` copies an account from the cluster
  the first time the rollup sees it.
- Phase 3 (load) and phase 6 (commit) in `rollup_core/src/sequencer.rs` are where a
  transaction's accounts enter the rollup and where their new balances are stored.
- `ROLLUP_CLUSTER_URL` selects the cluster. Settlement must work on any cluster
  through configuration, never through a hard-coded address.

## Rules of the cluster you can build on

Each of these can be checked on a local validator in a few lines. Check them; do not
take them on trust.

- An account has exactly one owner program. Only the owner can take lamports out of
  it or change its data.
- Any program can add lamports to any writable account.
- A wallet can make another program its owner with the System program's `Assign`
  instruction, which the wallet must sign.
- A program can give away an account it owns, but only while the account's data is
  empty or all zeroes.
- A signature proves control of a private key. It does not depend on who owns the
  account.
- The fee payer of a transaction must be owned by the System program.
- A program can own accounts at addresses derived from its own id, which have no
  private key.

## Decisions

There is more than one working design. For each decision, pick an option, and write
down why you picked it and what it costs.

1. **How do lamports get committed to the rollup?** A wallet could hand itself to
   your program, or deposit into an account your program owns. What does each mean
   for the wallet's address inside the rollup, for wallet software, and for what the
   wallet can still do on the cluster meanwhile?
2. **What does the rollup accept?** What should happen to a transaction that pays
   from, or pays to, an account that has not committed anything?
3. **What is settled: balances or differences?** Lamports can arrive at a committed
   account on the cluster while it is used in the rollup. What happens to them under
   each choice?
4. **Between whom are lamports moved?** Directly between the wallets that paid each
   other, or between each wallet and a shared pool? Which choice lets one wallet
   leave while all others stay?
5. **Where do the lamports for an early leaver come from?** A wallet that earned may
   want to leave before the wallets that paid it. Does the sequencer lend its own
   funds, or can the lamports be found elsewhere?
6. **Who is allowed to settle?** How does the program know the sequencer, and what
   stops someone else from settling with invented numbers?
7. **Who pays the cluster fees?** Decide for committing, for settling and for
   leaving. Look closely at which accounts are able to pay a fee at each moment.
8. **How does a wallet leave?** Who starts it, who signs, and in which order? What
   must be true in the rollup at the moment the cluster transaction is sent?
9. **What if the sequencer disappears, or restarts?** Rollup state is in memory.
   Decide what you guarantee and state what you do not.

MagicBlock's delegation program solves a related problem and its source is public:
<https://github.com/magicblock-labs/delegation-program>. Read it for ideas after you
have a first design of your own, and note where your decisions differ.

## Done means

- The on-chain program is in this repository and builds with `cargo build-sbf`.
- The rollup uses it when it is configured, and still runs from a genesis file
  without it.
- A `cargo xtask` task proves the whole flow on a local validator with the real
  server and client: two wallets commit lamports, one pays the other in the rollup,
  the receiver leaves first, the payer leaves later.
- That task checks account owners and exact lamport balances on the cluster after
  each step, including every fee.
- It also shows what your design refuses, for example a wallet that has not
  committed anything, or a settlement nobody authorised.
- `cargo xtask verify` still passes.
- The README describes how settlement works and lists its limits honestly.
- A short text answers the nine decisions above: what you chose and why.

Keep it small. A complete solution, including its test, fits in under a thousand
lines.
