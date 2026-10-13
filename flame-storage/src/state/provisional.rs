use std::future::Future;

use flamechain::BlockHash;

pub trait ProvisionalStorage {
    type State;
    type Error;

    fn get_state(
        &self,
        _tip: BlockHash,
    ) -> impl Future<Output = Result<Option<Self::State>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_state(
        &self,
        _tip: BlockHash,
        _state: &Self::State,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }
}
