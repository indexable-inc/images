//! Toy account domain backing the Gherkin proof of concept.
//!
//! The behavior lives in `tests/features/account.feature` as plain-language
//! Given/When/Then scenarios; `tests/gherkin_account.rs` binds each step to
//! this API with [cucumber-rs](https://github.com/cucumber-rs/cucumber). The
//! crate exists to demonstrate that pairing, plus a `cargo mutants` audit of
//! how well the scenarios pin the arithmetic down (see `README.md` for the
//! recorded run). Tracked in indexable-inc/index#4091.

use core::fmt;

/// Why a deposit or withdrawal was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountError {
    /// The amount was zero or negative; money movements must be positive.
    NonPositiveAmount,
    /// The withdrawal would overshoot the overdraft floor by this much.
    InsufficientFunds {
        /// Cents missing even after exhausting the overdraft.
        missing_cents: i64,
    },
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveAmount => write!(f, "amount must be positive"),
            Self::InsufficientFunds { missing_cents } => {
                write!(f, "insufficient funds: {missing_cents} cents short")
            }
        }
    }
}

impl core::error::Error for AccountError {}

/// A balance in integer cents with an agreed overdraft.
///
/// The invariant the Gherkin scenarios defend: the balance never drops below
/// `-overdraft_limit_cents`, and a rejected operation leaves it untouched.
#[derive(Debug, Default)]
pub struct Account {
    balance_cents: i64,
    overdraft_limit_cents: i64,
}

impl Account {
    /// An empty account allowed to go `limit_cents` below zero.
    #[must_use]
    pub const fn with_overdraft(limit_cents: i64) -> Self {
        Self {
            balance_cents: 0,
            overdraft_limit_cents: limit_cents,
        }
    }

    /// Current balance in cents; negative once inside the overdraft.
    #[must_use]
    pub const fn balance_cents(&self) -> i64 {
        self.balance_cents
    }

    /// Add `amount_cents` to the balance.
    ///
    /// # Errors
    ///
    /// [`AccountError::NonPositiveAmount`] when `amount_cents <= 0`.
    pub const fn deposit(&mut self, amount_cents: i64) -> Result<(), AccountError> {
        if amount_cents <= 0 {
            return Err(AccountError::NonPositiveAmount);
        }
        self.balance_cents += amount_cents;
        Ok(())
    }

    /// Remove `amount_cents` from the balance, dipping into the overdraft
    /// down to (and including) its limit.
    ///
    /// # Errors
    ///
    /// [`AccountError::NonPositiveAmount`] when `amount_cents <= 0`;
    /// [`AccountError::InsufficientFunds`] when the withdrawal would land
    /// below the overdraft floor, in which case the balance is unchanged.
    pub const fn withdraw(&mut self, amount_cents: i64) -> Result<(), AccountError> {
        if amount_cents <= 0 {
            return Err(AccountError::NonPositiveAmount);
        }
        let floor = -self.overdraft_limit_cents;
        let after = self.balance_cents - amount_cents;
        if after < floor {
            return Err(AccountError::InsufficientFunds {
                missing_cents: floor - after,
            });
        }
        self.balance_cents = after;
        Ok(())
    }
}
