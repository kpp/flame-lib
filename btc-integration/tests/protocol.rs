mod common;

use bitcoind::anyhow::Context;
use btc_integration::protocol::{MintingEndorsementData, validate_transaction_endorsements};
use common::{create_connection, setup};
use corepc_client::bitcoin::Amount;
use flamechain::BlockHash;

#[tokio::test(flavor = "multi_thread")]
async fn minting_sender_publishes_an_authenticated_endorsement_on_regtest()
-> bitcoind::anyhow::Result<()> {
    let ctx = setup()?;
    let connection = create_connection(&ctx).await?;
    let sender = connection.get_sender();
    let flame_block_hash = BlockHash::from([0x42; 32]);

    let endorsement_txid = sender.send_endorsement(1234, flame_block_hash).await?;
    let confirmation_block = ctx.generate_next_block()?;
    let confirmed = ctx
        .rpc
        .transactions_with_prevouts_in_block(confirmation_block)
        .await?
        .into_iter()
        .find(|entry| entry.transaction.compute_txid() == endorsement_txid)
        .context("signed Minter endorsement was not included in the next block")?;
    let endorsements = validate_transaction_endorsements(&confirmed)?;

    assert_eq!(endorsements.len(), 1);
    assert_eq!(endorsements[0].txid(), endorsement_txid);
    let endorsement_output_index = endorsements[0].output().output_index as usize;
    assert_eq!(
        confirmed.transaction.output[endorsement_output_index].value,
        Amount::ZERO
    );
    assert_eq!(
        endorsements[0].output().data,
        MintingEndorsementData::V1 {
            flame_block_height: 1234,
            flame_block_hash,
        }
    );

    Ok(())
}
