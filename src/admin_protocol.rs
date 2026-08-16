//! Small, transport-independent command boundary for the management portal.
//! The portal never imports the streaming control protocol; `main` adapts
//! these commands to the receiver's internal command queue.

#[derive(Debug, Clone, Copy)]
pub enum AdminCommand {
    StopSharing,
}
