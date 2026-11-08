//! Cuartos de nodo que los usuarios ligan al pulsar minar.
//!
//! El enlace empieza vacío: nadie lo abre desde una terminal. El primer
//! usuario que pulsa minar ocupa el cuarto 1/4. A los cuatro el nodo queda
//! ligado para siempre, autónomo, aunque todos suelten. Ocho usuarios ligan
//! dos nodos. Un cuarto que no llega a cuatro se suelta si ese usuario para.
//!
//! Equivalencia ([`rsc_node_tiers`]): un móvil **4/4** pesa lo mismo que
//! **un nodo comunitario**. Cada cuarto vale 1/4 de ese asiento.

#![forbid(unsafe_code)]

use std::fmt;

use rsc_node_tiers::{NodeTier, TierCapacity, MOBILE_QUARTERS_PER_NODE};

/// Usuarios que hacen falta para ligar un nodo para siempre.
///
/// Alias de [`MOBILE_QUARTERS_PER_NODE`]: 4 móviles = 1 asiento (igual que
/// 1 comunitario).
pub const QUARTERS_PER_NODE: u32 = MOBILE_QUARTERS_PER_NODE;

/// Rechazo al ligar o soltar un cuarto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MobileNodeError {
    /// El identificador del usuario llegó vacío.
    EmptyUser,
    /// La API de la cadena llegó vacía.
    EmptyApi,
    /// El estado guardado no se puede volver a abrir.
    InvalidState,
}

impl fmt::Display for MobileNodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MobileNodeError::EmptyUser => write!(formatter, "usuario vacío"),
            MobileNodeError::EmptyApi => write!(formatter, "api vacía"),
            MobileNodeError::InvalidState => write!(formatter, "estado inválido"),
        }
    }
}

/// Nodo ligado a la API. Completo y autónomo, o todavía a medias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeLink {
    /// Índice estable: 0 es el primero que se liga.
    pub index: u32,
    /// Usuarios que ligaron este nodo, en el orden en que empezaron a minar.
    pub miners: Vec<String>,
    /// Cuartos ocupados. De 1 a [`QUARTERS_PER_NODE`].
    pub quarters: u32,
    /// `true` cuando el nodo llegó a cuatro y quedó abierto.
    pub open: bool,
    /// `true` cuando ya no depende de que alguien siga minando.
    pub autonomous: bool,
    /// API de la cadena a la que este nodo queda ligado.
    pub api: String,
}

/// Nodos ya autónomos y el cuarto que todavía se está armando.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MiningState {
    /// Usuarios minando ahora.
    pub miners: u32,
    /// Nodos ligados para siempre.
    pub open_nodes: u32,
    /// Cuartos del nodo que todavía no se completa. De 0 a 3.
    pub partial_quarters: u32,
    /// Nodos autónomos y, si existe, el nodo a medias al final.
    pub links: Vec<NodeLink>,
}

/// Resultado de pulsar minar o de dejar de minar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MineStep {
    /// El usuario cambió de estado en esta llamada.
    pub changed: bool,
    /// Índice del nodo que pasó a 4/4 y quedó autónomo.
    pub opened: Option<u32>,
    pub state: MiningState,
}

/// Nodo que ya se ligó y no vuelve a cerrarse.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SealedNode {
    index: u32,
    miners: Vec<String>,
}

/// Nodo autónomo tal como queda guardado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNode {
    pub index: u32,
    pub miners: Vec<String>,
}

/// Enlace guardado para volver a abrirlo sin empezar de cero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNodes {
    pub sealed: Vec<StoredNode>,
    pub committed: Vec<String>,
    pub pending: Vec<String>,
}

/// Usuarios minando y los nodos que ya quedaron autónomos.
///
/// Un usuario ocupa un solo cuarto. Pulsar minar otra vez no suma otro.
/// Quien ya ligó un nodo autónomo sigue contando como ocupado hasta que
/// suelta. Al soltar, ese nodo permanece. Si vuelve a minar, entra en el
/// nodo siguiente.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobileNodes {
    api: String,
    sealed: Vec<SealedNode>,
    /// Quienes ya ligaron un nodo y todavía no han soltado.
    committed: Vec<String>,
    /// Cuartos del nodo que aún no llega a cuatro.
    pending: Vec<String>,
}

impl MobileNodes {
    /// Liga los cuartos a `api`, por ejemplo `https://rpc.reeskova-chain.xyz/rpc`.
    pub fn link(api: impl Into<String>) -> Result<Self, MobileNodeError> {
        let api = api.into();
        if api.is_empty() {
            return Err(MobileNodeError::EmptyApi);
        }
        Ok(Self {
            api,
            sealed: Vec::new(),
            committed: Vec::new(),
            pending: Vec::new(),
        })
    }

    /// API a la que quedan ligados los nodos.
    pub fn api(&self) -> &str {
        &self.api
    }

    /// El usuario empieza a minar y ocupa un cuarto.
    pub fn mine(&mut self, user: &str) -> Result<MineStep, MobileNodeError> {
        let user = user_id(user)?;
        if self.is_mining(&user) {
            return Ok(self.step(false, None));
        }
        self.pending.push(user);
        let opened = self.seal_if_complete();
        Ok(self.step(true, opened))
    }

    /// El usuario deja de minar.
    ///
    /// Si su cuarto todavía no completó un nodo, ese cuarto se suelta.
    /// Si ya había ligado un nodo, el nodo sigue abierto y autónomo.
    pub fn stop(&mut self, user: &str) -> Result<MineStep, MobileNodeError> {
        let user = user_id(user)?;
        if let Some(position) = self.pending.iter().position(|miner| miner == &user) {
            self.pending.remove(position);
            return Ok(self.step(true, None));
        }
        if let Some(position) = self.committed.iter().position(|miner| miner == &user) {
            self.committed.remove(position);
            return Ok(self.step(true, None));
        }
        Ok(self.step(false, None))
    }

    /// Copia el enlace para guardarlo en disco.
    pub fn export(&self) -> StoredNodes {
        StoredNodes {
            sealed: self
                .sealed
                .iter()
                .map(|node| StoredNode {
                    index: node.index,
                    miners: node.miners.clone(),
                })
                .collect(),
            committed: self.committed.clone(),
            pending: self.pending.clone(),
        }
    }

    /// Abre un enlace que ya se había empezado. Vacío sigue vacío.
    pub fn load(api: impl Into<String>, stored: StoredNodes) -> Result<Self, MobileNodeError> {
        let mut nodes = Self::link(api)?;
        let mut busy = Vec::new();
        for node in stored.sealed {
            if node.miners.len() != QUARTERS_PER_NODE as usize {
                return Err(MobileNodeError::InvalidState);
            }
            for miner in &node.miners {
                user_id(miner)?;
            }
            let index = nodes.sealed.len() as u32;
            nodes.sealed.push(SealedNode {
                index,
                miners: node.miners,
            });
        }
        for miner in stored.committed {
            let miner = user_id(&miner)?;
            if busy.iter().any(|known: &String| known == &miner) {
                return Err(MobileNodeError::InvalidState);
            }
            let sealed = nodes
                .sealed
                .iter()
                .any(|node| node.miners.iter().any(|known| known == &miner));
            if !sealed {
                return Err(MobileNodeError::InvalidState);
            }
            busy.push(miner.clone());
            nodes.committed.push(miner);
        }
        if stored.pending.len() > QUARTERS_PER_NODE as usize {
            return Err(MobileNodeError::InvalidState);
        }
        for miner in stored.pending {
            let miner = user_id(&miner)?;
            if busy.iter().any(|known: &String| known == &miner) {
                return Err(MobileNodeError::InvalidState);
            }
            busy.push(miner.clone());
            nodes.pending.push(miner);
        }
        nodes.seal_if_complete();
        Ok(nodes)
    }

    /// Capacidad comparable con nodos comunitarios (asientos y peso).
    pub fn tier_capacity(&self) -> TierCapacity {
        let state = self.state();
        TierCapacity {
            community_nodes: 0,
            sealed_mobile_nodes: state.open_nodes,
            partial_mobile_quarters: state.partial_quarters,
        }
    }

    /// Peso de un nodo ya sellado (4/4) = peso de un comunitario.
    pub fn sealed_node_weight() -> u32 {
        NodeTier::MobileSealed.weight()
    }

    /// Foto de los nodos autónomos y del cuarto en curso.
    pub fn state(&self) -> MiningState {
        let mining = self.pending.len().saturating_add(self.committed.len());
        let miners = u32::try_from(mining).unwrap_or(u32::MAX);
        let open_nodes = u32::try_from(self.sealed.len()).unwrap_or(u32::MAX);
        let mut links = self
            .sealed
            .iter()
            .map(|node| NodeLink {
                index: node.index,
                miners: node.miners.clone(),
                quarters: QUARTERS_PER_NODE,
                open: true,
                autonomous: true,
                api: self.api.clone(),
            })
            .collect::<Vec<_>>();
        if !self.pending.is_empty() {
            links.push(NodeLink {
                index: open_nodes,
                miners: self.pending.clone(),
                quarters: self.pending.len() as u32,
                open: false,
                autonomous: false,
                api: self.api.clone(),
            });
        }
        MiningState {
            miners,
            open_nodes,
            partial_quarters: self.pending.len() as u32,
            links,
        }
    }

    fn is_mining(&self, user: &str) -> bool {
        self.pending.iter().any(|miner| miner == user)
            || self.committed.iter().any(|miner| miner == user)
    }

    fn seal_if_complete(&mut self) -> Option<u32> {
        if self.pending.len() < QUARTERS_PER_NODE as usize {
            return None;
        }
        let miners: Vec<String> = self.pending.drain(..).collect();
        let index = self.sealed.len() as u32;
        self.committed.extend(miners.iter().cloned());
        self.sealed.push(SealedNode { index, miners });
        Some(index)
    }

    fn step(&self, changed: bool, opened: Option<u32>) -> MineStep {
        MineStep {
            changed,
            opened,
            state: self.state(),
        }
    }
}

fn user_id(user: &str) -> Result<String, MobileNodeError> {
    if user.is_empty() {
        return Err(MobileNodeError::EmptyUser);
    }
    Ok(user.to_string())
}

#[cfg(test)]
mod tests {
    use super::{MobileNodeError, MobileNodes, QUARTERS_PER_NODE};

    /// Bootstrap público (failover en apps). Debe estar vivo en ops.
    const API: &str = "https://rpc.reeskova-chain.xyz/rpc";

    fn open_pool() -> MobileNodes {
        MobileNodes::link(API).unwrap()
    }

    fn mine_many(pool: &mut MobileNodes, count: usize) {
        for index in 0..count {
            pool.mine(&format!("user-{index}")).unwrap();
        }
    }

    #[test]
    fn sealed_mobile_matches_community_seat_weight() {
        assert_eq!(
            rsc_node_tiers::SEALED_MOBILE_NODE_WEIGHT,
            rsc_node_tiers::COMMUNITY_NODE_WEIGHT
        );
        let mut pool = open_pool();
        mine_many(&mut pool, 4);
        let capacity = pool.tier_capacity();
        assert_eq!(capacity.complete_validator_seats(), 1);
        assert_eq!(
            capacity.total_weight(),
            u128::from(rsc_node_tiers::SEALED_MOBILE_NODE_WEIGHT)
        );
    }

    #[test]
    fn four_users_seal_one_node_and_eight_seal_two() {
        let mut pool = open_pool();
        for index in 0..3 {
            let step = pool.mine(&format!("user-{index}")).unwrap();
            assert!(step.opened.is_none());
        }
        assert_eq!(pool.state().open_nodes, 0);
        assert_eq!(pool.state().partial_quarters, 3);

        let again = pool.mine("user-0").unwrap();
        assert!(!again.changed);
        assert_eq!(pool.state().miners, 3);

        let fourth = pool.mine("user-3").unwrap();
        assert_eq!(fourth.opened, Some(0));
        assert_eq!(fourth.state.open_nodes, 1);
        assert_eq!(fourth.state.partial_quarters, 0);
        assert!(fourth.state.links[0].open);
        assert!(fourth.state.links[0].autonomous);
        assert_eq!(fourth.state.links[0].quarters, QUARTERS_PER_NODE);
        assert_eq!(fourth.state.links[0].api, API);
        assert_eq!(
            fourth.state.links[0].miners,
            vec!["user-0", "user-1", "user-2", "user-3"]
        );

        for index in 4..7 {
            let step = pool.mine(&format!("user-{index}")).unwrap();
            assert!(step.opened.is_none());
        }
        assert_eq!(pool.state().open_nodes, 1);
        assert_eq!(pool.state().partial_quarters, 3);

        let eighth = pool.mine("user-7").unwrap();
        assert_eq!(eighth.opened, Some(1));
        assert_eq!(eighth.state.open_nodes, 2);
        assert_eq!(eighth.state.partial_quarters, 0);
        assert_eq!(eighth.state.links.len(), 2);
        assert!(eighth.state.links.iter().all(|link| link.autonomous));
    }

    #[test]
    fn a_sealed_node_stays_open_after_everyone_stops() {
        let mut pool = open_pool();
        mine_many(&mut pool, 4);
        for index in 0..4 {
            let step = pool.stop(&format!("user-{index}")).unwrap();
            assert!(step.opened.is_none());
            assert_eq!(step.state.open_nodes, 1);
            assert!(step.state.links[0].autonomous);
            assert_eq!(
                step.state.links[0].miners,
                vec!["user-0", "user-1", "user-2", "user-3"]
            );
        }
        assert_eq!(pool.state().miners, 0);
        assert_eq!(pool.state().partial_quarters, 0);
        assert_eq!(pool.state().open_nodes, 1);

        let again = pool.stop("user-0").unwrap();
        assert!(!again.changed);
        assert_eq!(again.state.open_nodes, 1);
    }

    #[test]
    fn stopping_before_four_releases_the_quarter() {
        let mut pool = open_pool();
        mine_many(&mut pool, 3);
        let step = pool.stop("user-1").unwrap();
        assert!(step.opened.is_none());
        assert_eq!(step.state.open_nodes, 0);
        assert_eq!(step.state.partial_quarters, 2);
        assert_eq!(step.state.links[0].miners, vec!["user-0", "user-2"]);
        assert!(!step.state.links[0].autonomous);
    }

    #[test]
    fn two_sealed_nodes_remain_when_their_miners_leave() {
        let mut pool = open_pool();
        mine_many(&mut pool, 8);
        for index in 0..8 {
            pool.stop(&format!("user-{index}")).unwrap();
        }
        let state = pool.state();
        assert_eq!(state.miners, 0);
        assert_eq!(state.open_nodes, 2);
        assert_eq!(state.partial_quarters, 0);
        assert_eq!(state.links.len(), 2);
        assert!(state.links.iter().all(|link| link.autonomous && link.open));
    }

    #[test]
    fn a_miner_who_left_a_sealed_node_can_start_the_next_quarter() {
        let mut pool = open_pool();
        mine_many(&mut pool, 4);
        pool.stop("user-0").unwrap();
        let step = pool.mine("user-0").unwrap();
        assert!(step.changed);
        assert!(step.opened.is_none());
        assert_eq!(step.state.open_nodes, 1);
        assert_eq!(step.state.partial_quarters, 1);
        assert_eq!(step.state.links[1].miners, vec!["user-0"]);
        assert!(!step.state.links[1].autonomous);
    }

    #[test]
    fn nine_users_seal_two_nodes_and_leave_one_quarter() {
        let mut pool = open_pool();
        mine_many(&mut pool, 9);
        let state = pool.state();
        assert_eq!(state.open_nodes, 2);
        assert_eq!(state.partial_quarters, 1);
        assert_eq!(state.links.len(), 3);
        assert!(state.links[0].autonomous);
        assert!(state.links[1].autonomous);
        assert_eq!(state.links[2].quarters, 1);
        assert!(!state.links[2].open);
        assert!(!state.links[2].autonomous);
    }

    #[test]
    fn empty_user_and_empty_api_are_rejected() {
        assert_eq!(MobileNodes::link(""), Err(MobileNodeError::EmptyApi));
        let mut pool = open_pool();
        assert_eq!(pool.mine(""), Err(MobileNodeError::EmptyUser));
        assert_eq!(pool.stop(""), Err(MobileNodeError::EmptyUser));
        assert_eq!(pool.state().miners, 0);
        assert_eq!(pool.state().open_nodes, 0);
    }

    #[test]
    fn the_first_press_starts_the_link_and_a_reload_keeps_it() {
        let mut pool = open_pool();
        assert_eq!(pool.state().open_nodes, 0);
        assert_eq!(pool.state().partial_quarters, 0);
        assert!(pool.state().links.is_empty());

        let step = pool.mine("rsc-one").unwrap();
        assert!(step.changed);
        assert!(step.opened.is_none());
        assert_eq!(step.state.partial_quarters, 1);
        assert!(!step.state.links[0].open);

        let restored = MobileNodes::load(API, pool.export()).unwrap();
        assert_eq!(restored.state(), pool.state());
        assert!(MobileNodes::load(
            API,
            super::StoredNodes {
                sealed: Vec::new(),
                committed: vec!["nadie".into()],
                pending: Vec::new(),
            },
        )
        .is_err());
    }
}
