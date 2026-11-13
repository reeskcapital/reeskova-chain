//! Firma Ed25519 del anuncio. El `PeerId` tiene que salir de la misma clave.

use crate::util::{hex_decode, hex_encode};
use libp2p::identity::{Keypair, KeyType, PublicKey};
use libp2p::PeerId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedFields<'a> {
    pub peer_id: &'a str,
    pub network_id: &'a str,
    pub chain_id: u32,
    pub node_type: &'a str,
    pub p2p_multiaddr: &'a str,
    pub rpc_public_url: &'a str,
    pub service_url: &'a str,
    pub status: &'a str,
    pub software_version: &'a str,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sequence: u64,
    pub ttl_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    #[error("invalid signature")]
    Signature,
    #[error("identity mismatch")]
    Identity,
}

pub fn canonical(fields: &SignedFields<'_>) -> Vec<u8> {
    format!(
        "RSC-REG/1\n{peer}\n{network}\n{chain}\n{kind}\n{p2p}\n{rpc}\n{service}\n{status}\n{version}\n{issued}\n{expires}\n{sequence}\n{ttl}\n",
        peer = fields.peer_id,
        network = fields.network_id,
        chain = fields.chain_id,
        kind = fields.node_type,
        p2p = fields.p2p_multiaddr,
        rpc = fields.rpc_public_url,
        service = fields.service_url,
        status = fields.status,
        version = fields.software_version,
        issued = fields.issued_at,
        expires = fields.expires_at,
        sequence = fields.sequence,
        ttl = fields.ttl_secs,
    )
    .into_bytes()
}

pub fn dash(value: &Option<String>) -> String {
    match value {
        Some(text) if !text.is_empty() => text.clone(),
        _ => "-".to_string(),
    }
}

pub fn sign_fields(keypair: &Keypair, fields: &SignedFields<'_>) -> Result<String, SignError> {
    if keypair.key_type() != KeyType::Ed25519 {
        return Err(SignError::Identity);
    }
    let peer = PeerId::from(keypair.public());
    if peer.to_string() != fields.peer_id {
        return Err(SignError::Identity);
    }
    let raw = keypair
        .sign(&canonical(fields))
        .map_err(|_| SignError::Signature)?;
    if raw.len() != 64 {
        return Err(SignError::Signature);
    }
    Ok(hex_encode(&raw))
}

pub fn public_key_hex(keypair: &Keypair) -> Result<String, SignError> {
    if keypair.key_type() != KeyType::Ed25519 {
        return Err(SignError::Identity);
    }
    Ok(hex_encode(&keypair.public().encode_protobuf()))
}

pub fn verify_fields(
    public_key_hex: &str,
    fields: &SignedFields<'_>,
    signature_hex: &str,
) -> Result<(), SignError> {
    let key_bytes = hex_decode(public_key_hex).ok_or(SignError::Identity)?;
    let key = PublicKey::try_decode_protobuf(&key_bytes).map_err(|_| SignError::Identity)?;
    if key.key_type() != KeyType::Ed25519 {
        return Err(SignError::Identity);
    }
    if PeerId::from(&key).to_string() != fields.peer_id {
        return Err(SignError::Identity);
    }
    let signature = hex_decode(signature_hex).ok_or(SignError::Signature)?;
    if signature.len() != 64 {
        return Err(SignError::Signature);
    }
    if key.verify(&canonical(fields), &signature) {
        Ok(())
    } else {
        Err(SignError::Signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(peer: &'a str, rpc: &'a str, sequence: u64) -> SignedFields<'a> {
        SignedFields {
            peer_id: peer,
            network_id: "rsc-public",
            chain_id: 3,
            node_type: "full",
            p2p_multiaddr: "-",
            rpc_public_url: rpc,
            service_url: "-",
            status: "ONLINE",
            software_version: "0.1.0",
            issued_at: 1_000,
            expires_at: 1_045,
            sequence,
            ttl_secs: 45,
        }
    }

    #[test]
    fn signature_roundtrip_and_rejects_tamper() {
        let key = Keypair::generate_ed25519();
        let peer = PeerId::from(key.public()).to_string();
        let card = fields(&peer, "https://rpc.reeskova-chain.xyz/rpc", 1);
        let signature = sign_fields(&key, &card).unwrap();
        let public = public_key_hex(&key).unwrap();
        assert!(verify_fields(&public, &card, &signature).is_ok());

        let mut other = card;
        other.rpc_public_url = "https://evil.example/rpc";
        assert_eq!(
            verify_fields(&public, &other, &signature),
            Err(SignError::Signature)
        );

        let other_key = Keypair::generate_ed25519();
        assert_eq!(
            verify_fields(&public_key_hex(&other_key).unwrap(), &card, &signature),
            Err(SignError::Identity)
        );
    }

    #[test]
    fn error_text_has_no_secret() {
        let text = SignError::Signature.to_string();
        assert!(!text.to_ascii_lowercase().contains("bearer"));
        assert!(!text.to_ascii_lowercase().contains("private"));
    }
}
