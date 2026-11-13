//! Presencia de nodos RSC y puerta de entrada RPC.
//!
//! `PeerId` es la identidad de red libp2p. No es una cuenta, un validador
//! ni una clave de firma de bloques. El registro solo acepta anuncios
//! firmados por la clave Ed25519 que deriva ese `PeerId`.
//!
//! El gateway reenvía el cuerpo HTTP tal cual. No reescribe JSON-RPC.

#![forbid(unsafe_code)]

pub mod announce;
pub mod sign;
pub mod types;
pub mod util;
pub mod validate;

#[cfg(feature = "server")]
pub mod gateway;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod store;

pub use announce::{spawn_announcer, spawn_from_env, AnnouncerConfig, AnnouncerHandle};
pub use types::{NodeStatus, NodeType};
