use std::collections::BTreeMap;
use std::fmt;

use aos_effects::CapabilityBudget;
use serde::{Deserialize, Serialize};

pub type BudgetMap = BTreeMap<String, u64>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetEntry {
    pub limit: u64,
    pub reserved: u64,
    pub spent: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BudgetLedger {
    pub grants: BTreeMap<String, BTreeMap<String, BudgetEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapReservation {
    pub intent_hash: [u8; 32],
    pub cap_name: String,
    pub effect_kind: String,
    pub cap_type: String,
    pub enforcer_module: String,
    pub reserve: BudgetMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_ns: Option<u64>,
}

impl BudgetLedger {
    pub fn from_grants<I>(grants: I) -> Self
    where
        I: IntoIterator<Item = (String, Option<CapabilityBudget>)>,
    {
        let mut ledger = BudgetLedger::default();
        ledger.update_from_grants(grants);
        ledger
    }

    pub fn update_from_grants<I>(&mut self, grants: I)
    where
        I: IntoIterator<Item = (String, Option<CapabilityBudget>)>,
    {
        let mut next = BTreeMap::new();
        for (name, budget) in grants {
            let mut entries = BTreeMap::new();
            if let Some(budget) = budget {
                for (dim, limit) in budget.0 {
                    let (reserved, spent) = self
                        .grants
                        .get(&name)
                        .and_then(|existing| existing.get(&dim))
                        .map(|entry| (entry.reserved, entry.spent))
                        .unwrap_or((0, 0));
                    entries.insert(
                        dim,
                        BudgetEntry {
                            limit,
                            reserved,
                            spent,
                        },
                    );
                }
            }
            next.insert(name, entries);
        }
        self.grants = next;
    }

    pub fn check_reserve(&self, grant: &str, reserve: &BudgetMap) -> Result<(), BudgetError> {
        let Some(entries) = self.grants.get(grant) else {
            return Ok(());
        };
        for (dim, amount) in reserve {
            if let Some(entry) = entries.get(dim) {
                let total = entry
                    .spent
                    .saturating_add(entry.reserved)
                    .saturating_add(*amount);
                if total > entry.limit {
                    return Err(BudgetError::new(format!(
                        "budget exceeded for {grant}:{dim} ({total} > {})",
                        entry.limit
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn apply_reserve(&mut self, grant: &str, reserve: &BudgetMap) -> Result<(), BudgetError> {
        self.check_reserve(grant, reserve)?;
        if let Some(entries) = self.grants.get_mut(grant) {
            for (dim, amount) in reserve {
                if let Some(entry) = entries.get_mut(dim) {
                    entry.reserved = entry.reserved.saturating_add(*amount);
                }
            }
        }
        Ok(())
    }

    pub fn apply_settle(
        &mut self,
        grant: &str,
        reserve: &BudgetMap,
        usage: &BudgetMap,
    ) -> Result<(), BudgetError> {
        let Some(entries) = self.grants.get_mut(grant) else {
            return Ok(());
        };
        for (dim, amount) in reserve {
            if let Some(entry) = entries.get_mut(dim) {
                if entry.reserved < *amount {
                    return Err(BudgetError::new(format!(
                        "reservation underflow for {grant}:{dim}"
                    )));
                }
                entry.reserved -= *amount;
            }
        }
        for (dim, amount) in usage {
            if let Some(entry) = entries.get_mut(dim) {
                if let Some(reserved) = reserve.get(dim) {
                    if amount > reserved {
                        return Err(BudgetError::new(format!(
                            "usage exceeds reservation for {grant}:{dim}"
                        )));
                    }
                }
                let total = entry.spent.saturating_add(*amount);
                if total > entry.limit {
                    return Err(BudgetError::new(format!(
                        "budget exceeded for {grant}:{dim} ({total} > {})",
                        entry.limit
                    )));
                }
                entry.spent = total;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BudgetError {
    message: String,
}

impl BudgetError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
