//! Jerarquía de nodos de participación en RSC Chain.
//!
//! Regla de equivalencia:
//! - **1 nodo comunitario** = **1 nodo móvil sellado (4/4)**.
//! - El nodo comunitario es más fuerte *por persona*: un solo usuario aporta
//!   el peso completo.
//! - El nodo móvil reparte ese mismo peso en **4 cuartos**; solo al sellar
//!   4/4 equivale a un comunitario.
//!
//! No decide consenso ni emite RSK: solo fija unidades de capacidad
//! comparables para ranking, cupos y reparto proporcional.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Cuartos móviles necesarios para sellar un nodo autónomo.
pub const MOBILE_QUARTERS_PER_NODE: u32 = 4;

/// Peso de un cuarto móvil incompleto.
pub const MOBILE_QUARTER_WEIGHT: u32 = 1;

/// Peso de un nodo móvil sellado (4/4). Igual al comunitario.
pub const SEALED_MOBILE_NODE_WEIGHT: u32 = MOBILE_QUARTERS_PER_NODE * MOBILE_QUARTER_WEIGHT;

/// Peso de un nodo comunitario (1 usuario = nodo completo).
pub const COMMUNITY_NODE_WEIGHT: u32 = SEALED_MOBILE_NODE_WEIGHT;

/// Clase de participación.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeTier {
    /// Usuario con PC/servidor: nodo completo al ligarse.
    Community,
    /// Cuarto móvil aún no sellado (1/4 … 3/4).
    MobileQuarter,
    /// Nodo móvil autónomo tras 4/4.
    MobileSealed,
}

impl NodeTier {
    /// Unidades de capacidad de esta clase.
    pub const fn weight(self) -> u32 {
        match self {
            Self::Community => COMMUNITY_NODE_WEIGHT,
            Self::MobileQuarter => MOBILE_QUARTER_WEIGHT,
            Self::MobileSealed => SEALED_MOBILE_NODE_WEIGHT,
        }
    }
}

/// Capacidad agregada del conjunto de nodos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TierCapacity {
    /// Nodos comunitarios abiertos.
    pub community_nodes: u32,
    /// Nodos móviles ya sellados (4/4).
    pub sealed_mobile_nodes: u32,
    /// Cuartos del nodo móvil en curso (0..=3).
    pub partial_mobile_quarters: u32,
}

impl TierCapacity {
    /// Peso total en unidades de cuarto.
    pub fn total_weight(self) -> u128 {
        let community = u128::from(self.community_nodes)
            .saturating_mul(u128::from(COMMUNITY_NODE_WEIGHT));
        let sealed = u128::from(self.sealed_mobile_nodes)
            .saturating_mul(u128::from(SEALED_MOBILE_NODE_WEIGHT));
        let partial = u128::from(self.partial_mobile_quarters)
            .saturating_mul(u128::from(MOBILE_QUARTER_WEIGHT));
        community.saturating_add(sealed).saturating_add(partial)
    }

    /// Cuántos “asientos” de validador equivalentes (solo nodos completos).
    ///
    /// Un asiento = 1 comunitario **o** 1 móvil 4/4. Los cuartos parciales
    /// no cuentan hasta sellarse.
    pub fn complete_validator_seats(self) -> u32 {
        self.community_nodes
            .saturating_add(self.sealed_mobile_nodes)
    }

    /// Reparte `amount` entre nodos comunitarios y móviles sellados a partes
    /// iguales por asiento completo.
    ///
    /// Los cuartos parciales no cobran. Si no hay asientos, ambos lados quedan
    /// en cero.
    pub fn split_reward_by_seat(self, amount: u128) -> SeatRewardSplit {
        let seats = u128::from(self.complete_validator_seats());
        if seats == 0 || amount == 0 {
            return SeatRewardSplit {
                per_seat: 0,
                community_total: 0,
                sealed_mobile_total: 0,
                remainder: amount,
            };
        }
        let per_seat = amount / seats;
        let community_total = per_seat.saturating_mul(u128::from(self.community_nodes));
        let sealed_mobile_total =
            per_seat.saturating_mul(u128::from(self.sealed_mobile_nodes));
        let used = community_total.saturating_add(sealed_mobile_total);
        SeatRewardSplit {
            per_seat,
            community_total,
            sealed_mobile_total,
            remainder: amount.saturating_sub(used),
        }
    }

    /// Dentro de un nodo móvil sellado, reparte el premio del asiento en
    /// [`MOBILE_QUARTERS_PER_NODE`] partes iguales (una por minero).
    pub fn split_sealed_mobile_seat(seat_amount: u128) -> MobileQuarterSplit {
        let quarters = u128::from(MOBILE_QUARTERS_PER_NODE);
        let per_quarter = seat_amount / quarters;
        let paid = per_quarter.saturating_mul(quarters);
        MobileQuarterSplit {
            per_quarter,
            remainder: seat_amount.saturating_sub(paid),
        }
    }
}

/// Reparto de recompensa entre asientos comunitarios y móviles sellados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatRewardSplit {
    /// Monto por asiento completo.
    pub per_seat: u128,
    /// Suma destinada a todos los comunitarios.
    pub community_total: u128,
    /// Suma destinada a todos los móviles 4/4 (luego se parte en 4).
    pub sealed_mobile_total: u128,
    /// Residuo de redondeo.
    pub remainder: u128,
}

/// Reparto interno de un asiento móvil entre sus 4 cuartos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MobileQuarterSplit {
    /// Parte de cada minero del 4/4.
    pub per_quarter: u128,
    /// Residuo de redondeo.
    pub remainder: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_equals_sealed_mobile() {
        assert_eq!(COMMUNITY_NODE_WEIGHT, SEALED_MOBILE_NODE_WEIGHT);
        assert_eq!(COMMUNITY_NODE_WEIGHT, 4);
        assert_eq!(NodeTier::Community.weight(), NodeTier::MobileSealed.weight());
        assert_eq!(
            NodeTier::Community.weight(),
            MOBILE_QUARTERS_PER_NODE * NodeTier::MobileQuarter.weight()
        );
    }

    #[test]
    fn one_community_matches_four_mobile_quarters() {
        let only_community = TierCapacity {
            community_nodes: 1,
            sealed_mobile_nodes: 0,
            partial_mobile_quarters: 0,
        };
        let four_quarters = TierCapacity {
            community_nodes: 0,
            sealed_mobile_nodes: 0,
            partial_mobile_quarters: 4,
        };
        // Cuartos sueltos aún no son asiento, pero pesan igual en capacidad bruta.
        assert_eq!(only_community.total_weight(), four_quarters.total_weight());
        assert_eq!(only_community.complete_validator_seats(), 1);
        assert_eq!(four_quarters.complete_validator_seats(), 0);

        let sealed = TierCapacity {
            community_nodes: 0,
            sealed_mobile_nodes: 1,
            partial_mobile_quarters: 0,
        };
        assert_eq!(only_community.complete_validator_seats(), sealed.complete_validator_seats());
        assert_eq!(only_community.total_weight(), sealed.total_weight());
    }

    #[test]
    fn reward_split_treats_community_and_sealed_alike() {
        let capacity = TierCapacity {
            community_nodes: 1,
            sealed_mobile_nodes: 1,
            partial_mobile_quarters: 2,
        };
        let split = capacity.split_reward_by_seat(1_000);
        assert_eq!(split.per_seat, 500);
        assert_eq!(split.community_total, 500);
        assert_eq!(split.sealed_mobile_total, 500);
        assert_eq!(split.remainder, 0);

        let mobile = TierCapacity::split_sealed_mobile_seat(split.sealed_mobile_total);
        assert_eq!(mobile.per_quarter, 125);
        assert_eq!(mobile.remainder, 0);
    }
}
