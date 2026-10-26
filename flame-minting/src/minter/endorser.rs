use super::{
    EndorsementReservation, MinterJournal, sending::endorsement_sender::EndorsementSender,
};
use btc_integration::MintingEndorsementData;
use flamechain::Block;
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub enum EndorsementError<S, J> {
    NotCoreBlock,
    Journal(J),
    Send(S),
}

pub struct Endorser<S, J> {
    sender: Arc<S>,
    journal: Arc<J>,
}

impl<S, J> Endorser<S, J>
where
    S: EndorsementSender,
    J: MinterJournal<TransactionId = S::TransactionId> + Send + Sync,
{
    pub fn new(sender: Arc<S>, journal: Arc<J>) -> Self {
        Self { sender, journal }
    }

    pub async fn endorse(
        &self,
        block: Arc<Block>,
    ) -> Result<(), EndorsementError<S::Error, J::Error>> {
        let core = block
            .header
            .core_block
            .as_ref()
            .ok_or(EndorsementError::NotCoreBlock)?;
        let hash = block.header.id();
        let endorsement = MintingEndorsementData::V1 {
            flame_block_height: core.height.as_u32(),
            flame_block_hash: hash,
        };
        let reservation = self
            .journal
            .write_reserve_endorsement(&endorsement)
            .await
            .map_err(EndorsementError::Journal)?;
        if reservation == EndorsementReservation::AlreadyReserved {
            return Ok(());
        }
        let transaction_id = self
            .sender
            .send_endorsement(core.height.as_u32(), hash)
            .await
            .map_err(EndorsementError::Send)?;
        self.journal
            .write_sent_endorsement(&endorsement, &transaction_id)
            .await
            .map_err(EndorsementError::Journal)?;
        log::info!(
            "sent endorsement for core block {} ({hash:?})",
            core.height.as_u32()
        );
        Ok(())
    }
}
