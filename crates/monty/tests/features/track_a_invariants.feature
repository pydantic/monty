Feature: Track A compatibility invariants

  Scenario: Disabled observer-aware run execution matches baseline suspension
    Given a run script that suspends at one external call
    And the disabled observer-aware mode
    When baseline and observer-aware run execution both start
    Then both run modes suspend with matching external call payloads

  Scenario: No-op observer-aware REPL completion matches baseline completion
    Given the no-op observer-aware mode
    And a REPL snippet that completes without suspension
    When baseline and observer-aware REPL execution both start
    Then both REPL modes complete with the same observable result

  Scenario: No-op observer-aware REPL snapshot survives dump and load like baseline
    Given the no-op observer-aware mode
    And a REPL snippet that suspends and survives dump and load
    When baseline and observer-aware REPL execution both start
    And the observer-aware REPL progress is dumped and loaded
    Then both REPL modes still suspend with matching external call payloads
