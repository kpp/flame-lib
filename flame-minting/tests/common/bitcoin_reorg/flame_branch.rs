use super::*;

pub(super) fn flame_branch(mut state: Blockchain, branch: u8, length: u32) -> Vec<Arc<Block>> {
    (1..=length)
        .map(|height| {
            let mut block = state.build_block([branch; 32], vec![]).unwrap();
            block.header.core_block = Some(CoreBlockHeader {
                height: height.into(),
                target_btc_height: 2,
            });
            state.connect(&block).unwrap();
            Arc::new(block)
        })
        .collect()
}

pub(super) async fn register_block(
    block: &Arc<Block>,
    blocks: &InMemoryChainStorage,
    chain: &mut InMemoryChain,
    consensus: &InMemoryConsensusStorage,
) {
    blocks.add_block(block.clone()).await.unwrap();
    chain.import_block(block.clone()).await.unwrap();
    consensus
        .store_cumulative_weight(
            block.header.block_tip(),
            &WeightedBlockHeader {
                header: block.header.clone(),
                parent_weight: 0,
                effective_power: 0,
            },
        )
        .await
        .unwrap();
}
