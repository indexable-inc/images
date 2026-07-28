Feature: Account balance arithmetic
  Withdrawals may dip into an agreed overdraft but never past it,
  and a rejected operation leaves the balance untouched.

  Scenario: Deposits accumulate
    Given an account with a balance of $10.00
    When I deposit $2.50
    Then the balance is $12.50

  Scenario: A withdrawal may land exactly on the overdraft floor
    Given an account with a $5.00 overdraft and a balance of $10.00
    When I withdraw $15.00
    Then the balance is $-5.00

  Scenario: A withdrawal past the overdraft is rejected without side effects
    Given an account with a $5.00 overdraft and a balance of $10.00
    When I try to withdraw $15.01
    Then the operation fails, short $0.01
    And the error reads "insufficient funds: 1 cents short"
    And the balance is $10.00

  Scenario Outline: Non-positive amounts are rejected without side effects
    Given an account with a balance of $10.00
    When I try to <operation> $<amount>
    Then the operation fails as a non-positive amount
    And the error reads "amount must be positive"
    And the balance is $10.00

    Examples:
      | operation | amount |
      | deposit   | 0.00   |
      | deposit   | -1.00  |
      | withdraw  | 0.00   |
      | withdraw  | -1.00  |
