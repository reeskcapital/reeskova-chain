//! Tipo de nodo y estado de presencia. No son identidades financieras.

use serde::{Deserialize, Serialize};

/// Programa que se anuncia. Cada binario declara el suyo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    /// Nodo principal. Puede publicar el RPC de la cadena y sellar bloques.
    Full,
    /// Proceso validador. No es el RPC público principal.
    Validator,
    /// Nodo móvil. No publica el RPC principal.
    Mobile,
    /// Nodo comunitario. No publica el RPC principal.
    Community,
    /// Participa solo en libp2p.
    P2p,
}

impl NodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Validator => "validator",
            Self::Mobile => "mobile",
            Self::Community => "community",
            Self::P2p => "p2p",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "validator" => Some(Self::Validator),
            "mobile" => Some(Self::Mobile),
            "community" => Some(Self::Community),
            "p2p" => Some(Self::P2p),
            _ => None,
        }
    }

    /// Solo el nodo principal sella bloques en este proceso.
    pub fn produces_blocks(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Solo el nodo principal puede ser el RPC público de las aplicaciones.
    pub fn allows_primary_rpc(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn allows_rpc_url(self) -> bool {
        matches!(self, Self::Full | Self::Validator)
    }
}

/// Estado de presencia. `OFFLINE` también aparece cuando el heartbeat vence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    #[serde(rename = "STARTING")]
    Starting,
    #[serde(rename = "ONLINE")]
    Online,
    #[serde(rename = "DEGRADED")]
    Degraded,
    #[serde(rename = "OFFLINE")]
    Offline,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Online => "ONLINE",
            Self::Degraded => "DEGRADED",
            Self::Offline => "OFFLINE",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "STARTING" => Some(Self::Starting),
            "ONLINE" => Some(Self::Online),
            "DEGRADED" => Some(Self::Degraded),
            "OFFLINE" => Some(Self::Offline),
            _ => None,
        }
    }
}

/// Ficha pública. `primary_rpc` y `produces_blocks` los fija el registro.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub peer_id: String,
    pub network_id: String,
    pub chain_id: u32,
    pub p2p_multiaddr: Option<String>,
    pub rpc_public_url: Option<String>,
    pub service_url: Option<String>,
    pub node_type: String,
    pub status: String,
    pub last_heartbeat: u64,
    pub registered_at: u64,
    pub software_version: String,
    pub primary_rpc: bool,
    pub produces_blocks: bool,
    pub public_key_hex: String,
    pub sequence: u64,
}
