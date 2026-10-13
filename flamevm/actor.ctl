type ActorID = [U8; 32];

enum ActorAddress: U8 {
    Init { ctor: ^Cell } = 0,
    Address { addr: ActorID } = 1,
    Restore { addr: ActorID, state: ^Cell } = 2,
}
