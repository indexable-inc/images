//! Step definitions binding `features/account.feature` to the crate API.
//!
//! Runs under the normal libtest harness (cucumber's suggested
//! `harness = false` is only about output ordering), so nextest and
//! cargo-unit treat it like any other test.

use core::str::FromStr;

use cucumber::{World, given, then, when};
use gherkin_poc::{Account, AccountError};

/// A dollars literal such as `12.50` or `-0.01`, parsed to signed cents.
///
/// A dedicated `FromStr` type keeps step signatures on Copy values instead of
/// `String` captures parsed inline in every step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cents(i64);

impl FromStr for Cents {
    type Err = core::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (sign, digits) = s.strip_prefix('-').map_or((1, s), |rest| (-1, rest));
        let (dollars, cents) = digits
            .split_once('.')
            .expect("money literals in the feature file carry a decimal point");
        Ok(Self(
            sign * (dollars.parse::<i64>()? * 100 + cents.parse::<i64>()?),
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    Deposit,
    Withdraw,
}

impl FromStr for Operation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "deposit" => Ok(Self::Deposit),
            "withdraw" => Ok(Self::Withdraw),
            other => Err(format!("unknown operation {other:?}")),
        }
    }
}

#[derive(Debug, Default, World)]
struct AccountWorld {
    account: Account,
    last_result: Option<Result<(), AccountError>>,
}

#[given(regex = r"^an account with a balance of \$(-?\d+\.\d{2})$")]
fn account_with_balance(world: &mut AccountWorld, balance: Cents) {
    world.account = Account::default();
    world
        .account
        .deposit(balance.0)
        .expect("feature files seed accounts with positive balances");
}

#[given(regex = r"^an account with a \$(\d+\.\d{2}) overdraft and a balance of \$(-?\d+\.\d{2})$")]
fn account_with_overdraft(world: &mut AccountWorld, overdraft: Cents, balance: Cents) {
    world.account = Account::with_overdraft(overdraft.0);
    world
        .account
        .deposit(balance.0)
        .expect("feature files seed accounts with positive balances");
}

#[when(regex = r"^I deposit \$(-?\d+\.\d{2})$")]
fn deposit(world: &mut AccountWorld, amount: Cents) {
    world
        .account
        .deposit(amount.0)
        .expect("a bare `I deposit` step expects success; use `I try to`");
}

#[when(regex = r"^I withdraw \$(-?\d+\.\d{2})$")]
fn withdraw(world: &mut AccountWorld, amount: Cents) {
    world
        .account
        .withdraw(amount.0)
        .expect("a bare `I withdraw` step expects success; use `I try to`");
}

#[when(regex = r"^I try to (deposit|withdraw) \$(-?\d+\.\d{2})$")]
const fn try_operation(world: &mut AccountWorld, operation: Operation, amount: Cents) {
    world.last_result = Some(match operation {
        Operation::Deposit => world.account.deposit(amount.0),
        Operation::Withdraw => world.account.withdraw(amount.0),
    });
}

#[then(regex = r"^the balance is \$(-?\d+\.\d{2})$")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber's macro requires steps to take `&mut World`"
)]
fn balance_is(world: &mut AccountWorld, expected: Cents) {
    assert_eq!(world.account.balance_cents(), expected.0);
}

#[then(regex = r"^the operation fails, short \$(\d+\.\d{2})$")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber's macro requires steps to take `&mut World`"
)]
fn fails_insufficient(world: &mut AccountWorld, missing: Cents) {
    assert_eq!(
        world.last_result,
        Some(Err(AccountError::InsufficientFunds {
            missing_cents: missing.0
        }))
    );
}

#[then(regex = r#"^the error reads "([^"]+)"$"#)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "cucumber step parameters are parsed via FromStr into owned values"
)]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber's macro requires steps to take `&mut World`"
)]
fn error_reads(world: &mut AccountWorld, expected: String) {
    let error = world
        .last_result
        .expect("an `I try to` step ran before this assertion")
        .expect_err("the operation was expected to fail");
    assert_eq!(error.to_string(), expected);
}

#[then("the operation fails as a non-positive amount")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber's macro requires steps to take `&mut World`"
)]
fn fails_non_positive(world: &mut AccountWorld) {
    assert_eq!(world.last_result, Some(Err(AccountError::NonPositiveAmount)));
}

#[test]
fn account_features() {
    futures::executor::block_on(AccountWorld::run(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/features"
    )));
}
