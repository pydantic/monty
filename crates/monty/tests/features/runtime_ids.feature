Feature: Stable runtime IDs in suspendable execution

  Scenario: Runtime IDs remain stable across resume for persistent objects
    Given a suspendable script with repeated object external calls
    When execution pauses and resumes through both external calls
    Then the first and second runtime ID vectors are equal
    And the runtime ID vectors are non-empty

  Scenario: Runtime IDs survive run progress dump and load
    Given a suspendable script with one external call
    When run progress is dumped and loaded
    Then the first and second runtime ID vectors are equal
    And the runtime ID vectors are non-empty

  Scenario: Keyword runtime IDs survive run progress dump and load
    Given a suspendable script with one keyword-only external call
    When run progress is dumped and loaded
    Then the first and second kwarg runtime ID vectors are equal
    And the kwarg runtime ID vectors are non-empty
    And kwarg payload and runtime ID pairing remain aligned

  Scenario: OS-call runtime IDs survive run progress dump and load
    Given a suspendable script with one OS call
    When run progress is dumped and loaded
    Then the first and second runtime ID vectors are equal
    And the runtime ID vectors are non-empty

  Scenario: Corrupted run progress payload fails load
    Given a suspendable script with one external call
    When the serialized run progress payload is corrupted before load
    Then loading the progress payload fails
