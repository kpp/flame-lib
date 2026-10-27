use super::*;

#[tokio::test]
async fn a_rolled_back_block_can_be_applied_again_with_fresh_undo() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    p.apply(fixture.block()).await.unwrap();
    p.rollback(btc_tip(101)).await.unwrap();
    p.apply(fixture.block()).await.unwrap();
    fixture.assert_applied().await;
    assert_eq!(p.engine.calls.load(Ordering::SeqCst), 2);
}
