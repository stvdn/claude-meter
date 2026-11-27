//! Application layer: the use cases (poll usage, sign in and out, change settings) and
//! the ports they need. Depends only on `domain`.

pub mod poller;
pub mod ports;
pub mod settings;
pub mod state;
