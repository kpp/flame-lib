use crate::block_processing::journal::consensus::{BitcoinOperationKind, ConsensusOperation};

use super::*;

impl<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> ConsensusManager<B, F, J> {
    pub(super) async fn recover(&self) -> Result<BtcBlockTip, ManagerError<B, F, J>> {
        let latest = self
            .journal
            .latest_entry()
            .await
            .map_err(ConsensusManagerError::Journal)?;
        let Some(entry) = latest else {
            return self
                .bitcoin_processor
                .resume()
                .await
                .map_err(ConsensusManagerError::Bitcoin);
        };
        if !entry.is_complete() {
            return match entry.operation {
                ConsensusOperation::Bitcoin(_) => self
                    .bitcoin_processor
                    .resume()
                    .await
                    .map_err(ConsensusManagerError::Bitcoin),
                ConsensusOperation::Flame(operation) => {
                    self.flame_processor
                        .resume()
                        .await
                        .map_err(ConsensusManagerError::Flame)?;
                    Ok(operation.btc_tip)
                }
            };
        }
        Ok(match entry.operation {
            ConsensusOperation::Bitcoin(operation) => match operation.kind {
                BitcoinOperationKind::Apply => operation.entry.btc_tip,
                BitcoinOperationKind::Rollback => operation.entry.previous_btc_tip,
            },
            ConsensusOperation::Flame(operation) => operation.btc_tip,
        })
    }

    pub(super) async fn process_event(
        bitcoin_processor: &B,
        flame_processor: &F,
        event: ConsensusEvent,
        cursor: &mut BtcBlockTip,
    ) -> Result<(), ManagerError<B, F, J>> {
        match event {
            ConsensusEvent::NewBitcoinBlock(block) => {
                *cursor = bitcoin_processor
                    .apply(block)
                    .await
                    .map_err(ConsensusManagerError::Bitcoin)?;
            }
            ConsensusEvent::NewFlameBlockArrival(tip) => {
                flame_processor
                    .process(tip)
                    .await
                    .map_err(ConsensusManagerError::Flame)?;
            }
            ConsensusEvent::BitcoinReorg {
                removed_block_tips,
                new_blocks,
            } => {
                for tip in removed_block_tips {
                    *cursor = bitcoin_processor
                        .rollback(tip)
                        .await
                        .map_err(ConsensusManagerError::Bitcoin)?;
                }
                for block in new_blocks {
                    *cursor = bitcoin_processor
                        .apply(block)
                        .await
                        .map_err(ConsensusManagerError::Bitcoin)?;
                }
            }
        }
        Ok(())
    }
}
