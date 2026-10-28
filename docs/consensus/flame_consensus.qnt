// -*- mode: Bluespec; -*-
/**
 * Flame minting consensus AS IMPLEMENTED in runflame/flame-lib @ fbd94bc (PR #14).
 *
 * This model deliberately reproduces the current behaviour, bugs included.
 * The invariants at the bottom state what docs/consensus/design-doc.md promises;
 * each `run` test drives the model into a state where one of them is violated,
 * reproducing an issue from the review doc (FL-01 .. FL-08). FL-09 was found while checking
 * this model against the code and is not in the review doc yet.
 * A fixed protocol should live in a separate module and pass the same invariants.
 *
 * TODO: this model is a hybrid of two PR #14 configurations that no single deployment runs:
 *  - blocks and weights follow demo-consensus: the integrator registers every block with
 *    parent_weight (flame.rs:93-131) and the root is a core block;
 *  - minters follow EndorsementPolicy::Auto: MinterWorker + Endorser + InMemoryMinterJournal.
 *    demo-consensus runs EndorsementPolicy::Manual instead: no worker, endorsements are sent
 *    by hand straight through the sender, bypassing the journal (session/mod.rs:198-208).
 *  Either bring the model to one configuration, or make the minter mode a parameter
 *  (Auto: once per tip with the journal; Manual: any block at any time, no journal).
 *
 * Abstractions (all deliberate, all listed):
 *  - Burns are reduced to a constant minting power per minter, i.e. floor(amount / duration)
 *    of an always-active burn. "x" is an identity with no burns at all.
 *  - Block hashes are integers and "smaller hash" = smaller id. Adversary blocks get ids
 *    below 100, honest ones 100+: Flame blocks need no work, so the adversary can always
 *    grind a smaller hash than any honest block it has seen.
 *  - PR #14 has no BFT. In demo-consensus every block is a core block with a user-chosen
 *    target and core height = parent's + 1, and the root is a core block with core height 1
 *    (demo-consensus flame.rs:45-48, 64-92). The model keeps that root, but the BFT is taken
 *    from the docs, not from code: an honest proposer that extends its own last block, restarts
 *    from node n1's canonical tip when fork choice moves to another branch (design-doc.md,
 *    "Required Properties"), may also produce regular blocks, and sets target = BTC tip + 2
 *    (core-blocks.md).
 *  - Honest minters act on node n1's view, like a minter attached to a local node, once per
 *    BTC tip (minter_worker.rs:44-47): a core block that shows up later at the same tip is
 *    not endorsed until the next tip.
 *  - Bitcoin: no reorgs, but miners may delay (censor) any endorsement for a while.
 *  - Rewards (FL-04) are not modelled: Quint has no floats. Only the halving constant is checked.
 *  - MAX_DELAY is 2 instead of 10 (orchestrator/defaults.rs) to keep traces short;
 *    the logic does not depend on the value.
 */
module flame_minting {

  // ------------------------------------------------------------------
  // Types
  // ------------------------------------------------------------------

  type BlockId = int

  type Block = {
    id: BlockId,
    parent: BlockId,
    height: int,        // Flame block height
    honest: bool,       // produced by honest BFT (model bookkeeping only)
    isCore: bool,       // header.core_block.is_some()
    coreHeight: int,    // CoreBlockHeader.height      -- NOT validated (FL-01)
    target: int,        // CoreBlockHeader.target_btc_height -- NOT validated (FL-01)
  }

  // MintingEndorsementData::V1 { flame_block_height, flame_block_hash } plus inclusion height.
  type Endorsement = { minter: str, blockId: BlockId, coreHeight: int, incl: int }

  // WeightedBlockHeader { parent_weight, effective_power }
  type WeightRec = { parentWeight: int, effPower: int }

  // What one node (ConsensusStorage + ChainStorage + cursor) holds.
  type NodeState = {
    cursor: int,                              // last processed Bitcoin height
    known: Set[BlockId],                      // ChainStorage
    importOrder: List[BlockId],               // order used by CoreBlockSource::get_core_block
    firstSeenAt: BlockId -> int,              // cursor when the block arrived (model bookkeeping)
    stored: (str, int) -> Endorsement,        // EndorsementStorage.endorsements keyed by (minter, core height)
    storedEff: (str, int) -> int,             // effective power stored with each endorsement
    pending: Set[Endorsement],                // EndorsementStorage.pending_endorsements
    doubleSigned: Set[str],                   // DoubleSignStorage: any entry = banned forever
    weights: BlockId -> WeightRec,            // WeightedHeaderStorage
  }

  type World = {
    btcHeight: int,
    btcChain: List[Set[Endorsement]],         // btcChain[h - 1] = endorsements included at height h
    mempool: Set[Endorsement],
    blocks: BlockId -> Block,                 // every Flame block ever created
    nextId: int,
    bft: BlockId,                             // BFT's own last block
  }

  // InMemoryMinterJournal: core height -> endorsed block; lost on restart.
  // lastTip: MinterWorker::run last_tip, -1 = None.
  // missed: target heights for which the worker found no core block (model bookkeeping, FL-09).
  type MinterState = { journal: int -> BlockId, conflicts: Set[int], lastTip: int, missed: Set[int] }

  // ------------------------------------------------------------------
  // Parameters
  // ------------------------------------------------------------------

  pure val HONEST = Set("h1", "h2")
  pure val ADVERSARY = Set("a")
  pure val NO_BURN = Set("x")
  pure val MINTERS = HONEST.union(ADVERSARY).union(NO_BURN)

  // Honest 7, adversary 4: the adversary holds 4/11 = 36% of the power.
  pure val POWER: str -> int = Map("h1" -> 4, "h2" -> 3, "a" -> 4, "x" -> 0)

  pure val NODES = Set("n1", "n2")

  pure val MAX_DELAY = 2            // code: max_endorsement_delay = 10
  pure val MIN_DURATION = 1         // code: orchestrator/defaults.rs:58
  pure val MATURITY = 1             // code: orchestrator/defaults.rs:56
  pure val RATIONALE_MIN_DURATION = 2048   // design-doc.md:324

  pure val MAX_BTC = 6
  pure val MAX_BLOCKS = 8
  pure val MAX_CORE = 5
  pure val GHOST = 999              // a block id that is never created
  pure val GENESIS = 0

  // ------------------------------------------------------------------
  // Pure helpers
  // ------------------------------------------------------------------

  // u64::checked_ilog2().unwrap_or(0)
  pure def ilog2(x: int): int =
    if (x <= 0) 0
    else 0.to(62).fold(0, (acc, i) => if (2^i <= x and i > acc) i else acc)

  // endorsement_weight.rs: power at target >> delay, zero past max delay
  pure def effPower(m: str, target: int, incl: int): int =
    if (incl - target > MAX_DELAY) 0 else POWER.get(m) / 2^(incl - target)

  pure def ancestorsOrSelf(blocks: BlockId -> Block, b: BlockId): Set[BlockId] =
    0.to(MAX_BLOCKS).fold(Set(b), (acc, _i) =>
      acc.union(acc.map(x => blocks.get(x).parent).filter(p => p >= 0)))

  pure def descendants(blocks: BlockId -> Block, known: Set[BlockId], b: BlockId): Set[BlockId] =
    known.filter(d => d != b and ancestorsOrSelf(blocks, d).contains(b))

  // nearest core ancestor-or-self, -1 if none
  pure def lastCore(blocks: BlockId -> Block, b: BlockId): BlockId =
    ancestorsOrSelf(blocks, b).filter(x => blocks.get(x).isCore).fold(-1, (best, x) =>
      if (best == -1) x
      else if (blocks.get(x).height > blocks.get(best).height) x
      else best)

  pure def lastCoreHeight(blocks: BlockId -> Block, b: BlockId): int = {
    val c = lastCore(blocks, b)
    if (c == -1) 0 else blocks.get(c).coreHeight
  }

  pure def lastCoreTarget(blocks: BlockId -> Block, b: BlockId): int = {
    val c = lastCore(blocks, b)
    if (c == -1) 0 else blocks.get(c).target
  }

  // weighted_header_storage.rs:50-57
  pure def chainWeight(blocks: BlockId -> Block, n: NodeState, b: BlockId): int = {
    val rec = n.weights.get(b)
    rec.parentWeight + (if (blocks.get(b).isCore) ilog2(rec.effPower) else 0)
  }

  // weighted_header_storage.rs:42-47: max weight, ties -> smallest hash; ALL blocks are candidates
  pure def canonicalTip(blocks: BlockId -> Block, n: NodeState): BlockId =
    n.known.fold(-1, (best, b) =>
      if (best == -1) b
      else if (chainWeight(blocks, n, b) > chainWeight(blocks, n, best)) b
      else if (chainWeight(blocks, n, b) == chainWeight(blocks, n, best) and b < best) b
      else best)

  pure def isAncestorOrSelf(blocks: BlockId -> Block, a: BlockId, b: BlockId): bool =
    ancestorsOrSelf(blocks, b).contains(a)

  // flame-chain-service in_memory.rs:78-92: FIRST imported block with that target, canonical or not
  pure def firstImportedWithTarget(blocks: BlockId -> Block, n: NodeState, target: int): BlockId =
    n.importOrder.foldl(-1, (acc, b) =>
      if (acc != -1) acc
      else if (blocks.get(b).isCore and blocks.get(b).target == target) b
      else -1)

  pure def keyOf(e: Endorsement): (str, int) = (e.minter, e.coreHeight)

  // ChainStorage::get_core_block_header: known AND its core height matches the endorsement
  pure def isKnownCore(blocks: BlockId -> Block, n: NodeState, e: Endorsement): bool =
    if (n.known.contains(e.blockId))
      blocks.get(e.blockId).isCore and blocks.get(e.blockId).coreHeight == e.coreHeight
    else false

  // endorsement_validator.rs:69-84: unknown block -> pending, no power check (FL-07)
  pure def isPendingE(blocks: BlockId -> Block, n: NodeState, e: Endorsement): bool =
    not(isKnownCore(blocks, n, e)) and not(n.doubleSigned.contains(e.minter))

  // endorsement_validator.rs:85-104: target reached, not banned NOW (FL-03), has burns
  pure def isEligible(blocks: BlockId -> Block, n: NodeState, e: Endorsement): bool =
    if (isKnownCore(blocks, n, e))
      and {
        e.incl >= blocks.get(e.blockId).target,
        not(n.doubleSigned.contains(e.minter)),
        POWER.get(e.minter) > 0,
      }
    else false

  // record_endorsement: the first one (lowest inclusion height) wins
  pure def firstOf(es: Set[Endorsement]): Endorsement =
    es.fold({ minter: "", blockId: -1, coreHeight: -1, incl: 1000000 }, (a, e) =>
      if (e.incl < a.incl) e else a)

  // block hashes seen for (minter, core height): this batch plus what is stored
  pure def hashesAt(n: NodeState, eligible: Set[Endorsement], k: (str, int)): Set[BlockId] =
    eligible.filter(e => keyOf(e) == k).map(e => e.blockId)
      .union(if (n.stored.keys().contains(k)) Set(n.stored.get(k).blockId) else Set())

  pure def deltaFor(
    blocks: BlockId -> Block, n: NodeState,
    accepted: Set[Endorsement], removedKeys: Set[(str, int)], b: BlockId
  ): int = {
    val added = accepted.filter(e => e.blockId == b)
      .fold(0, (s, e) => s + effPower(e.minter, blocks.get(b).target, e.incl))
    val removed = removedKeys.filter(k => n.stored.get(k).blockId == b)
      .fold(0, (s, k) => s + n.storedEff.get(k))
    added - removed
  }

  // weighter.rs:51-74: new effective power, log2 delta pushed into every known descendant
  pure def applyDelta(
    blocks: BlockId -> Block, known: Set[BlockId],
    ws: BlockId -> WeightRec, b: BlockId, d: int
  ): BlockId -> WeightRec = {
    val oldP = ws.get(b).effPower
    val newP = oldP + d
    val dl = ilog2(newP) - ilog2(oldP)
    val ws1 = ws.set(b, { ...ws.get(b), effPower: newP })
    descendants(blocks, known, b).fold(ws1, (acc, x) =>
      acc.set(x, { ...acc.get(x), parentWeight: acc.get(x).parentWeight + dl }))
  }

  /// EndorsementValidator + Weighter + ConsensusOperations on one batch of endorsements:
  /// a Bitcoin block, or the pending endorsements of a block that just arrived.
  pure def processBatch(blocks: BlockId -> Block, n: NodeState, batch: Set[Endorsement]): NodeState = {
    val eligible = batch.filter(e => isEligible(blocks, n, e))
    val pendingNew = batch.filter(e => isPendingE(blocks, n, e))
    val eKeys = eligible.map(e => keyOf(e))
    val storedKeys = n.stored.keys()
    // double sign = same minter, same CORE HEIGHT, different hash (FL-02)
    val dsKeys = eKeys.filter(k => hashesAt(n, eligible, k).size() > 1)
    val disq = dsKeys.map(k => k._1)
    // only the stored endorsement at the conflicting height is removed;
    // the minter's endorsements at other heights keep their weight (FL-03)
    val removedKeys = dsKeys.filter(k => storedKeys.contains(k))
    val accKeys = eKeys.filter(k => and {
      not(dsKeys.contains(k)),
      not(disq.contains(k._1)),
      not(storedKeys.contains(k)),
    })
    val accepted = accKeys.map(k => firstOf(eligible.filter(e => keyOf(e) == k)))
    val affected = removedKeys.map(k => n.stored.get(k).blockId).union(accepted.map(e => e.blockId))
    val weights1 = affected.fold(n.weights, (ws, b) =>
      applyDelta(blocks, n.known, ws, b, deltaFor(blocks, n, accepted, removedKeys, b)))
    val keptKeys = storedKeys.exclude(removedKeys)
    val stored1 = accepted.fold(keptKeys.mapBy(k => n.stored.get(k)), (m, e) => m.put(keyOf(e), e))
    val eff1 = accepted.fold(keptKeys.mapBy(k => n.storedEff.get(k)), (m, e) =>
      m.put(keyOf(e), effPower(e.minter, blocks.get(e.blockId).target, e.incl)))
    {
      ...n,
      stored: stored1,
      storedEff: eff1,
      pending: n.pending.exclude(batch).union(pendingNew.filter(e => not(disq.contains(e.minter)))),
      doubleSigned: n.doubleSigned.union(disq),
      weights: weights1,
    }
  }

  // Integration code (demo-consensus flame.rs:95-131) registers every block with
  // parent_weight = chain weight of the parent; flame_block_processor.rs:91-110 then
  // re-evaluates the pending endorsements of a core block against the CURRENT state.
  pure def arrive(blocks: BlockId -> Block, n: NodeState, b: BlockId): NodeState = {
    val pw = chainWeight(blocks, n, blocks.get(b).parent)
    val n1 = {
      ...n,
      known: n.known.union(Set(b)),
      importOrder: n.importOrder.append(b),
      weights: n.weights.put(b, { parentWeight: pw, effPower: 0 }),
      firstSeenAt: n.firstSeenAt.put(b, n.cursor),
    }
    if (blocks.get(b).isCore)
      processBatch(blocks, n1, n1.pending.filter(e => e.blockId == b and e.coreHeight == blocks.get(b).coreHeight))
    else n1
  }

  pure def processBtcBlock(world: World, n: NodeState): NodeState = {
    val h = n.cursor + 1
    val n1 = processBatch(world.blocks, n, world.btcChain[h - 1])
    { ...n1, cursor: h }
  }

  // ------------------------------------------------------------------
  // State
  // ------------------------------------------------------------------

  var w: World
  var nodes: str -> NodeState
  var minters: str -> MinterState

  // demo-consensus root: a core block with core height 1 (target_btc_height is configurable).
  // TODO: the spec does not define genesis (core or not, core height, target, endorsable);
  // sync this with docs/consensus once it does.
  pure val genesisBlock: Block = {
    id: GENESIS, parent: -1, height: 0, honest: true, isCore: true, coreHeight: 1, target: 0,
  }

  pure val emptyNode: NodeState = {
    cursor: 0,
    known: Set(GENESIS),
    importOrder: [GENESIS],
    firstSeenAt: Map(GENESIS -> 0),
    stored: Map(),
    storedEff: Map(),
    pending: Set(),
    doubleSigned: Set(),
    weights: Map(GENESIS -> { parentWeight: 0, effPower: 0 }),
  }

  action init = all {
    w' = {
      btcHeight: 0, btcChain: List(), mempool: Set(),
      blocks: Map(GENESIS -> genesisBlock), nextId: 1, bft: GENESIS,
    },
    nodes' = NODES.mapBy(_n => emptyNode),
    minters' = HONEST.mapBy(_m => { journal: Map(), conflicts: Set(), lastTip: -1, missed: Set() }),
  }

  // ------------------------------------------------------------------
  // Actions
  // ------------------------------------------------------------------

  // A Bitcoin block; endorsements of `censored` minters stay in the mempool.
  action mineBtc(censored: Set[str]): bool = {
    val included = w.mempool.filter(e => not(censored.contains(e.minter)))
    all {
      w.btcHeight < MAX_BTC,
      w' = {
        ...w,
        btcHeight: w.btcHeight + 1,
        btcChain: w.btcChain.append(included.map(e => { ...e, incl: w.btcHeight + 1 })),
        mempool: w.mempool.exclude(included),
      },
      nodes' = nodes,
      minters' = minters,
    }
  }

  // Honest BFT core block: target = latest BTC height + 2 (core-blocks.md),
  // core height = parent's last core height + 1, target above the chain's last core target.
  // TODO: bftPropose and bftRegular model the BFT from the docs; replace them with the real
  // BFT (IBFT or Simplex) once it lands in code.
  action bftPropose: bool = {
    val n = nodes.get("n1")
    val canon = canonicalTip(w.blocks, n)
    val parent = if (isAncestorOrSelf(w.blocks, canon, w.bft)) w.bft else canon
    val target = w.btcHeight + 2
    val id = 100 + w.nextId
    all {
      w.nextId < MAX_BLOCKS,
      lastCoreTarget(w.blocks, parent) < target,
      w' = {
        ...w,
        blocks: w.blocks.put(id, {
          id: id, parent: parent, height: w.blocks.get(parent).height + 1, honest: true,
          isCore: true, coreHeight: lastCoreHeight(w.blocks, parent) + 1, target: target,
        }),
        nextId: w.nextId + 1,
        bft: id,
      },
      nodes' = nodes,
      minters' = minters,
    }
  }

  // Honest BFT regular block.
  action bftRegular: bool = {
    val n = nodes.get("n1")
    val canon = canonicalTip(w.blocks, n)
    val parent = if (isAncestorOrSelf(w.blocks, canon, w.bft)) w.bft else canon
    val id = 100 + w.nextId
    all {
      w.nextId < MAX_BLOCKS,
      w' = {
        ...w,
        blocks: w.blocks.put(id, {
          id: id, parent: parent, height: w.blocks.get(parent).height + 1, honest: true,
          isCore: false, coreHeight: 0, target: 0,
        }),
        nextId: w.nextId + 1,
        bft: id,
      },
      nodes' = nodes,
      minters' = minters,
    }
  }

  // minter_worker.rs + endorser.rs: once per BTC tip t, endorse the block with target t + 1.
  // The journal is keyed by core height; a different hash at a used height is refused.
  // TODO: once LATE_MINTING_POLICY (core-blocks.md step 3) or wait_core_block is implemented,
  // let the minter endorse a core block that arrives after its tip was processed (FL-09).
  action honestEndorse(m: str): bool = {
    val n = nodes.get("n1")
    val target = w.btcHeight + 1
    val bid = firstImportedWithTarget(w.blocks, n, target)
    val ch = if (bid >= 0) w.blocks.get(bid).coreHeight else -1
    val prev = minters.get(m)
    val ms = { ...prev, lastTip: w.btcHeight, missed: prev.missed.exclude(Set(target)) }
    val reserved = ms.journal.keys().contains(ch)
    all {
      prev.lastTip != w.btcHeight,
      nodes' = nodes,
      if (bid < 0)
        // "No core block for the BTC target height": the tip is used up, no retry (FL-09)
        all { minters' = minters.set(m, { ...ms, missed: ms.missed.union(Set(target)) }), w' = w }
      else if (reserved and ms.journal.get(ch) == bid)
        // AlreadyReserved: nothing to do
        all { minters' = minters.set(m, ms), w' = w }
      else if (reserved)
        // InMemoryMinterJournal::ConflictingEndorsement: logged, minter abstains (FL-02)
        all {
          minters' = minters.set(m, { ...ms, conflicts: ms.conflicts.union(Set(ch)) }),
          w' = w,
        }
      else
        all {
          minters' = minters.set(m, { ...ms, journal: ms.journal.put(ch, bid) }),
          w' = { ...w, mempool: w.mempool.union(Set({ minter: m, blockId: bid, coreHeight: ch, incl: -1 })) },
        },
    }
  }

  // The journal lives in memory: a restart forgets every reservation (FL-02),
  // and the new worker processes the current tip again.
  action restartMinter(m: str): bool = all {
    minters.get(m).journal.keys().size() > 0,
    minters' = minters.set(m, { ...minters.get(m), journal: Map(), conflicts: Set(), lastTip: -1 }),
    w' = w,
    nodes' = nodes,
  }

  // Anyone may build a block anywhere, with any core metadata (design-doc.md:444, FL-01).
  action advBlock(parent: BlockId, isCore: bool, ch: int, target: int): bool = all {
    w.nextId < MAX_BLOCKS,
    w.blocks.keys().contains(parent),
    w' = {
      ...w,
      blocks: w.blocks.put(w.nextId, {
        id: w.nextId, parent: parent, height: w.blocks.get(parent).height + 1, honest: false,
        isCore: isCore, coreHeight: if (isCore) ch else 0, target: if (isCore) target else 0,
      }),
      nextId: w.nextId + 1,
    },
    nodes' = nodes,
    minters' = minters,
  }

  // Adversary (or a burn-less identity) endorses any (core height, hash), known or not.
  action advEndorse(m: str, bid: BlockId, ch: int): bool = all {
    w' = { ...w, mempool: w.mempool.union(Set({ minter: m, blockId: bid, coreHeight: ch, incl: -1 })) },
    nodes' = nodes,
    minters' = minters,
  }

  // Gossip: any created block may reach any node at any time, or never (withholding).
  action deliver(nd: str, b: BlockId): bool = all {
    w.blocks.keys().contains(b),
    not(nodes.get(nd).known.contains(b)),
    nodes.get(nd).known.contains(w.blocks.get(b).parent),
    nodes' = nodes.set(nd, arrive(w.blocks, nodes.get(nd), b)),
    w' = w,
    minters' = minters,
  }

  action processBtc(nd: str): bool = all {
    nodes.get(nd).cursor < w.btcHeight,
    nodes' = nodes.set(nd, processBtcBlock(w, nodes.get(nd))),
    w' = w,
    minters' = minters,
  }

  action step = any {
    nondet censored = MINTERS.powerset().oneOf()
    mineBtc(censored),
    bftPropose,
    bftRegular,
    nondet m = HONEST.oneOf()
    honestEndorse(m),
    nondet m = HONEST.oneOf()
    restartMinter(m),
    nondet parent = w.blocks.keys().oneOf()
    nondet isCore = Set(true, false).oneOf()
    nondet ch = 0.to(MAX_CORE).oneOf()
    nondet target = 0.to(MAX_BTC + 2).oneOf()
    advBlock(parent, isCore, ch, target),
    nondet m = ADVERSARY.union(NO_BURN).oneOf()
    nondet bid = w.blocks.keys().union(Set(GHOST)).oneOf()
    nondet ch = 0.to(MAX_CORE).oneOf()
    advEndorse(m, bid, ch),
    nondet nd = NODES.oneOf()
    nondet b = w.blocks.keys().oneOf()
    deliver(nd, b),
    nondet nd = NODES.oneOf()
    processBtc(nd),
  }

  // ------------------------------------------------------------------
  // Properties promised by the design doc (all expected to FAIL here)
  // ------------------------------------------------------------------

  // FL-01: on the canonical chain core targets strictly increase (one core block per BTC slot).
  val canonicalSlotsIncrease: bool = {
    val n = nodes.get("n1")
    val cores = ancestorsOrSelf(w.blocks, canonicalTip(w.blocks, n)).filter(b => w.blocks.get(b).isCore)
    cores.forall(x => cores.forall(y =>
      (w.blocks.get(x).height < w.blocks.get(y).height) implies
        (w.blocks.get(x).target < w.blocks.get(y).target)))
  }

  pure def endorsedOnTimeByAllHonest(blocks: BlockId -> Block, n: NodeState, b: BlockId): bool =
    HONEST.forall(m => {
      val k = (m, blocks.get(b).coreHeight)
      if (n.stored.keys().contains(k))
        n.stored.get(k).blockId == b and n.stored.get(k).incl == blocks.get(b).target
      else false
    })

  // FL-01, FL-05: a 36% adversary cannot displace an honest core block
  // that every honest minter endorsed with zero delay.
  val honestCoreBlocksStay: bool = {
    val n = nodes.get("n1")
    val tip = canonicalTip(w.blocks, n)
    n.known
      .filter(b => w.blocks.get(b).honest and w.blocks.get(b).isCore and endorsedOnTimeByAllHonest(w.blocks, n, b))
      .forall(b => isAncestorOrSelf(w.blocks, b, tip))
  }

  // FL-02: an honest minter following fork choice is never blocked by its own journal...
  val honestMintersNeverBlocked: bool = HONEST.forall(m => minters.get(m).conflicts.size() == 0)

  // FL-02: ...and never gets banned for double signing.
  val honestNeverDoubleSigned: bool =
    NODES.forall(nd => HONEST.forall(m => not(nodes.get(nd).doubleSigned.contains(m))))

  // FL-03: two nodes that saw the same data compute the same weights and the same tip.
  val determinism: bool = {
    val a = nodes.get("n1")
    val b = nodes.get("n2")
    (a.cursor == b.cursor and a.known == b.known) implies
      (a.weights == b.weights and canonicalTip(w.blocks, a) == canonicalTip(w.blocks, b))
  }

  // FL-07: pending endorsements come from minters with power and expire after MAX_DELAY.
  val pendingBounded: bool =
    NODES.forall(nd => nodes.get(nd).pending.forall(e =>
      POWER.get(e.minter) > 0 and nodes.get(nd).cursor - e.incl <= MAX_DELAY))

  // FL-08: design-doc.md:144 -- a block that shows up long after its slot cannot gain weight.
  val lateBlocksGetNoWeight: bool = {
    val n = nodes.get("n1")
    n.known.forall(b =>
      if (w.blocks.get(b).isCore and n.firstSeenAt.get(b) - w.blocks.get(b).target > MAX_DELAY)
        n.weights.get(b).effPower == 0
      else true)
  }

  // FL-09: core-blocks.md step 3 -- an honest minter that found no core block for its tip
  // still endorses it once it arrives; it never ends up holding a missed slot whose honest
  // core block node n1 knows by now.
  val honestMintersNeverMissSlot: bool = {
    val n = nodes.get("n1")
    HONEST.forall(m => minters.get(m).missed.forall(t =>
      n.known.forall(b => not(and {
        w.blocks.get(b).honest,
        w.blocks.get(b).isCore,
        w.blocks.get(b).target == t,
      }))))
  }

  // FL-06: the security argument assumes burns last at least 2048 blocks.
  val paramsMatchRationale: bool = MIN_DURATION >= RATIONALE_MIN_DURATION

  val allInvariants: bool = and {
    canonicalSlotsIncrease,
    honestCoreBlocksStay,
    honestMintersNeverBlocked,
    honestNeverDoubleSigned,
    determinism,
    pendingBounded,
    lateBlocksGetNoWeight,
    honestMintersNeverMissSlot,
  }

  // ------------------------------------------------------------------
  // Reproductions: each run ends in a state that violates one promise
  // ------------------------------------------------------------------

  // FL-01: three core blocks on one target outweigh an honest block with 7/11 of the power.
  run fl01CoreSpamTest =
    init
      .then(advBlock(GENESIS, true, 2, 1))                // id 1
      .then(advBlock(1, true, 3, 1))                      // id 2, same target
      .then(advBlock(2, true, 4, 1))                      // id 3, same target
      .then(advEndorse("a", 1, 2))
      .then(advEndorse("a", 2, 3))
      .then(advEndorse("a", 3, 4))
      .then(bftPropose)                                   // id 104: core 2, target 2
      .then(deliver("n1", 104))
      .then(deliver("n1", 1))
      .then(deliver("n1", 2))
      .then(deliver("n1", 3))
      .then(mineBtc(Set()))                               // BTC 1: adversary endorsements
      .then(honestEndorse("h1"))
      .then(honestEndorse("h2"))
      .then(mineBtc(Set()))                               // BTC 2: honest endorsements, delay 0
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .expect(and {
        canonicalTip(w.blocks, nodes.get("n1")) == 3,     // weight 6 vs 2
        not(canonicalSlotsIncrease),
        not(honestCoreBlocksStay),
      })

  // FL-05: same slot, valid metadata, 4 vs 7 power -> both weigh floor(log2) = 2;
  // the adversary's smaller hash wins the tie.
  run fl05TieGrindTest =
    init
      .then(bftPropose)                                   // id 101: core 2, target 2
      .then(deliver("n1", 101))
      .then(advBlock(GENESIS, true, 2, 2))                // id 2: core 2, target 2
      .then(deliver("n1", 2))
      .then(mineBtc(Set()))                               // BTC 1
      .then(honestEndorse("h1"))
      .then(honestEndorse("h2"))
      .then(advEndorse("a", 2, 2))
      .then(mineBtc(Set()))                               // BTC 2: everyone on time
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .expect(and {
        canonicalTip(w.blocks, nodes.get("n1")) == 2,
        canonicalSlotsIncrease,                           // not FL-01: metadata is valid
        not(honestCoreBlocksStay),
      })

  // FL-02: Bitcoin miners delay honest endorsements, a valid adversary branch with fewer
  // core blocks wins, BFT restarts on it and reuses core height 3. The honest journal refuses
  // to endorse; after a restart the minter endorses and is banned forever.
  run fl02HonestSlashedTest =
    init
      .then(bftPropose)                                   // A1 = 101: core 2, target 2
      .then(deliver("n1", 101))
      .then(mineBtc(Set()))                               // BTC 1
      .then(honestEndorse("h1"))                          // journal: 2 -> 101
      .then(honestEndorse("h2"))
      .then(bftPropose)                                   // A2 = 102: core 3, target 3
      .then(deliver("n1", 102))
      .then(advBlock(GENESIS, true, 2, 3))                // B1 = 3: core 2, target 3
      .then(deliver("n1", 3))
      .then(mineBtc(HONEST))                              // BTC 2: honest txs delayed
      .then(honestEndorse("h1"))                          // journal: 3 -> 102
      .then(honestEndorse("h2"))
      .then(advEndorse("a", 3, 2))
      .then(mineBtc(HONEST))                              // BTC 3: only the adversary
      .then(mineBtc(Set()))                               // BTC 4: honest txs, delays 2 and 1
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(processBtc("n1"))                             // B1 (2) beats A1 + A2 (0 + 1)
      .then(bftPropose)                                   // 104 on B1: core 3, target 6
      .then(deliver("n1", 104))
      .then(mineBtc(Set()))                               // BTC 5
      .then(honestEndorse("h1"))                          // core 3 already reserved for 102
      .expect(and {
        // fork choice moved to B1; the fresh 104 ties with its parent and loses on hash (FL-05)
        canonicalTip(w.blocks, nodes.get("n1")) == 3,
        w.blocks.get(104).parent == 3,
        not(honestMintersNeverBlocked),
      })
      .then(restartMinter("h1"))                          // in-memory journal is gone
      .then(honestEndorse("h1"))
      .then(mineBtc(Set()))                               // BTC 6
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .expect(not(honestNeverDoubleSigned))

  // FL-03: same blocks, same Bitcoin chain, different arrival order -> different weights.
  run fl03OrderDependenceTest =
    init
      .then(advBlock(GENESIS, true, 2, 1))                // B1 = 1: core 2
      .then(advBlock(GENESIS, true, 3, 2))                // X = 2: core 3
      .then(advBlock(GENESIS, true, 3, 2))                // Y = 3: core 3, other hash
      .then(advEndorse("a", 1, 2))
      .then(mineBtc(Set()))                               // BTC 1: a -> B1
      .then(advEndorse("a", 2, 3))
      .then(advEndorse("a", 3, 3))
      .then(mineBtc(Set()))                               // BTC 2: a signs X and Y
      // n1 knows B1 early
      .then(deliver("n1", 1))
      .then(processBtc("n1"))
      .then(deliver("n1", 2))
      .then(deliver("n1", 3))
      .then(processBtc("n1"))
      // n2 learns about the double sign before B1 arrives
      .then(processBtc("n2"))
      .then(deliver("n2", 2))
      .then(deliver("n2", 3))
      .then(processBtc("n2"))
      .then(deliver("n2", 1))
      .expect(and {
        nodes.get("n1").weights.get(1).effPower == 4,
        nodes.get("n2").weights.get(1).effPower == 0,
        not(determinism),
      })

  // FL-07: an identity without burns endorses a block that will never exist; it stays forever.
  run fl07PendingDosTest =
    init
      .then(advEndorse("x", GHOST, 1))
      .then(mineBtc(Set()))
      .then(processBtc("n1"))
      .then(mineBtc(Set()))
      .then(mineBtc(Set()))
      .then(mineBtc(Set()))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .expect(and {
        nodes.get("n1").pending.size() == 1,
        not(pendingBounded),
      })

  // FL-08: endorse a hidden block on time, reveal it three BTC blocks later: full weight.
  run fl08HiddenForkTest =
    init
      .then(advBlock(GENESIS, true, 2, 1))                // id 1: core 2, withheld
      .then(advEndorse("a", 1, 2))
      .then(mineBtc(Set()))                               // BTC 1: on time
      .then(mineBtc(Set()))
      .then(mineBtc(Set()))
      .then(mineBtc(Set()))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .then(deliver("n1", 1))                             // revealed at cursor 4
      .expect(and {
        nodes.get("n1").weights.get(1).effPower == 4,
        not(lateBlocksGetNoWeight),
      })

  // FL-09: the honest core block reaches n1 right after h1 processed the tip: h1 never
  // endorses it, h2 (which processes the tip later) does. The block gets 3 of 7 honest power.
  run fl09MissedSlotTest =
    init
      .then(bftPropose)                                   // id 101: core 2, target 2; gossip slow
      .then(mineBtc(Set()))                               // BTC 1
      .then(honestEndorse("h1"))                          // no core block for target 2 yet
      .then(deliver("n1", 101))                           // arrives at the same tip
      .then(honestEndorse("h2"))                          // h2 sees it
      .expect(and {
        minters.get("h1").missed == Set(2),
        not(honestMintersNeverMissSlot),
      })
      .then(mineBtc(Set()))                               // BTC 2: only h2, delay 0
      .then(honestEndorse("h1"))                          // tip 2 -> target 3: nothing
      .then(processBtc("n1"))
      .then(processBtc("n1"))
      .expect(and {
        nodes.get("n1").weights.get(101).effPower == 3,
        not(nodes.get("n1").stored.keys().contains(("h1", 2))),
        not(honestMintersNeverMissSlot),
      })

  // FL-06: parameters in code contradict the rationale.
  run fl06ParamsTest = init.expect(not(paramsMatchRationale))

  // Minor: 50 FLM in sparks reaches zero after 33 halvings, not 64 (design-doc.md:549).
  run halvingTest = init.expect(5_000_000_000 / 2^33 == 0 and 5_000_000_000 / 2^32 > 0)
}
