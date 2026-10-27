pub const VERSION_V1: u8 = 1;

pub const OP_RETURN: u8 = 0x6a;
pub const OP_PUSHBYTES_41: u8 = 0x29;

pub const ACQUISITION_MAGIC: [u8; 4] = *b"FLMS";
pub const MINTING_MAGIC: [u8; 4] = *b"FLMB";

pub const ACQUISITION_PAYLOAD_LEN: usize = 69;
pub const ACQUISITION_WITH_DURATION_PAYLOAD_LEN: usize = 71;
pub const MINTING_PAYLOAD_LEN: usize = 41;
