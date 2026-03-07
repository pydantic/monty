Feature: Snapshot extension bytes

  Scenario: Run progress preserves snapshot extension bytes across dump/load
    Given a suspendable script with one external call
    And snapshot extension bytes
    When run progress is dumped and loaded with snapshot extension bytes
    Then the loaded snapshot extension bytes match

  Scenario: Corrupted run progress payload fails to load
    Given a suspendable script with one external call
    And snapshot extension bytes
    When run progress payload is corrupted
    Then loading the run progress fails

  Scenario: Corrupted REPL progress payload fails to load
    Given a REPL snippet with one external call
    And snapshot extension bytes
    When REPL progress payload is corrupted
    Then loading the REPL progress fails

  Scenario: REPL progress preserves snapshot extension bytes across dump/load
    Given a REPL snippet with one external call
    And snapshot extension bytes
    When REPL progress is dumped and loaded with snapshot extension bytes
    Then the loaded snapshot extension bytes match
