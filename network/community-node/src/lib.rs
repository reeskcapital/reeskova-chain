//! Nodos de la comunidad.
//!
//! Cada usuario liga un nodo completo contra la API. No hay cuartos: un
//! `link` abre el nodo y un `unlink` lo suelta. Quien ya está ligado no
//! suma otro.
//!
//! Equivalencia ([`rsc_node_tiers`]): **1 comunitario = 1 móvil 4/4**.
//! El comunitario es más fuerte por persona (un usuario aporta el asiento
//! entero); el móvil reparte ese mismo asiento entre cuatro mineros.

#![forbid(unsafe_code)]

use std::fmt;

use rsc_node_tiers::{NodeTier, TierCapacity};

/// Rechazo al ligar o soltar un nodo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommunityNodeError {
    /// El identificador del usuario llegó vacío.
    EmptyUser,
    /// La API de la cadena llegó vacía.
    EmptyApi,
    /// El estado guardado no se puede volver a abrir.
    InvalidState,
}

impl fmt::Display for CommunityNodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommunityNodeError::EmptyUser => write!(formatter, "usuario vacío"),
            CommunityNodeError::EmptyApi => write!(formatter, "api vacía"),
            CommunityNodeError::InvalidState => write!(formatter, "estado inválido"),
        }
    }
}

/// Un nodo de comunidad ligado a la API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityLink {
    /// Índice estable: 0 es el primero que se ligó.
    pub index: u32,
    /// Usuario que abrió este nodo.
    pub user: String,
    /// API de la cadena a la que queda ligado.
    pub api: String,
    /// Siempre abierto mientras figure en el registro.
    pub open: bool,
}

/// Foto de los nodos de comunidad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityState {
    /// Cuántos nodos están ligados ahora.
    pub open_nodes: u32,
    pub links: Vec<CommunityLink>,
}

/// Resultado de ligar o soltar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityStep {
    pub changed: bool,
    /// Índice del nodo que acaba de abrirse.
    pub opened: Option<u32>,
    /// Índice del nodo que acaba de soltarse.
    pub closed: Option<u32>,
    pub state: CommunityState,
}

/// Copia en disco de los nodos ligados.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCommunity {
    pub users: Vec<String>,
}

/// Registro de nodos de la comunidad ligados a una API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityNodes {
    api: String,
    users: Vec<String>,
}

impl CommunityNodes {
    /// Abre el registro vacío contra `api`.
    pub fn link(api: impl Into<String>) -> Result<Self, CommunityNodeError> {
        let api = api.into();
        if api.is_empty() {
            return Err(CommunityNodeError::EmptyApi);
        }
        Ok(Self {
            api,
            users: Vec::new(),
        })
    }

    /// API a la que quedan ligados los nodos.
    pub fn api(&self) -> &str {
        &self.api
    }

    /// El usuario liga un nodo completo.
    pub fn join(&mut self, user: &str) -> Result<CommunityStep, CommunityNodeError> {
        let user = user_id(user)?;
        if self.users.iter().any(|known| known == &user) {
            return Ok(self.step(false, None, None));
        }
        self.users.push(user);
        let opened = (self.users.len() as u32).saturating_sub(1);
        Ok(self.step(true, Some(opened), None))
    }

    /// El usuario suelta su nodo.
    pub fn leave(&mut self, user: &str) -> Result<CommunityStep, CommunityNodeError> {
        let user = user_id(user)?;
        let Some(position) = self.users.iter().position(|known| known == &user) else {
            return Ok(self.step(false, None, None));
        };
        self.users.remove(position);
        Ok(self.step(true, None, Some(position as u32)))
    }

    /// Capacidad comparable con nodos móviles (cada link = 1 asiento = 4/4).
    pub fn tier_capacity(&self) -> TierCapacity {
        TierCapacity {
            community_nodes: self.state().open_nodes,
            sealed_mobile_nodes: 0,
            partial_mobile_quarters: 0,
        }
    }

    /// Peso de un nodo comunitario (= peso de un móvil sellado).
    pub fn node_weight() -> u32 {
        NodeTier::Community.weight()
    }

    /// Foto actual.
    pub fn state(&self) -> CommunityState {
        let links = self
            .users
            .iter()
            .enumerate()
            .map(|(index, user)| CommunityLink {
                index: index as u32,
                user: user.clone(),
                api: self.api.clone(),
                open: true,
            })
            .collect();
        CommunityState {
            open_nodes: self.users.len() as u32,
            links,
        }
    }

    /// Copia para guardar en disco.
    pub fn export(&self) -> StoredCommunity {
        StoredCommunity {
            users: self.users.clone(),
        }
    }

    /// Abre un registro que ya se había empezado.
    pub fn load(api: impl Into<String>, stored: StoredCommunity) -> Result<Self, CommunityNodeError> {
        let mut nodes = Self::link(api)?;
        let mut seen = Vec::new();
        for user in stored.users {
            let user = user_id(&user)?;
            if seen.iter().any(|known: &String| known == &user) {
                return Err(CommunityNodeError::InvalidState);
            }
            seen.push(user.clone());
            nodes.users.push(user);
        }
        Ok(nodes)
    }

    fn step(
        &self,
        changed: bool,
        opened: Option<u32>,
        closed: Option<u32>,
    ) -> CommunityStep {
        CommunityStep {
            changed,
            opened,
            closed,
            state: self.state(),
        }
    }
}

fn user_id(user: &str) -> Result<String, CommunityNodeError> {
    if user.is_empty() {
        return Err(CommunityNodeError::EmptyUser);
    }
    Ok(user.to_string())
}

#[cfg(test)]
mod tests {
    use super::{CommunityNodeError, CommunityNodes};
    use rsc_node_tiers::COMMUNITY_NODE_WEIGHT;

    /// Bootstrap público (failover en apps). Debe estar vivo en ops.
    const API: &str = "https://rpc.reeskova-chain.xyz/rpc";

    #[test]
    fn community_seat_equals_four_mobile_quarters() {
        assert_eq!(COMMUNITY_NODE_WEIGHT, rsc_node_tiers::SEALED_MOBILE_NODE_WEIGHT);
        assert_eq!(CommunityNodes::node_weight(), 4);
        let mut nodes = CommunityNodes::link(API).unwrap();
        nodes.join("rsc-pc-1").unwrap();
        let capacity = nodes.tier_capacity();
        assert_eq!(capacity.complete_validator_seats(), 1);
        assert_eq!(capacity.total_weight(), 4);
    }

    #[test]
    fn a_user_links_one_full_node_from_any_terminal() {
        let mut nodes = CommunityNodes::link(API).unwrap();
        assert_eq!(nodes.state().open_nodes, 0);

        let step = nodes.join("rsc-pc-1").unwrap();
        assert!(step.changed);
        assert_eq!(step.opened, Some(0));
        assert_eq!(step.state.open_nodes, 1);
        assert!(step.state.links[0].open);
        assert_eq!(step.state.links[0].api, API);

        let again = nodes.join("rsc-pc-1").unwrap();
        assert!(!again.changed);
        assert_eq!(again.state.open_nodes, 1);

        let second = nodes.join("rsc-pc-2").unwrap();
        assert_eq!(second.opened, Some(1));
        assert_eq!(second.state.open_nodes, 2);
    }

    #[test]
    fn leaving_releases_only_that_node() {
        let mut nodes = CommunityNodes::link(API).unwrap();
        nodes.join("a").unwrap();
        nodes.join("b").unwrap();
        let step = nodes.leave("a").unwrap();
        assert!(step.changed);
        assert_eq!(step.closed, Some(0));
        assert_eq!(step.state.open_nodes, 1);
        assert_eq!(step.state.links[0].user, "b");
        assert!(!nodes.leave("a").unwrap().changed);
    }

    #[test]
    fn a_reload_keeps_linked_nodes() {
        let mut nodes = CommunityNodes::link(API).unwrap();
        nodes.join("rsc-one").unwrap();
        nodes.join("rsc-two").unwrap();
        let restored = CommunityNodes::load(API, nodes.export()).unwrap();
        assert_eq!(restored.state(), nodes.state());
        assert_eq!(CommunityNodes::link(""), Err(CommunityNodeError::EmptyApi));
        assert_eq!(nodes.join(""), Err(CommunityNodeError::EmptyUser));
    }
}
