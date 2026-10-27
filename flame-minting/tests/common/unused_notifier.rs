use std::convert::Infallible;

use flame_minting::core_block_notifier::CoreBlockNotifier;

pub struct UnusedNotifier;

impl CoreBlockNotifier for UnusedNotifier {
    type Error = Infallible;
}
